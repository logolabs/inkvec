/**
 * The Vectorize tab: the screen where 90% of the time is spent.
 *
 * Four zones — app bar (owned by main), stage, right rail, status strip. This module owns
 * the stage: the viewer's toolbar, the viewer itself, the states the stage can be in when
 * it is not showing a drawing, and the line along the bottom.
 */

import { fill, h, icon } from "../lib/dom";
import type { SampleInfo, Settings } from "../lib/ipc";
import { count, de00, modKey, plannedTracePx, seconds, type StageState, type Store } from "../lib/state";
import { createViewer, type Viewer } from "../components/viewer";
import { DESKTOP_URL, WEB } from "../lib/platform";
import { fetchBar, fetchPercent } from "../components/denoiserfetch";
import { createActivity } from "../components/activity";
import { elapsedText, runningLabel } from "../lib/live";

/** What the stage's buttons ask the app to do. */
export interface WorkspaceActions {
  openFile(): void;
  openSample(file: string): void;
  traceNow(): void;
  cancel(): void;
  retryAt(px: number): void;
  jumpToWorst(): void;
  openDenoiser(): void;
  /** Inkvec Studio Lite: try the denoiser's download again after it failed. */
  retryDenoiser(): void;
  showUpdate(): void;
  /** The first-run introduction has been read. */
  markSeen(): void;
  changeSetting?: (key: keyof Settings, value: Settings[keyof Settings]) => void;
}

/** The stage, as `main.ts` holds it. */
export interface Workspace {
  el: HTMLElement;
  viewer: Viewer;
  /** Put something over the stage that manages its own visibility: the chooser card. */
  mount(node: HTMLElement): void;
}

/**
 * The Vectorize tab's stage: the viewer's toolbar, the viewer (or the empty state with the
 * samples before an image is open, or a message when a trace could not be shown), the
 * activity log, and the status strip along the bottom.
 */
export function createWorkspace(store: Store, act: WorkspaceActions, samples: () => SampleInfo[]): Workspace {
  const viewer = createViewer(store);
  const tools = h("div.viewertools");
  const stageBody = h("div", { style: { flex: "1", minHeight: "0", position: "relative", display: "flex" } });
  const strip = h("div.statusstrip");
  // The page's `main` landmark (the app bar is its banner, the rail its complementary
  // region): every tab's stage is one, so the toolbar, the viewer and the status strip are
  // inside a landmark and a screen reader can jump straight to them.
  const el = h("main.stage", null, tools, stageBody, strip);

  // The toolbar reads the zoom only to say which stop is pressed. A wheel tick changes the
  // zoom every frame and the pressed stop almost never, so the bar is rebuilt only when
  // something it shows has changed.
  let toolsDrawn = "";
  const renderTools = () => {
    const st = store.state;
    const on = Boolean(st.source);
    const stops = [1, 4, 12].map((z) => !st.fitted && Math.abs(st.zoom - z) < 0.01);
    const key = JSON.stringify([on, st.view, st.show, st.fitted, stops, st.detail, Boolean(st.svg), st.bandsMissing, st.settings.mode]);
    if (key === toolsDrawn) return;
    toolsDrawn = key;
    // Pressing a stop changes what the bar shows, so the bar is rebuilt under the key that
    // pressed it; focus goes back to the button's twin (by `data-ctl`), as the rail does.
    const focused =
      document.activeElement instanceof HTMLElement && tools.contains(document.activeElement)
        ? document.activeElement.getAttribute("data-ctl")
        : null;
    const zoomStop = (label: string, z: number | "fit") =>
      h(
        "button",
        {
          "aria-pressed": String(z === "fit" ? st.fitted : !st.fitted && Math.abs(st.zoom - z) < 0.01),
          disabled: !on,
          "data-ctl": `zoom:${z}`,
          onclick: () => (z === "fit" ? viewer.fit() : viewer.zoomTo(z)),
        },
        label,
      );
    // The pan buttons: the single-pointer way to move a zoomed view (WCAG 2.2 SC 2.5.7;
    // dragging was the only one). Each moves the view by a quarter of the pane. Shown once
    // the view is not fitted, which is when there is anything off screen to pan to.
    const panStep = 0.25;
    const panButton = (dir: "left" | "up" | "down" | "right", dx: number, dy: number) =>
      h(
        `button.pan.${dir}`,
        {
          "aria-label": `Pan ${dir}`,
          title: `Pan ${dir} (or focus the viewer and use the arrow keys)`,
          "data-ctl": `pan:${dir}`,
          onclick: () => viewer.panBy(dx * panStep, dy * panStep),
        },
        icon(dir === "left" || dir === "right" ? "chevronRight" : "chevronDown", 12),
      );

    fill(
      tools,
      h(
        "div.seg",
        null,
        ...(["side", "wipe", "ab"] as const).map((v) =>
          h(
            "button",
            {
              "aria-pressed": String(st.view === v),
              disabled: !on,
              "data-ctl": `view:${v}`,
              onclick: () => store.set({ view: v }),
            },
            v === "side" ? "Side by side" : v === "wipe" ? "Wipe" : "A/B",
          ),
        ),
      ),
      h("span.muted.flickhint", { style: { fontSize: "11.5px" } }, "hold ", h("kbd", null, "Space"), " to flick"),
      h("div.sep"),
      h("span.eyebrow", null, "Show"),
      h(
        "div",
        { style: { display: "flex", gap: "6px" } },
        ...(
          [
            ["fill", "Fill"],
            ["wireframe", "Wireframe"],
            ["anchors", "Anchors"],
            ["handles", "Handles"],
            ["certainty", "Certainty"],
          ] as const
        ).map(([key, label]) =>
          h(
            "button.toggle",
            {
              "aria-pressed": String(st.show[key]),
              "data-ctl": `show:${key}`,
              // The bands are fetched when Certainty is first shown, so whether a trace
              // has any is only known once somebody asks.
              disabled: !st.svg || (key === "certainty" && st.bandsMissing),
              title:
                key === "certainty"
                  ? "Where each boundary could be: a band two sigmas either side, measured from the pixels. Thin is certain; a wide band is a soft, blurred or compressed edge, and the curve there is a best guess."
                  : undefined,
              onclick: () => {
                st.show[key] = !st.show[key];
                store.touch("show");
              },
            },
            label,
          ),
        ),
      ),
      h(
        "div",
        { style: { marginLeft: "auto", display: "flex", alignItems: "center", gap: "12px" } },
        // The key to the dots is only worth the room while there are dots to read.
        st.show.wireframe || st.show.anchors || st.show.handles || st.show.certainty
          ? h(
              "div.legend",
              null,
              st.show.anchors ? h("span", null, h("i.anchor"), "anchor") : null,
              st.show.handles ? h("span", null, h("i.handle"), "handle") : null,
              st.show.wireframe ? h("span", null, h("i.wire"), "wireframe") : null,
              ...(st.show.certainty
                ? [
                    h("span", null, h("i.band.sure"), "sure"),
                    h("span", null, h("i.band.soft"), "soft"),
                    h("span", null, h("i.band.unsure"), "unsure"),
                  ]
                : []),
            )
          : null,
        h(
          "button.toggle",
          {
            "aria-pressed": String(st.detail),
            disabled: !st.svg,
            "data-ctl": "detail",
            title: "Show the source's pixel grid under the vector edge",
            onclick: () => {
              const on = !st.detail;
              store.set({ detail: on });
              // The pixel grid is only legible past about 6x, so turning it on takes
              // the view there rather than leaving it somewhere it cannot be read.
              if (on && st.zoom < 12) viewer.zoomTo(12);
            },
          },
          "Detail",
        ),
        on && !st.fitted
          ? h("div.seg.panpad", { role: "group", "aria-label": "Pan the view" }, panButton("left", -1, 0), panButton("up", 0, -1), panButton("down", 0, 1), panButton("right", 1, 0))
          : null,
        h("div.seg.num", null, zoomStop("Fit", "fit"), zoomStop("1×", 1), zoomStop("4×", 4), zoomStop("12×", 12)),
      ),
    );
    if (focused) {
      // A pan button that went away (the view was fitted) hands the keyboard to Fit.
      (tools.querySelector<HTMLElement>(`[data-ctl="${focused}"]`) ?? (focused.startsWith("pan:") ? tools.querySelector<HTMLElement>('[data-ctl="zoom:fit"]') : null))?.focus({ preventScroll: true });
    }
  };

  // The viewer stays mounted for good, hidden while there is nothing to view, and what
  // sits over it — the stage states, the result chip, the detail callout — is rebuilt on
  // its own. Detaching and reattaching the viewer restyled and relaid out every node of
  // the drawing for a chip changing its label. `display: contents` keeps the chrome's
  // children positioned against the stage exactly as if they were its own.
  const chrome = h("div", { style: { display: "contents" } });
  stageBody.append(viewer.el, chrome, createActivity(store));

  const renderStage = () => {
    const st = store.state;
    if (!st.source) {
      viewer.el.style.display = "none";
      // No image is open, but the last attempt may have failed: a file that would not open
      // as the first of the session used to leave this screen exactly as it was, so the
      // user got no answer at all (3 of 16 inputs, r2-product). The empty state now leads
      // with what went wrong, and keeps the drop zone and the samples to carry on from.
      fill(chrome, firstRun(store, act, samples(), firstFailure(st.stageState)));
      return;
    }
    viewer.el.style.display = "";
    fill(
      chrome,
      stateOverlay(st.stageState, st, act),
      st.svg ? resultChip(store) : null,
      st.detail && st.svg ? detailCallout(store, act) : null,
    );
    if (st.zoom === 1 && st.pan.x === 0 && st.pan.y === 0) viewer.fit();
  };

  const renderStrip = () => {
    const st = store.state;
    const r = st.report;
    let lead = "Ready";
    let rest = "";
    let colour = "var(--dim)";
    let live: HTMLElement | null = null;

    if (st.tracing) {
      lead = runningLabel(st);
      const engineTag = st.settings.mode === "fast" ? "fast" : "quality";
      rest = `· ${engineTag} · ${plannedTracePx(st, st.tracingTier) ?? "—"} px ·`;
      live = h("span", null, h("span.num", { "data-live": "elapsed" }, elapsedText(st)), " elapsed · Esc to cancel");
      colour = "var(--state-stale)";
    } else if (r && st.result) {
      const draft = st.result.tier === "draft";
      const engineTag = st.settings.mode === "fast" ? "Fast" : "Quality";
      lead = draft ? `${engineTag} draft` : `${engineTag} in ${seconds(r.seconds)}`;
      rest = draft
        ? `· ${r.tracedPx} px · full trace queued`
        : `· ${r.tracedPx} px · final · ${count(r.segments)} segments`;
      colour = draft ? "var(--state-draft)" : "var(--state-final)";
    } else if (st.source) {
      lead = "Opened";
      rest = `· ${st.source.width} × ${st.source.height} · ${st.source.container}`;
    }

    fill(
      strip,
      h("span.lead", { style: { color: colour } }, lead),
      rest ? h("span", null, rest) : null,
      live,
      // The update chip lives here and nowhere else: never a modal, never timed.
      st.update?.newer
        ? h(
            "span",
            { style: { display: "flex", alignItems: "center", gap: "8px", marginLeft: "12px" } },
            h("span.updatechip", null, h("i"), `${st.update.latest} available`),
            h("button.reset", { onclick: act.showUpdate }, "What changed"),
          )
        : null,
      WEB ? denoiserChip(store, act) : null,
      // Fixed wording, used identically everywhere it appears.
      h("span.privacy", null, "Your image never leaves this computer."),
    );
  };

  store.on(["source", "view", "show", "zoom", "fitted", "detail", "svg", "bandsMissing", "settings"], renderTools);
  store.on(["source", "svg", "stageState", "result", "detail", "worstCorner", "prefs"], renderStage);
  store.on(["tracing", "liveStages", "liveNow", "report", "result", "source", "update", "denoiserFetch", "settings"], renderStrip);

  renderTools();
  renderStage();
  renderStrip();

  // Scroll wheel, drag and double-click are the viewer's; these are the keyboard's.
  window.addEventListener("keydown", (e) => {
    if (store.state.screen || store.state.tab !== "vectorize") return;
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
    const st = store.state;
    if (e.key === " " && !e.repeat) {
      e.preventDefault();
      store.set({ flicked: true });
    } else if (e.key === "0") {
      viewer.fit();
    } else if (e.key === "+" || e.key === "=") {
      viewer.zoomTo(st.zoom * 1.4);
    } else if (e.key === "-") {
      viewer.zoomTo(st.zoom / 1.4);
    }
  });
  window.addEventListener("keyup", (e) => {
    if (e.key === " ") store.set({ flicked: false });
  });

  return { el, viewer, mount: (node) => stageBody.append(node) };
}

/**
 * Inkvec Studio Lite: the denoiser's background download, small, in the status strip while
 * it runs; gone once it is stored, and a note with a retry if it failed. The Denoiser control
 * in the rail says more when the controls are waiting for it.
 */
function denoiserChip(store: Store, act: WorkspaceActions): HTMLElement | null {
  const st = store.state;
  const f = st.denoiserFetch;
  if (!f || !st.caps?.denoiser.supported) return null;
  const wanted = st.settings.cleanUpDamage !== "off";
  const mb = (n: number) => Math.round(n / (1024 * 1024));
  if (f.phase === "downloading") {
    const pct = fetchPercent(f);
    return h(
      "span.fetchchip",
      { "data-ctl": "denoiser-chip", title: "The denoiser downloads in the background, once; later visits read it from this browser" },
      fetchBar(f),
      f.total ? `Denoiser ${mb(f.got)} of ${mb(f.total)} MB · ${pct}%` : `Denoiser ${mb(f.got)} MB`,
    );
  }
  if ((f.phase === "stored" || f.phase === "preparing") && wanted) {
    return h("span.fetchchip", { "data-ctl": "denoiser-chip" }, fetchBar(f), "Preparing the denoiser…");
  }
  if (f.phase === "failed") {
    return h(
      "span.fetchchip.bad",
      { "data-ctl": "denoiser-chip", title: f.message ?? "" },
      "Denoiser download failed",
      h("button.reset", { "data-ctl": "denoiser-chip-retry", onclick: act.retryDenoiser }, "Retry"),
    );
  }
  return null;
}

/** The draft/final/re-tracing chip, beside the result it describes. */
function resultChip(store: Store): HTMLElement {
  const st = store.state;
  const kind = st.tracing ? "stale" : st.result?.tier === "draft" ? "draft" : "final";
  const label = st.tracing
    ? `re-tracing · ${plannedTracePx(st, st.tracingTier) ?? "—"} px`
    : `${st.result?.tier ?? "final"} · ${st.report?.tracedPx ?? "—"} px`;
  return h(
    `span.chip.${kind}${st.justSwapped ? ".swapped" : ""}`,
    { style: { position: "absolute", top: "46px", right: "20px" } },
    h("i"),
    label,
  );
}

/**
 * The detail-mode callout.
 *
 * Detail mode draws the source's pixel grid under the vector edge, and the only thing
 * worth saying while it is on is where to point it. The sentence is the measurement, not
 * a claim about it: if there is no worst corner it says the trace and the source agree
 * everywhere we can measure, which is what a null means.
 */
function detailCallout(store: Store, act: WorkspaceActions): HTMLElement {
  const st = store.state;
  return h(
    "div.callout",
    null,
    h(
      "div.head",
      null,
      // 12x is where Jump to it lands, not where the view happens to be: a live zoom
      // here would go stale on every wheel tick unless the stage rebuilt with it.
      h("span.eyebrow", null, st.worstCorner ? "Worst corner · 12×" : "Detail"),
      st.worstCorner ? h("button.reset", { onclick: act.jumpToWorst }, "Jump to it") : null,
    ),
    h("span", null, worstCornerCaption(store)),
  );
}

/**
 * The stage states that are not a drawing.
 *
 * Copy carries the weight in every one of them, and none of them apologises. "Oversized"
 * in particular exists because people find it confusing: it says plainly that the trace
 * measured at one size and the SVG is still full size, and that this is normal.
 */
function stateOverlay(state: StageState, st: ReturnType<Store["state"]["valueOf"]> & Store["state"], act: WorkspaceActions): HTMLElement | null {
  const frame = (glyph: string, colour: string, title: string, body: string, action?: [string, () => void]) =>
    h(
      "div.stagestate",
      null,
      h("span", { style: { color: colour, display: "flex" } }, icon(glyph, 24)),
      h("span.title", null, title),
      h("span.body", null, body),
      action ? h("button.action.reset", { onclick: action[1] }, action[0]) : null,
    );

  switch (state.kind) {
    case "decoding":
      return frame("clock", "var(--faint)", `Reading ${st.source?.name ?? "the image"}`, `${st.source?.width} × ${st.source?.height}. Tracing starts as soon as the pixels are in memory.`);
    case "flat":
      return frame("info", "var(--gold)", "This image is one flat colour", "There are no boundaries to trace. A single rectangle would be the whole output.", ["Open another image", act.openFile]);
    case "undecodable":
      return frame("alert", "var(--bad)", "This file will not decode", plainMessage(state.message), ["Open another image", act.openFile]);
    case "outOfMemory":
      return frame(
        "alert",
        "var(--bad)",
        "The trace would run out of memory",
        `About ${state.neededGb.toFixed(1)} GB is needed at this trace size. Lowering it to ${state.suggestPx} px usually costs nothing visible.`,
        [`Retry at ${state.suggestPx} px`, () => act.retryAt(state.suggestPx)],
      );
    case "failed":
      return frame("alert", "var(--bad)", "The trace stopped", state.message, ["Trace again", act.traceNow]);
    case "cancelled":
      return frame("x", "var(--faint)", "Trace cancelled", "The last full trace is still on screen and still exportable. Nothing was discarded.", ["Trace again", act.traceNow]);
    // Not an error: the preset works without it, the colours just keep the compression
    // damage. The download is explained before anything is fetched.
    case "denoiserMissing":
      return frame(
        "download",
        "var(--faint)",
        "This preset wants the denoiser",
        "Photo or scan works without it, but compression damage stays in the colours. The download is explained before anything is fetched, and it runs locally like everything else.",
        ["Download the denoiser", act.openDenoiser],
      );
    // Tracing never needed a network. Saying so is the whole message.
    case "offline":
      return frame(
        "globe",
        "var(--faint)",
        "No network",
        `Tracing is unaffected — it never needed one. ${state.message}`,
      );
    default:
      return null;
  }
}

/**
 * What happens when an image is opened, said once, on the first launch: the automatic
 * trace starts at once, and a card offers the Custom wizard beside it. Three lines and a
 * button, over the empty stage; never shown again once read or once an image has been
 * opened and the card answered.
 */
function firstRunIntro(store: Store, act: WorkspaceActions): HTMLElement | null {
  const p = store.state.prefs;
  if (!p || p.seenFirstRun) return null;
  const step = (n: string, title: string, body: string) =>
    h("li", null, h("span.introstep.num", null, n), h("div", null, h("span.introtitle", null, title), h("span.faint", null, body)));
  return h(
    "div.intro",
    { role: "note" },
    h("span.eyebrow", null, "How opening an image works"),
    h(
      "ol",
      null,
      step("1", "Auto traces it at once.", "The moment it opens, with nothing to choose first."),
      step("2", "A small card offers Custom.", "A few short steps, each explained, with the result beside Auto's as you go. Ignore it and Auto stands."),
      step("3", "Everything is an ordinary setting.", "What Custom chooses is under Tune afterwards, to change like any other control."),
    ),
    h(
      "div.introfoot",
      null,
      h("span.muted", null, "Settings has a switch to always use Auto, or always Custom."),
      h("button.btn.compact", { "data-ctl": "intro-seen", onclick: act.markSeen }, "Got it"),
    ),
  );
}

/**
 * What to say over the empty state when the file just chosen did not open: the backend's
 * message, which names the format it found, and what can be opened instead. Null when the
 * stage is not in a failed state.
 */
export function firstFailure(state: StageState): { title: string; body: string } | null {
  switch (state.kind) {
    case "undecodable":
      return { title: "That file did not open", body: plainMessage(state.message, true) };
    case "failed":
      return { title: "The trace stopped", body: plainMessage(state.message) };
    case "outOfMemory":
      return { title: "That image is too large to open here", body: `About ${state.neededGb.toFixed(1)} GB would be needed. A smaller copy of it will open.` };
    default:
      return null;
  }
}

/**
 * A backend message as a sentence for the screen: without the `Error: ` a thrown error's
 * text starts with, and, where the screen says which formats open in its own words
 * (`withoutFormats`), without the core's sentence listing them, so it is not said twice.
 */
export function plainMessage(message: string, withoutFormats = false): string {
  let m = message.replace(/^(Error:\s*)+/, "");
  if (withoutFormats) m = m.replace(/\s*PNG, JPEG, WebP, BMP, GIF and TIFF are supported\.?\s*$/, "");
  return m;
}

/** The formats the open dialog and the drop zone take, as the drop zone says them. */
export const OPENS = "PNG, JPEG, WebP, GIF, BMP, TIFF or SVG";

/** First run, and the empty state the app returns to; `failed` leads it when a file did not open. */
function firstRun(store: Store, act: WorkspaceActions, samples: SampleInfo[], failed: { title: string; body: string } | null = null): HTMLElement {
  const mod = modKey(store.state.caps?.platform);
  return h(
    "div.firstrun",
    null,
    failed
      ? h(
          "div.firstfail",
          // An alert, so a screen reader says it the moment it appears: the file chooser has
          // just closed and nothing else on the screen changed.
          { role: "alert" },
          h("span.glyph", null, icon("alert", 20)),
          h(
            "div",
            null,
            h("span.title", null, failed.title),
            h("span.body", null, failed.body),
            h(
              "span.body.faint",
              null,
              `Inkvec opens ${OPENS} files. A photo from a phone (HEIC, AVIF) or a PDF opens once it is saved as a PNG or JPEG.`,
            ),
          ),
        )
      : null,
    failed ? null : firstRunIntro(store, act),
    h(
      "div.drop",
      null,
      h("span.glyph", null, icon("image", 32)),
      h(
        "div",
        { style: { display: "flex", flexDirection: "column", gap: "6px" } },
        h("span.headline.resting", null, "Drop an image"),
        h("span.headline.release", null, "Release to trace"),
        // Every format the open dialog takes, not three of them (the old line named PNG, JPEG
        // and WebP while seven were accepted).
        h("span.faint.resting", { style: { fontSize: "13px" } }, OPENS),
        h(
          "span.faint.resting.kbdhint",
          { style: { fontSize: "13px" } },
          "or press ",
          h("kbd", null, `${mod}+O`),
          " to choose a file",
        ),
      ),
      h("button.btn.primary.resting", { onclick: act.openFile }, failed ? "Open another image" : "Open an image"),
    ),
    samples.length
      ? h(
          "div.samples",
          null,
          h("span.eyebrow", null, "Or try one of these"),
          h(
            "div.samplerow",
            null,
            ...samples.map((sample) =>
              h(
                "button.sample",
                { onclick: () => act.openSample(sample.file) },
                h("span.thumb", null, h("img", { src: sample.preview, alt: "" })),
                h("span.label", null, sample.label),
              ),
            ),
          ),
        )
      : null,
    // The browser build says once, here, where the full app is.
    WEB
      ? h(
          "p.desktopnote.faint",
          null,
          "For the fastest engine, whole folders in one batch, no browser limits and offline use, get ",
          h("a", { href: DESKTOP_URL, target: "_blank", rel: "noopener noreferrer" }, "Inkvec Studio for the desktop"),
          ".",
        )
      : null,
  );
}

/** The "find the worst corner" jump, exported so the rail can call it too. */
export function jumpToWorst(store: Store, viewer: Viewer): void {
  const corner = store.state.worstCorner;
  if (!corner) return;
  store.set({ detail: true });
  viewer.goTo(corner.x, corner.y, 12);
}

/** A short description of the worst corner, for the detail pane's caption. */
export function worstCornerCaption(store: Store): string {
  const c = store.state.worstCorner;
  if (!c) return "The trace and the source agree everywhere we can measure.";
  return `Worst disagreement: ${de00(c.de00)} dE00, at ${c.x} × ${c.y} in the traced raster.`;
}
