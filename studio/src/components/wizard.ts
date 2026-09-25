/**
 * The vectorization wizard: Auto or Custom, for the image just opened.
 *
 * Two rules shape all of it. **Auto is the automatic trace, unchanged**: it starts the
 * moment an image is opened, before anybody has chosen anything, so choosing Auto costs no
 * waiting and Custom starts from a finished result. **Custom is ordinary settings**: every
 * step moves controls the Tune tab has, through the same path the rail uses, so finishing
 * lands in the normal workspace with the choices shown as changed and nothing hidden.
 *
 * Two pieces live here. The *chooser* is a small card over the stage — not a modal, and
 * ignoring it is Auto. The *wizard* is a rail takeover, like the export sheet, so the big
 * view stays beside the steps; its left pane compares the current trace with Auto's (or
 * with a preview) through `State.compare`, never by replacing the drawing.
 */

import { fill, h, icon } from "../lib/dom";
import type { ColourGroup, Control, OnOpen, PresetId, Report, Settings } from "../lib/ipc";
import { bytes, count, de00, seconds, type Store, type WizardStep } from "../lib/state";
import { previewKey, type Previews } from "../lib/previews";
import { changedControls, describeAuto, looksDamaged, NOISY_LEVELS } from "../lib/suggest";
import { autoChose, suggestionsOf, type AutoChoseActions } from "./autochose";
import { paletteCard, wirePaletteHover } from "./palette";
import { controlRow, type RailActions } from "./rail";
import { toast } from "./overlays";

const DOCS = "https://logolabs.github.io/inkvec/";

export interface WizardActions extends RailActions, AutoChoseActions {
  /** Put the controls and colour groups back to a snapshot; a new trace follows. */
  restore(snapshot: Snapshot): void;
  /** Remember what opening an image should offer. */
  setOnOpen(choice: OnOpen): void;
  /** The first-run introduction has been seen. */
  markSeen(): void;
}

/** The controls and colour groups at one moment, for "Undo my changes". */
export interface Snapshot {
  settings: Settings;
  preset: string | null;
  colourGroups: ColourGroup[];
}

// ------------------------------------------------------------------ chooser ---

/**
 * The card over the stage that offers Auto or Custom for the image just opened.
 *
 * Rebuilt only when something it shows changes shape; a running trace's stages are written
 * into it in place, because a button replaced between the press and the release never
 * hears the click.
 */
export function createChooser(store: Store, act: WizardActions, openWizard: () => void): HTMLElement {
  const el = h("div.chooser", { role: "region", "aria-label": "How to trace this image" });
  let remember = false;

  const dismiss = (choice: "auto" | "custom") => {
    const st = store.state;
    if (remember) act.setOnOpen(choice);
    if (!st.prefs?.seenFirstRun) act.markSeen();
    store.set({ chooser: false });
    if (choice === "custom") openWizard();
  };

  const status = () => {
    const st = store.state;
    if (st.tracing && st.generation === st.auto?.generation) {
      const last = st.liveStages[st.liveStages.length - 1];
      const elapsed = st.liveStages.reduce((a, x) => a + x.ms, 0) / 1000;
      return `Tracing now · ${last?.name ?? "starting"} · ${seconds(elapsed)}`;
    }
    const r = st.auto?.report;
    if (r) return `Done · ${de00(r.meanDe00)} dE00 · ${count(r.coordinates)} coordinates · ${bytes(r.bytes)}`;
    if (st.stageState.kind !== "drawing" && st.stageState.kind !== "decoding") return "Stopped; the stage says why";
    return "Starting…";
  };

  const render = () => {
    const st = store.state;
    const show = st.chooser && Boolean(st.source) && st.tab === "vectorize" && !st.wizard && !st.screen;
    el.hidden = !show;
    if (!show) {
      fill(el);
      return;
    }
    const first = !st.prefs?.seenFirstRun;
    const news = autoChose(store, act);
    fill(
      el,
      h(
        "div.choosehead",
        null,
        h("span.eyebrow", null, `Just opened · ${st.source?.name ?? ""}`),
        h(
          "button.btn.icon.compact.ghost",
          { "aria-label": "Close; Auto stands", title: "Close. Auto stands (Esc)", onclick: () => dismiss("auto") },
          icon("x", 14),
        ),
      ),
      first
        ? h(
            "p.chooseintro",
            null,
            "New: every image is traced the moment it opens. Keep Auto, or customise it a step at a time with the result beside each choice. Ignore this card and Auto stands.",
          )
        : null,
      h(
        "div.choices",
        null,
        h(
          "div.choice",
          null,
          h("span.choicename", null, icon("zap", 14), "Auto"),
          h("span.choicebody.num", { "data-status": "" }, status()),
          h("button.btn.primary.compact", { "data-ctl": "chooser-auto", onclick: () => dismiss("auto") }, "Keep Auto"),
        ),
        h(
          "div.choice",
          null,
          h("span.choicename", null, icon("settings", 14), "Custom"),
          h("span.choicebody", null, "What it is, colours, detail, shape, output: each explained, beside Auto's result."),
          h("button.btn.compact", { "data-ctl": "chooser-custom", onclick: () => dismiss("custom") }, "Customise…"),
        ),
      ),
      news ? h("div.choosenews", null, h("span.eyebrow", null, "Auto chose"), news) : null,
      h(
        "label.remember",
        null,
        h("input", {
          type: "checkbox",
          checked: remember,
          onchange: (e: Event) => {
            remember = (e.target as HTMLInputElement).checked;
          },
        }),
        h("span", null, "Remember my choice"),
        h("span.muted", null, "· change it in Settings"),
      ),
    );
  };

  const tick = () => {
    const node = el.querySelector<HTMLElement>("[data-status]");
    if (node) node.textContent = status();
  };

  store.on(
    ["chooser", "source", "tab", "wizard", "screen", "auto", "facts", "prefs", "preset", "settings", "groupSuggestions", "palette", "stageState"],
    render,
  );
  store.on(["tracing", "liveStages", "generation"], tick);
  render();
  return el;
}

// ------------------------------------------------------------------- steps ---

interface StepInfo {
  id: WizardStep;
  title: string;
  /** What the step does, when to change it and what it costs, in plain words. */
  does: string;
  when: string;
  costs: string;
  learn: [string, string];
}

const STEPS: Record<WizardStep, StepInfo> = {
  kind: {
    id: "kind",
    title: "What is it?",
    does: "Picks the preset that suits the kind of image: a handful of settings chosen together. Each tile is a small draft of this image under that preset.",
    when: "When Auto's trace is not the kind of drawing you wanted: strokes that should be lines, a scan that should be two inks.",
    costs: "Nothing on its own. Photo or scan adds the denoiser to the full trace, about 0.6 s.",
    learn: ["How a trace is made", "algorithm/plain/00-overview.html"],
  },
  cleanup: {
    id: "cleanup",
    title: "Clean-up",
    does: "Repairs JPEG and screenshot damage before tracing, so the colours are the ones the artwork was drawn with rather than the ones the compression left.",
    when: "The source is a JPEG, a screenshot or a photo of a mark, and its flat colours come back speckled or with extra shades.",
    costs: "About 0.6 s per full trace, and a one-time download. Drafts skip it.",
    learn: ["Intake and noise", "algorithm/plain/01-intake.html"],
  },
  colours: {
    id: "colours",
    title: "Colours",
    does: "Shows every ink the trace found. Suggested groups are inks that are the same colour measured twice; grouping re-traces so the shapes between them join.",
    when: "The palette has near-duplicates, or more colours than the design has.",
    costs: "Fewer colours can merge a detail into its neighbour; the colour difference says when.",
    learn: ["How the palette is found", "algorithm/plain/03-palette.html"],
  },
  detail: {
    id: "detail",
    title: "Detail",
    does: "How closely curves follow the pixels, and how small a speck can be before it is dropped.",
    when: "Tighter for crests and filigree; looser for a smaller file with fewer nodes.",
    costs: "Tighter precision means more nodes and a bigger file; the readout above shows the trade.",
    learn: ["Curve fitting", "algorithm/plain/11-fitting.html"],
  },
  shape: {
    id: "shape",
    title: "Shape",
    does: "How the outlines are built: nodes an artist would place, fewer paths, one shape reused where it repeats, and what a curve or a corner costs the fit.",
    when: "The SVG will be edited by hand, or has to be as small as possible.",
    costs: "Editable nodes cost a little colour accuracy (about 0.04 dE00 on icons); the readout shows it here.",
    learn: ["Symmetry and repeated shapes", "algorithm/plain/10-symmetry.html"],
  },
  output: {
    id: "output",
    title: "Output",
    does: "How the file is written: the background, a margin around the drawing, and whether it is minified.",
    when: "Placing the SVG on a coloured page, or shipping it on the web.",
    costs: "None of these re-trace: they rewrite the same drawing.",
    learn: ["Writing the SVG", "algorithm/plain/13-emit.html"],
  },
};

/** The controls each step shows, by key. */
const STEP_CONTROLS: Partial<Record<WizardStep, (keyof Settings)[]>> = {
  detail: ["precision", "speckleFloor"],
  shape: ["editability", "fewerPaths", "matchRepeatedShapes", "bezierCost", "cornerAngle"],
  output: ["transparentBackground", "margin", "minify"],
};

/** The "What is it?" tiles, besides Auto's own. */
const KINDS: { preset: PresetId; name: string; why: string; note?: string }[] = [
  { preset: "logo", name: "Logo", why: "Flat colours and crisp shapes. The default." },
  { preset: "icon", name: "Icon", why: "A small flat mark: keeps one-pixel details, at most 16 colours." },
  { preset: "fine-detail", name: "Illustration or emoji", why: "Gradients and fine detail: curves fitted twice as closely." },
  { preset: "line-art", name: "Line art", why: "Even-width lines come back as strokes you can re-weight." },
  { preset: "black-and-white", name: "Black & white", why: "Two inks: stamps, signatures, scans of print." },
  {
    preset: "photo-or-scan",
    name: "Photo or scan",
    why: "JPEGs and screenshots: the denoiser cleans compression damage first.",
    note: "The draft is shown without the denoiser, which runs on the full trace.",
  },
];

/** The steps that apply to the open image. */
export function stepsFor(store: Store): WizardStep[] {
  const st = store.state;
  const cleanup = Boolean(st.caps?.denoiser.supported) && looksDamaged(st.source, st.facts);
  return (["kind", "cleanup", "colours", "detail", "shape", "output"] as WizardStep[]).filter(
    (s) => s !== "cleanup" || cleanup,
  );
}

// ------------------------------------------------------------------ wizard ---

export interface Wizard {
  /** Open on the first step, over `host` (the rail). */
  open(): void;
  /** Close, keeping every choice made. `toastIt` says what was applied. */
  close(toastIt?: boolean): void;
  isOpen(): boolean;
}

export function createWizard(store: Store, host: HTMLElement, act: WizardActions, previews: Previews): Wizard {
  let sheet: HTMLElement | null = null;
  let stops: (() => void)[] = [];
  let snapshot: Snapshot | null = null;
  let viewBefore = store.state.view;
  /** What the left pane compares with when nothing is hovered. */
  let left: "auto" | "source" = "auto";
  let autoCompare: { svg: string; label: string } | null = null;
  let autoUrl: { svg: string; url: string } | null = null;

  // The sheet's parts. The frame (title, dots, nav) is rebuilt per step; `controls` when a
  // setting moves; `live` when a trace lands. They are separate so a slider is never
  // replaced under the pointer by a trace finishing.
  const head = h("div.wizhead");
  const intro = h("div.wizintro");
  const controls = h("div.wizcontrols");
  const live = h("div.wizlive");
  const guide = h("div.wizguide");
  const scroll = h("div.wizscroll", null, intro, controls, live, guide);
  const nav = h("div.wiznav");
  wirePaletteHover(live, store);

  const steps = () => stepsFor(store);
  const step = (): WizardStep => store.state.wizard?.step ?? "kind";

  const go = (next: WizardStep) => {
    if (next === step()) return;
    if (step() === "kind") previews.pause();
    store.set({ wizard: { step: next }, hoverFill: null });
    scroll.scrollTop = 0;
  };

  const leftCompare = (): { svg: string; label: string } | null => {
    const svg = store.state.auto?.svg;
    if (left !== "auto" || !svg) return null;
    if (autoCompare?.svg !== svg) autoCompare = { svg, label: "Auto" };
    return autoCompare;
  };
  const restoreCompare = () => store.set({ compare: leftCompare() });

  const autoThumb = (): string | null => {
    const svg = store.state.auto?.svg;
    if (!svg) return null;
    if (autoUrl?.svg !== svg) {
      if (autoUrl) URL.revokeObjectURL(autoUrl.url);
      autoUrl = { svg, url: URL.createObjectURL(new Blob([svg], { type: "image/svg+xml" })) };
    }
    return autoUrl.url;
  };

  // ---------------------------------------------------------------- frame ---

  const renderHead = () => {
    const list = steps();
    const at = Math.max(0, list.indexOf(step()));
    const info = STEPS[step()];
    const moved = snapshot !== null && (
      store.state.preset !== snapshot.preset ||
      changedControls(store.state.caps, store.state.settings, snapshot.settings).length > 0 ||
      JSON.stringify(store.state.colourGroups) !== JSON.stringify(snapshot.colourGroups)
    );
    fill(
      head,
      h(
        "div.cardhead",
        null,
        h("span.eyebrow", null, `Custom · step ${at + 1} of ${list.length}`),
        h(
          "div",
          { style: { display: "flex", gap: "10px", alignItems: "baseline" } },
          moved
            ? h(
                "button.reset",
                {
                  "data-ctl": "wiz-undo",
                  title: "Put every control and colour group back where they were when the wizard opened",
                  onclick: () => snapshot && act.restore(snapshot),
                },
                "Undo my changes",
              )
            : null,
          h("button.reset", { "data-ctl": "wiz-close", title: "Keep what is chosen so far (Esc)", onclick: () => close(true) }, "Close"),
        ),
      ),
      h("h2.wiztitle.serif", null, info.title),
      h(
        "div.wizdots",
        { role: "tablist", "aria-label": "Steps" },
        ...list.map((id, i) =>
          h(
            "button",
            {
              role: "tab",
              "aria-selected": String(id === step()),
              "aria-label": `Step ${i + 1}: ${STEPS[id].title}`,
              title: STEPS[id].title,
              class: i < at ? "done" : undefined,
              onclick: () => go(id),
            },
          ),
        ),
      ),
      h(
        "div.wizcompare",
        null,
        h("span.muted", null, "Left pane"),
        h(
          "div.seg",
          { role: "group", "aria-label": "What the left pane shows" },
          h(
            "button",
            {
              "aria-pressed": String(left === "auto" && Boolean(store.state.auto?.svg)),
              disabled: !store.state.auto?.svg,
              title: store.state.auto?.svg ? "Auto's trace, to compare yours with" : "Auto's trace has not landed yet",
              onclick: () => {
                left = "auto";
                restoreCompare();
                renderHead();
              },
            },
            "Auto's trace",
          ),
          h(
            "button",
            {
              "aria-pressed": String(left === "source" || !store.state.auto?.svg),
              onclick: () => {
                left = "source";
                restoreCompare();
                renderHead();
              },
            },
            "Source",
          ),
        ),
      ),
    );
  };

  const renderIntro = () => {
    const info = STEPS[step()];
    // What the step does leads; when to change it and what it costs follow what it
    // changes, so the choices themselves are the first thing under the title.
    fill(intro, h("p.wizlead", null, info.does));
    fill(
      guide,
      h(
        "dl.wizexplain",
        null,
        h("dt", null, "Change it when"),
        h("dd", null, info.when),
        h("dt", null, "What it costs"),
        h("dd", null, info.costs),
      ),
      h("a.wizlearn", { href: DOCS + info.learn[1] }, `Learn more: ${info.learn[0]} →`),
    );
  };

  const renderNav = () => {
    const list = steps();
    const at = list.indexOf(step());
    const last = at === list.length - 1;
    fill(
      nav,
      h("button.btn", { "data-ctl": "wiz-back", disabled: at <= 0, onclick: () => go(list[at - 1]) }, "Back"),
      last
        ? null
        : h("button.reset", { "data-ctl": "wiz-skip", title: "Keep what is chosen so far and go to the last step", onclick: () => go(list[list.length - 1]) }, "Skip to finish"),
      h("span", { style: { flex: "1" } }),
      last
        ? h("button.btn", { "data-ctl": "wiz-export", disabled: !store.state.svg, onclick: () => close(true, true) }, "Export now")
        : null,
      last
        ? h("button.btn.primary", { "data-ctl": "wiz-finish", onclick: () => close(true) }, "Finish")
        : h("button.btn.primary", { "data-ctl": "wiz-next", onclick: () => go(list[at + 1]) }, "Next"),
    );
  };

  const renderFrame = () => {
    if (!sheet) return;
    // A step that stopped applying (the facts arrived and the image is clean) is left.
    const list = steps();
    if (!list.includes(step())) {
      store.set({ wizard: { step: list[Math.min(list.length - 1, 1)] } });
      return;
    }
    renderHead();
    renderIntro();
    renderNav();
    renderControls();
    renderLive();
  };

  // ------------------------------------------------------------- controls ---

  const controlByKey = (key: keyof Settings): Control | undefined => store.state.caps?.controls.find((c) => c.key === key);

  const rows = (keys: (keyof Settings)[]) => {
    const base = act.baseSettings();
    return keys
      .map((k) => controlByKey(k))
      .filter((c): c is Control => Boolean(c))
      .map((c) => controlRow(store, c, act, base));
  };

  const renderControls = () => {
    const focused =
      document.activeElement instanceof HTMLElement && controls.contains(document.activeElement)
        ? document.activeElement.getAttribute("data-ctl")
        : null;
    const st = store.state;
    const s = step();
    let body: (HTMLElement | null)[] = [];
    if (s === "cleanup") body = cleanupControls();
    else if (s === "colours") body = rows(st.facts?.hasAlpha ? ["maxColours", "traceTransparency"] : ["maxColours"]);
    else if (s === "detail") body = [viewSwitch(), ...rows(STEP_CONTROLS.detail ?? [])];
    else if (s === "shape" || s === "output") body = rows(STEP_CONTROLS[s] ?? []);
    fill(controls, ...body);
    controls.hidden = body.length === 0;
    if (focused) controls.querySelector<HTMLElement>(`[data-ctl="${focused}"]`)?.focus({ preventScroll: true });
    if (sheet) renderHead();
  };

  const viewSwitch = () =>
    h(
      "div.wizrow",
      null,
      h("span.label", null, "Compare as"),
      h(
        "div.seg",
        { role: "group", "aria-label": "How the two panes are arranged" },
        ...(["side", "wipe"] as const).map((v) =>
          h(
            "button",
            { "aria-pressed": String(store.state.view === v), "data-ctl": `wiz-view:${v}`, onclick: () => store.set({ view: v }) },
            v === "side" ? "Side by side" : "Wipe",
          ),
        ),
      ),
    );

  const cleanupControls = (): HTMLElement[] => {
    const st = store.state;
    const den = st.caps?.denoiser;
    const mode = st.settings.cleanUpDamage;
    const evidence = [
      st.source?.lossy ? `the file is a ${st.source.container}, which stores colour approximately` : null,
      st.facts && st.facts.noiseLevels >= NOISY_LEVELS
        ? `its pixels measure ${st.facts.noiseLevels.toFixed(1)} levels of noise (a clean file reads about 0.5)`
        : null,
    ].filter(Boolean);
    const choice = (id: "off" | "auto", name: string, body: string) =>
      h(
        "button.wizchoice",
        { "aria-pressed": String(mode === id || (id === "auto" && mode === "on")), "data-ctl": `wiz-cleanup:${id}`, onclick: () => act.changeSetting("cleanUpDamage", id) },
        h("span.name", null, name),
        h("span.muted", null, body),
      );
    return [
      evidence.length ? h("p.wizwhy", null, `Offered because ${evidence.join(", and ")}.`) : null,
      h(
        "div.wizchoices",
        null,
        choice("off", "Off", "Trace the pixels as they are, damage and all."),
        choice("auto", "Auto", "Clean the damage first, where it is found."),
      ),
      den && !den.installed && mode !== "off"
        ? h(
            "div.wiznote",
            null,
            h(
              "span",
              null,
              `The denoiser is not downloaded yet. It is a one-time download from ${den.repo}, checked against its SHA-256 on arrival, and it runs on this computer: the model comes down, your image never goes up. Until then the trace runs without it.`,
            ),
            h("button.btn.compact", { "data-ctl": "wiz-download", onclick: act.openDenoiser }, "Download the denoiser…"),
          )
        : null,
      h(
        "p.muted.wizsmall",
        null,
        "Left: Auto's trace, without it. Right: yours. It runs on the full trace only, so the right pane changes when the full trace lands, a moment after the draft.",
      ),
    ].filter((x): x is HTMLElement => x !== null);
  };

  // ----------------------------------------------------------------- live ---

  let tiles: HTMLElement | null = null;

  const renderLive = () => {
    const s = step();
    if (s === "kind") {
      renderKinds();
      return;
    }
    tiles = null;
    const st = store.state;
    if (s === "colours") {
      fill(
        live,
        st.palette.length ? paletteCard(store, act) : h("p.muted.wizsmall", null, "The palette appears when a trace lands."),
        h("p.muted.wizsmall", null, "Hover an ink or a group to single it out in the drawing on the right."),
      );
    } else if (s === "detail" || s === "shape") {
      fill(live, readout());
    } else if (s === "output") {
      fill(live, changesSummary(), readout());
    } else {
      fill(live);
    }
  };

  /** Auto's figures beside the current ones: what the step's controls bought or cost. */
  const readout = (): HTMLElement => {
    const st = store.state;
    const a = st.auto?.report ?? null;
    const r = st.report;
    const draft = st.result?.tier === "draft";
    const nodes = (x: Report | null) => (x ? (x.structure?.nodes || Math.round(x.coordinates / 2)) : null);
    const line = (label: string, av: number | null, nv: number | null, fmt: (n: number) => string, lowerBetter = true, eps = 0) => {
      const d = av !== null && nv !== null && !draft && !st.tracing ? nv - av : null;
      const cls = d === null || Math.abs(d) <= eps ? "same" : (d < 0) === lowerBetter ? "better" : "worse";
      return h(
        "tr",
        null,
        h("th", null, label),
        h("td.num", null, av === null ? "—" : fmt(av)),
        h("td.num", null, nv === null ? "—" : fmt(nv)),
        h(`td.num.delta.${cls}`, null, d === null ? "" : Math.abs(d) <= eps ? "same" : `${d < 0 ? "−" : "+"}${fmt(Math.abs(d))}`),
      );
    };
    return h(
      "div.card.wizreadout",
      { "aria-live": "polite" },
      h(
        "div.cardhead",
        null,
        h("span.eyebrow", null, "Auto · yours"),
        h("span.muted", { style: { fontSize: "11px" } }, st.tracing ? "tracing…" : draft ? "draft; the full trace follows" : "full traces"),
      ),
      h(
        "table",
        null,
        h("thead", null, h("tr", null, h("th"), h("th", null, "Auto"), h("th", null, "Yours"), h("th"))),
        h(
          "tbody",
          null,
          line("colour difference", a?.meanDe00 ?? null, r?.meanDe00 ?? null, (n) => n.toFixed(2), true, 0.005),
          line("nodes", nodes(a), nodes(r), count),
          line("paths", a?.paths ?? null, r?.paths ?? null, count),
          line("file size", a?.bytes ?? null, r?.bytes ?? null, bytes, true, 16),
        ),
      ),
      h("span.muted.wizsmall", null, "Colour difference in dE00: under 1 is invisible side by side."),
    );
  };

  /** Everything that differs from Auto, as the Tune tab will show it. */
  const changesSummary = (): HTMLElement => {
    const st = store.state;
    const auto = st.auto;
    const keys = auto ? changedControls(st.caps, auto.settings, st.settings) : [];
    const show = (c: Control, v: Settings[keyof Settings]) =>
      typeof v === "boolean" ? (v ? "on" : "off") : typeof v === "number" ? `${c.decimals ? v.toFixed(c.decimals) : Math.round(v)}${c.unit ? ` ${c.unit}` : ""}` : String(v);
    const preset = st.caps?.presets.find((p) => p.id === st.preset);
    return h(
      "div.card.wizchanges",
      null,
      h("span.eyebrow", null, "What you changed from Auto"),
      preset && auto && preset.id !== auto.preset ? h("div.wizchange", null, h("span", null, "Preset"), h("span.num", null, preset.name)) : null,
      ...keys.map((k) => {
        const c = controlByKey(k);
        if (!c || !auto) return null;
        return h("div.wizchange", null, h("span", null, c.label), h("span.num", null, `${show(c, auto.settings[k])} → ${show(c, st.settings[k])}`));
      }),
      st.colourGroups.length
        ? h("div.wizchange", null, h("span", null, "Colour groups"), h("span.num", null, String(st.colourGroups.length)))
        : null,
      !keys.length && !st.colourGroups.length && !(preset && auto && preset.id !== auto.preset)
        ? h("span.muted", null, "Nothing yet: this is Auto's trace.")
        : h("span.muted.wizsmall", null, "Finish keeps these as ordinary settings, under Tune."),
    );
  };

  // ------------------------------------------------------------ the tiles ---

  interface Tile {
    id: string;
    name: string;
    why: string;
    note?: string;
    settings: Settings | null;
    preset: PresetId | null;
    suggested: boolean;
  }

  const kindTiles = (): Tile[] => {
    const st = store.state;
    const auto = st.auto;
    const caps = st.caps;
    const suggested = new Set(suggestionsOf(store).map((s) => s.preset));
    const groups = st.colourGroups.length ? { colourGroups: st.colourGroups } : {};
    const autoKey = auto ? previewKey({ ...auto.settings, ...groups }) : null;
    const out: Tile[] = [];
    let autoIs: string | null = null;
    for (const k of KINDS) {
      const p = caps?.presets.find((x) => x.id === k.preset);
      if (!p) continue;
      const settings = { ...p.settings, ...groups };
      if (autoKey && previewKey(settings) === autoKey && k.preset !== "photo-or-scan" && autoIs === null) {
        autoIs = k.name;
        continue;
      }
      out.push({ id: k.preset, name: k.name, why: k.why, note: k.note, settings, preset: k.preset, suggested: suggested.has(k.preset) });
    }
    const autoTile: Tile = {
      id: "auto",
      name: autoIs ? `${autoIs} · Auto` : "As Auto traced",
      why: auto ? `Auto used ${describeAuto(caps, auto.settings, auto.preset)}.` : "Auto's trace.",
      settings: null,
      preset: null,
      suggested: false,
    };
    // What Auto noticed goes first after Auto, so its preview is the first to land.
    return [autoTile, ...out.filter((t) => t.suggested), ...out.filter((t) => !t.suggested)];
  };

  const isChosen = (t: Tile): boolean => {
    const st = store.state;
    if (t.id === "auto") {
      const auto = st.auto;
      return Boolean(auto) && st.preset === auto?.preset && changedControls(st.caps, st.settings, auto!.settings).length === 0;
    }
    return st.preset === t.preset;
  };

  const choose = (t: Tile) => {
    if (t.id === "auto") act.backToAuto();
    else if (t.preset) act.setPreset(t.preset);
  };

  /** Point the left pane at a tile's preview while it is hovered or focused. */
  const peek = (t: Tile | null) => {
    if (!t) {
      restoreCompare();
      return;
    }
    const st = store.state;
    if (t.id === "auto" || sameAsAuto(t)) {
      if (st.auto?.svg) store.set({ compare: leftCompare() ?? { svg: st.auto.svg, label: "Auto" } });
      return;
    }
    const p = t.settings ? previews.get(previewKey(t.settings)) : undefined;
    if (p?.state === "done") store.set({ compare: { svg: p.svg, label: `Preview · ${t.name} (draft)` } });
  };

  /** A tile whose draft would be Auto's own settings shows Auto's finished trace instead. */
  const sameAsAuto = (t: Tile): boolean => {
    const auto = store.state.auto;
    if (!auto || !t.settings) return false;
    const groups = store.state.colourGroups.length ? { colourGroups: store.state.colourGroups } : {};
    return previewKey(t.settings) === previewKey({ ...auto.settings, ...groups });
  };

  const tileStatus = (t: Tile): { url: string | null; text: string; busy: boolean } => {
    const st = store.state;
    if (t.id === "auto" || sameAsAuto(t)) {
      const r = st.auto?.report;
      return r
        ? { url: autoThumb(), text: `${de00(r.meanDe00)} dE00 · ${count(r.paths)} paths · ${bytes(r.bytes)}`, busy: false }
        : { url: null, text: "tracing…", busy: true };
    }
    const p = t.settings ? previews.get(previewKey(t.settings)) : undefined;
    if (!p || p.state === "queued") return { url: null, text: st.auto?.report ? "queued" : "after Auto", busy: true };
    if (p.state === "running") return { url: null, text: "tracing a draft…", busy: true };
    if (p.state === "failed") return { url: null, text: "no preview", busy: false };
    return { url: p.url, text: `${de00(p.report.meanDe00)} dE00 · ${count(p.report.paths)} paths · ${bytes(p.report.bytes)}`, busy: false };
  };

  /** Build the tile grid, or bring the one on screen up to date in place. */
  const renderKinds = () => {
    const list = kindTiles();
    const signature = list.map((t) => t.id).join(",");
    if (!tiles || tiles.dataset.signature !== signature) {
      tiles = h("div.kinds", { "data-signature": signature, role: "radiogroup", "aria-label": "What is it?" });
      for (const t of list) {
        const tile = h(
          "button.kind",
          {
            role: "radio",
            "data-key": t.id,
            "data-ctl": `wiz-kind:${t.id}`,
            onclick: () => choose(t),
            onpointerenter: () => peek(t),
            onpointerleave: () => peek(null),
            onfocus: () => peek(t),
            onblur: () => peek(null),
          },
          h("span.kthumb", null, h("img", { alt: "", draggable: "false" }), h("span.kspin")),
          h("span.kname", null, t.name, t.suggested ? h("span.ksugg", null, "suggested") : null),
          h("span.kwhy", null, t.why),
          t.note ? h("span.knote", null, t.note) : null,
          h("span.kstat.num"),
        );
        tiles.append(tile);
      }
      fill(
        live,
        tiles,
        h("p.muted.wizsmall.kqueue"),
      );
    }
    // Ask for what is missing: drafts of each tile, one at a time, behind the main trace.
    // Not before Auto has landed, so they never delay the trace somebody is waiting for.
    if (store.state.auto?.report) {
      previews.want(
        list.filter((t) => t.settings && !sameAsAuto(t)).map((t) => ({ key: previewKey(t.settings!), settings: t.settings! })),
      );
    }
    for (const t of list) {
      const tile = tiles.querySelector<HTMLElement>(`[data-key="${t.id}"]`);
      if (!tile) continue;
      const s = tileStatus(t);
      const img = tile.querySelector("img") as HTMLImageElement;
      if (s.url && img.getAttribute("src") !== s.url) img.src = s.url;
      img.hidden = !s.url;
      tile.classList.toggle("busy", s.busy);
      tile.setAttribute("aria-checked", String(isChosen(t)));
      (tile.querySelector(".kstat") as HTMLElement).textContent = s.text;
    }
    const pending = previews.pending();
    const q = live.querySelector<HTMLElement>(".kqueue");
    if (q) {
      q.textContent = pending
        ? `Previews are small drafts of this image, traced one at a time behind the main trace · ${pending} to go.`
        : "Previews are small drafts of this image. Hover one to see it large on the left; choose one and the full trace follows on the right.";
    }
  };

  // ------------------------------------------------------------ open/close ---

  function open(): void {
    if (sheet) return;
    const st = store.state;
    snapshot = { settings: { ...st.settings }, preset: st.preset, colourGroups: [...st.colourGroups] };
    viewBefore = st.view;
    left = "auto";
    tiles = null;
    const body = h("div.sheetbody.wizbody", null, head, scroll, nav);
    sheet = h("div.sheet.wizsheet", null, body);
    host.classList.add("wizarding");
    host.append(sheet);
    store.set({
      wizard: { step: steps()[0] },
      chooser: false,
      view: st.view === "ab" ? "side" : st.view,
      compare: leftCompare(),
    });
    if (!st.prefs?.seenFirstRun) act.markSeen();
    stops = [
      store.on(["wizard", "facts", "caps", "prefs"], renderFrame),
      store.on(["settings", "preset", "colourGroups", "view"], () => {
        renderControls();
        if (step() === "kind" || step() === "output") renderLive();
      }),
      store.on(
        ["report", "result", "tracing", "palette", "groupSuggestions", "resultGroups", "paletteSelection", "simplifyTo", "losses", "auto", "svg"],
        () => {
          renderLive();
          if (store.state.compare === null || store.state.compare === autoCompare || store.state.compare?.label === "Auto") restoreCompare();
          renderNav();
        },
      ),
      previews.on(() => {
        if (step() === "kind") renderLive();
      }),
      // Auto's trace landing enables "Auto's trace" as the left pane.
      store.on(["auto"], renderHead),
    ];
    renderFrame();
    (nav.querySelector<HTMLElement>("[data-ctl=wiz-next]") ?? body).focus?.();
  }

  function close(toastIt = false, thenExport = false): void {
    if (!sheet) return;
    for (const stop of stops) stop();
    stops = [];
    previews.pause();
    sheet.remove();
    sheet = null;
    tiles = null;
    host.classList.remove("wizarding");
    if (autoUrl) URL.revokeObjectURL(autoUrl.url);
    autoUrl = null;
    autoCompare = null;
    const st = store.state;
    const moved =
      snapshot !== null &&
      (st.preset !== snapshot.preset ||
        changedControls(st.caps, st.settings, snapshot.settings).length > 0 ||
        JSON.stringify(st.colourGroups) !== JSON.stringify(snapshot.colourGroups));
    const changed = st.auto ? changedControls(st.caps, st.auto.settings, st.settings).length : 0;
    store.set({ wizard: null, compare: null, hoverFill: null, view: viewBefore, railTab: moved ? "tune" : st.railTab });
    snapshot = null;
    if (toastIt && moved) {
      toast(
        changed
          ? `Custom settings kept: ${changed} control${changed === 1 ? "" : "s"} changed from Auto, all of them under Tune.`
          : "Custom settings kept. Everything is under Tune.",
        { kind: "good" },
      );
    }
    if (thenExport) act.openExport();
  }

  return { open, close, isOpen: () => sheet !== null };
}
