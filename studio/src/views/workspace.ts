/**
 * The Vectorize tab: the screen where 90% of the time is spent.
 *
 * Four zones — app bar (owned by main), stage, right rail, status strip. This module owns
 * the stage: the viewer's toolbar, the viewer itself, the states the stage can be in when
 * it is not showing a drawing, and the line along the bottom.
 */

import { fill, h, icon } from "../lib/dom";
import type { SampleInfo } from "../lib/ipc";
import { count, de00, modKey, plannedTracePx, seconds, type StageState, type Store } from "../lib/state";
import { createViewer, type Viewer } from "../components/viewer";

export interface WorkspaceActions {
  openFile(): void;
  openSample(file: string): void;
  traceNow(): void;
  cancel(): void;
  retryAt(px: number): void;
  jumpToWorst(): void;
  openDenoiser(): void;
  showUpdate(): void;
}

export interface Workspace {
  el: HTMLElement;
  viewer: Viewer;
}

export function createWorkspace(store: Store, act: WorkspaceActions, samples: () => SampleInfo[]): Workspace {
  const viewer = createViewer(store);
  const tools = h("div.viewertools");
  const stageBody = h("div", { style: { flex: "1", minHeight: "0", position: "relative", display: "flex" } });
  const strip = h("div.statusstrip");
  const el = h("section.stage", null, tools, stageBody, strip);

  // The toolbar reads the zoom only to say which stop is pressed. A wheel tick changes the
  // zoom every frame and the pressed stop almost never, so the bar is rebuilt only when
  // something it shows has changed.
  let toolsDrawn = "";
  const renderTools = () => {
    const st = store.state;
    const on = Boolean(st.source);
    const stops = [1, 4, 12].map((z) => !st.fitted && Math.abs(st.zoom - z) < 0.01);
    const key = JSON.stringify([on, st.view, st.show, st.fitted, stops, st.detail, Boolean(st.svg), st.bandsMissing]);
    if (key === toolsDrawn) return;
    toolsDrawn = key;
    const zoomStop = (label: string, z: number | "fit") =>
      h(
        "button",
        {
          "aria-pressed": String(z === "fit" ? st.fitted : !st.fitted && Math.abs(st.zoom - z) < 0.01),
          disabled: !on,
          onclick: () => (z === "fit" ? viewer.fit() : viewer.zoomTo(z)),
        },
        label,
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
              onclick: () => store.set({ view: v }),
            },
            v === "side" ? "Side by side" : v === "wipe" ? "Wipe" : "A/B",
          ),
        ),
      ),
      h("span.muted", { style: { fontSize: "11.5px" } }, "hold ", h("kbd", null, "Space"), " to flick"),
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
        h("div.seg.num", null, zoomStop("Fit", "fit"), zoomStop("1×", 1), zoomStop("4×", 4), zoomStop("12×", 12)),
      ),
    );
  };

  // The viewer stays mounted for good, hidden while there is nothing to view, and what
  // sits over it — the stage states, the result chip, the detail callout — is rebuilt on
  // its own. Detaching and reattaching the viewer restyled and relaid out every node of
  // the drawing for a chip changing its label. `display: contents` keeps the chrome's
  // children positioned against the stage exactly as if they were its own.
  const chrome = h("div", { style: { display: "contents" } });
  stageBody.append(viewer.el, chrome);

  const renderStage = () => {
    const st = store.state;
    if (!st.source) {
      viewer.el.style.display = "none";
      fill(chrome, firstRun(store, act, samples()));
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

    if (st.tracing) {
      const last = st.liveStages[st.liveStages.length - 1];
      lead = last?.name ?? "starting";
      const elapsed = st.liveStages.reduce((a, x) => a + x.ms, 0) / 1000;
      rest = `· ${plannedTracePx(st, st.tracingTier) ?? "—"} px · ${seconds(elapsed)} elapsed · Esc to cancel`;
      colour = "var(--state-stale)";
    } else if (r && st.result) {
      const draft = st.result.tier === "draft";
      lead = draft ? "draft shown" : `traced in ${seconds(r.seconds)}`;
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
      // The update chip lives here and nowhere else: never a modal, never timed.
      st.update?.newer
        ? h(
            "span",
            { style: { display: "flex", alignItems: "center", gap: "8px", marginLeft: "12px" } },
            h("span.updatechip", null, h("i"), `${st.update.latest} available`),
            h("button.reset", { onclick: act.showUpdate }, "What changed"),
          )
        : null,
      // Fixed wording, used identically everywhere it appears.
      h("span.privacy", null, "Your image never leaves this computer."),
    );
  };

  store.on(["source", "view", "show", "zoom", "fitted", "detail", "svg", "bandsMissing"], renderTools);
  store.on(["source", "svg", "stageState", "result", "detail", "worstCorner"], renderStage);
  store.on(["tracing", "liveStages", "report", "result", "source", "update"], renderStrip);

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

  return { el, viewer };
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
      return frame("alert", "var(--bad)", "This file will not decode", state.message, ["Open another image", act.openFile]);
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

/** First run, and the empty state the app returns to. No tour, nothing to dismiss. */
function firstRun(store: Store, act: WorkspaceActions, samples: SampleInfo[]): HTMLElement {
  const mod = modKey(store.state.caps?.platform);
  return h(
    "div.firstrun",
    null,
    h(
      "div.drop",
      null,
      h("span.glyph", null, icon("image", 32)),
      h(
        "div",
        { style: { display: "flex", flexDirection: "column", gap: "6px" } },
        h("span.headline.resting", null, "Drop a PNG, JPEG or WebP"),
        h("span.headline.release", null, "Release to trace"),
        h(
          "span.faint.resting",
          { style: { fontSize: "13px" } },
          "or press ",
          h("kbd", null, `${mod}+O`),
          " to choose a file",
        ),
      ),
      h("button.btn.primary.resting", { onclick: act.openFile }, "Open an image"),
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
