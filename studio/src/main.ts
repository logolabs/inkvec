/**
 * Inkvec Studio Lite — the app shell.
 *
 * Owns the window chrome, the tab switcher, the keyboard, and the trace loop that ties a
 * moving control to a draft and a settled one to a full trace.
 *
 * The trace loop is the part worth reading. Every change to a control starts a draft at
 * once, so the viewer keeps up with the slider, and arms a timer; when the controls have
 * been still for `settleMs` the full-resolution trace is queued and swapped in with a
 * visible crossfade. Draft and final are two states of one result, never a silent
 * substitution.
 */

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";

import { appMark, fill, h } from "./lib/dom";
import {
  api,
  events,
  type ColourGroup,
  type Outcome,
  type Prefs,
  type SampleInfo,
  type Settings,
} from "./lib/ipc";
import { initial, modKey, Store } from "./lib/state";
import { Previews } from "./lib/previews";
import { createRail, type RailActions } from "./components/rail";
import { createChooser, createWizard, type Snapshot, type WizardActions } from "./components/wizard";
import { proposeGroups } from "./components/palette";
import { openCardComposer } from "./components/card";
import { openExportSheet } from "./components/exportsheet";
import { helpPageFor, installHelp, openHelp } from "./components/help";
import { closeOverlay, openPopover, toast } from "./components/overlays";
import { windowControls } from "./components/wincontrols";
import { createBatch } from "./views/batch";
import { createMinify } from "./views/minify";
import { createFabricate } from "./views/fabricate";
import { createScreens, openDenoiserModal } from "./views/screens";
import { createWorkspace, jumpToWorst } from "./views/workspace";

const DEFAULT_SETTINGS: Settings = {
  precision: 0.1,
  speckleFloor: 2,
  traceSize: 2048,
  timeLimit: 0,
  maxColours: 64,
  colourMerging: 0.035,
  flatFills: false,
  blackAndWhite: false,
  cleanUpDamage: "off",
  matchRepeatedShapes: true,
  matchThreshold: 0.92,
  fewerPaths: false,
  lineArt: false,
  repairRings: true,
  editability: false,
  bezierCost: 6,
  cornerAngle: 10,
  minify: false,
  transparentBackground: false,
  margin: 0,
  holesAsCutouts: false,
  traceTransparency: true,
};

/**
 * Write one control's value into the settings object.
 *
 * The Tune tab is driven by data — a list of controls, each naming its field — so
 * the write is necessarily dynamic. This is the one place that is true, and it is written
 * out so the unsoundness is visible and contained rather than sprinkled through the
 * callers: the backend's own tests assert that every control names a real field.
 */
function assignSetting<K extends keyof Settings>(target: Settings, key: K, value: Settings[K]): void {
  target[key] = value;
}

const store = new Store(
  initial(DEFAULT_SETTINGS, { tolerancePx: 0.1, judgePx: 1024, cornerDegrees: 30, documentCleanup: true }),
);

let samples: SampleInfo[] = [];
let settleTimer = 0;
/** The generation whose stages are currently being collected. */
let watching = 0;
/** The colour groups each trace in flight was sent with, by generation. */
const groupsSent = new Map<number, ColourGroup[]>();
/** The wizard's preview drafts, queued one at a time behind the main trace. */
const previews = new Previews();

// --------------------------------------------------------------- the trace loop ---

/**
 * The settings a trace is started with: the controls, and this image's colour groups.
 *
 * The groups live apart from the controls (`State.colourGroups`) so that nothing which
 * copies the controls — presets, saved presets, the preferences file, a batch — can carry
 * them to another image. With none, the settings go exactly as they always did.
 */
function traceSettings(): Settings {
  const sent: Settings = { ...store.state.settings };
  delete sent.colourGroups;
  if (store.state.colourGroups.length) sent.colourGroups = store.state.colourGroups;
  return sent;
}

/** Start a trace. A draft keeps up with a moving control; a final is what gets exported. */
async function trace(tier: "draft" | "final"): Promise<void> {
  if (!store.state.source) return;
  try {
    const groups = store.state.colourGroups;
    const generation = await api.startTrace(traceSettings(), tier);
    groupsSent.set(generation, groups);
    for (const old of groupsSent.keys()) if (old < generation - 8) groupsSent.delete(old);
    watching = generation;
    store.set({ generation, tracing: true, tracingTier: tier, liveStages: [] });
  } catch (e) {
    store.set({ tracing: false, stageState: { kind: "failed", message: String(e) } });
  }
}

/** A control moved: draft now, full trace once the controls have been still. */
function controlChanged(): void {
  window.clearTimeout(settleTimer);
  if (!store.state.source) return;
  void trace("draft");
  settleTimer = window.setTimeout(() => void trace("final"), store.state.prefs?.settleMs ?? 800);
}

/** Run a full trace and wait for it, which is what Export needs before it writes. */
function traceAndWait(): Promise<void> {
  return new Promise((resolve) => {
    if (!store.state.source) {
      resolve();
      return;
    }
    const stop = store.on(["result", "stageState"], () => {
      if (!store.state.tracing) {
        stop();
        resolve();
      }
    });
    window.clearTimeout(settleTimer);
    void trace("final");
  });
}

function applyOutcome(outcome: Outcome, generation = store.state.generation): void {
  if (outcome.state === "traced") {
    const wasDraft = store.state.result?.tier === "draft";
    // A draft is smaller than a final, so only a final is ever the yardstick: the readout
    // compares this full trace with the one before it, and a draft in between changes
    // nothing about what that one was.
    const before = store.state.result?.tier === "final" ? store.state.report : store.state.previous;
    store.set({
      result: outcome,
      resultGeneration: generation,
      resultGroups: groupsSent.get(generation) ?? store.state.resultGroups,
      bandsMissing: false,
      previous: outcome.tier === "final" ? before : store.state.previous,
      svg: outcome.svg,
      report: outcome.report,
      palette: outcome.palette,
      losses: outcome.losses,
      worstCorner: outcome.worstCorner,
      stageState: { kind: "drawing" },
      tracing: false,
      justSwapped: wasDraft && outcome.tier === "final",
    });
    if (store.state.justSwapped) {
      window.setTimeout(() => store.set({ justSwapped: false }), 600);
    }
    // Groups are proposed from a full trace's palette, never from a draft's, and only
    // proposed: nothing is applied until the user accepts it.
    if (outcome.tier === "final") store.set({ groupSuggestions: proposeGroups(store.state) });
    // The automatic trace an image was opened with has landed: it is what the wizard starts
    // from and compares against. Only now are the source's facts measured, from the raster
    // that trace already decoded, so they never cost the trace itself a moment.
    const auto = store.state.auto;
    if (auto && generation === auto.generation && outcome.tier === "final" && !auto.report) {
      store.set({
        auto: { ...auto, svg: outcome.svg, report: outcome.report, palette: outcome.palette, losses: outcome.losses },
      });
      api.sourceFacts(auto.settings.traceSize).then(
        (facts) => {
          if (store.state.auto?.generation === generation) store.set({ facts });
        },
        () => {},
      );
    }
    if (outcome.oversized && outcome.tier === "final") {
      toast(
        `Traced at ${outcome.tracedPx} px; the SVG is still ${outcome.sourcePx[0]} × ${outcome.sourcePx[1]} and scales without limit. This is normal.`,
      );
    }
    return;
  }

  store.set({ tracing: false });
  switch (outcome.state) {
    case "flat":
      store.set({ stageState: { kind: "flat" }, svg: null, report: null, palette: [], losses: [] });
      break;
    case "undecodable":
      store.set({ stageState: { kind: "undecodable", message: outcome.message } });
      break;
    case "outOfMemory":
      store.set({
        stageState: { kind: "outOfMemory", neededGb: outcome.neededGb, suggestPx: outcome.suggestPx },
      });
      break;
    case "failed":
      store.set({ stageState: { kind: "failed", message: outcome.message } });
      break;
  }
}

// ------------------------------------------------------------------- opening ---

/**
 * Open an image and trace it: the fork every entry point goes through.
 *
 * AUTO is the trace started here, exactly as it always was and before anything else: the
 * chooser and the wizard are offered only once it is on its way, so choosing Auto costs no
 * waiting and Custom starts from a finished result. `offer` is false for the Minify tab's
 * "rebuild", which is a re-trace rather than an image somebody opened.
 */
async function openWith(fn: () => Promise<void>, offer = true): Promise<void> {
  // A wizard still open over the last image closes, keeping its choices: they are the controls.
  wizard.close();
  store.set({
    stageState: { kind: "decoding" },
    svg: null,
    report: null,
    previous: null,
    palette: [],
    losses: [],
    result: null,
    // Colour groups and what was proposed or turned down name colours of the image that
    // was open; the next one starts without any.
    colourGroups: [],
    resultGroups: [],
    groupSuggestions: null,
    dismissedGroups: [],
    simplifyTo: null,
    paletteSelection: [],
    hoverFill: null,
    auto: null,
    facts: null,
    chooser: false,
    compare: null,
    autoNoteHidden: false,
  });
  try {
    await fn();
    store.set({ zoom: 1, pan: { x: 0, y: 0 }, stageState: { kind: "drawing" } });
    await trace("final");
    // Everything below happens after the automatic trace has been started.
    previews.reset();
    const st = store.state;
    if (st.tracing) {
      store.set({
        auto: { generation: st.generation, settings: { ...st.settings }, preset: st.preset, svg: null, report: null, palette: [], losses: [] },
      });
    }
    if (offer) offerChoice();
  } catch (e) {
    store.set({ stageState: { kind: "undecodable", message: String(e) } });
  }
}

/** What opening an image offers besides Auto: the chooser card, the wizard, or nothing. */
function offerChoice(): void {
  const on = store.state.prefs?.onOpen ?? "ask";
  if (on === "custom") wizard.open();
  else if (on === "ask") store.set({ chooser: true });
}

async function openPath(path: string): Promise<void> {
  await openWith(async () => {
    const info = await api.openPath(path);
    store.set({ source: info });
    const prefs = await api.loadPrefs();
    store.set({ prefs });
  });
}

async function chooseFile(): Promise<void> {
  const picked = await open({
    multiple: false,
    // An SVG is accepted too: it is traced from its render, which is how a messy drawing
    // comes back as clean shapes.
    filters: [{ name: "Images and SVG", extensions: ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "svg"] }],
  });
  if (typeof picked === "string") await openPath(picked);
}

/** Trace an SVG from its render, in the Vectorize tab: the Minify tab's "rebuild". */
async function retraceSvg(svg: string, name: string): Promise<void> {
  store.set({ tab: "vectorize" });
  renderTab();
  await openWith(async () => {
    const info = await api.openBytes([...new TextEncoder().encode(svg)], name);
    store.set({ source: info });
  }, false);
}
window.addEventListener("inkvec:retrace", (e) => {
  const { svg, name } = (e as CustomEvent<{ svg: string; name: string }>).detail;
  void retraceSvg(svg, name);
});

// ------------------------------------------------------------------ the shell ---

const appbar = h("header.appbar");
const content = h("div.body");
const screens = createScreens(store, {
  applyPrefs: (patch) => void savePrefs(patch),
  close: () => store.set({ screen: null }),
});

const workspace = createWorkspace(
  store,
  {
    openFile: () => void chooseFile(),
    openSample: (file) =>
      void openWith(async () => {
        const info = await api.openSample(file);
        store.set({ source: info });
      }),
    traceNow: () => void trace("final"),
    cancel: () => {
      void api.cancelTrace();
      store.set({ tracing: false, stageState: store.state.svg ? { kind: "cancelled" } : { kind: "empty" } });
    },
    retryAt: (px) => {
      store.state.settings.traceSize = px;
      store.touch("settings");
      void trace("final");
    },
    jumpToWorst: () => jumpToWorst(store, workspace.viewer),
    openDenoiser: () => openDenoiserModal(store),
    showUpdate: () => {
      const u = store.state.update;
      if (u) void openUrl(u.url);
    },
    markSeen: () => void savePrefs({ seenFirstRun: true }),
  },
  () => samples,
);

/**
 * A preset id to the settings it means, whether it is one of the built-in seven or one
 * the user saved. Returns `null` for an id that no longer exists — a saved preset can be
 * deleted while it is the selected one.
 */
function resolvePreset(id: string): { settings: Settings; wantsDenoiser: boolean } | null {
  const builtin = store.state.caps?.presets.find((p) => p.id === id);
  if (builtin) return { settings: builtin.settings, wantsDenoiser: builtin.wantsDenoiser };
  const saved = store.state.prefs?.saved.find((p) => p.id === id);
  return saved ? { settings: saved.settings, wantsDenoiser: false } : null;
}

/** Where the controls started: the selected preset's values, or the defaults if none is selected. */
function baseSettings(): Settings {
  const id = store.state.preset;
  return (id && resolvePreset(id)?.settings) || DEFAULT_SETTINGS;
}

/** Put the controls and colour groups back to a snapshot, and trace them. */
function restore(snapshot: Snapshot): void {
  store.set({ settings: { ...snapshot.settings }, preset: snapshot.preset, colourGroups: [...snapshot.colourGroups] });
  controlChanged();
}

const railActs: RailActions = {
  setPreset: (id: string) => {
    const preset = resolvePreset(id);
    if (!preset) return;
    store.set({ preset: id, settings: { ...preset.settings } });
    // The preset works without the denoiser — the colours just keep their compression
    // damage — so this explains itself on the stage and the trace carries on behind it.
    // A modal here would be one the user did not ask for.
    if (preset.wantsDenoiser && store.state.caps?.denoiser.supported && !store.state.caps.denoiser.installed) {
      store.set({ stageState: { kind: "denoiserMissing" } });
    }
    controlChanged();
  },
  // A saved preset is a whole snapshot of the controls, not a diff against the defaults:
  // it was made by somebody who had already moved exactly what they wanted moved.
  savePreset: (name: string) => {
    const prefs = store.state.prefs;
    if (!prefs) return;
    const id = `saved:${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
    const saved = [...prefs.saved, { id, name, settings: { ...store.state.settings } }];
    // Selected only once the backend has said it kept it: sanitising trims the name and
    // enforces the ceiling, so the tile that comes back is the one to point at — and if
    // the tray was full, saying so beats a confirmation for something that did not happen.
    void savePrefs({ saved }).then(() => {
      const kept = store.state.prefs?.saved.find((p) => p.id === id);
      if (kept) {
        store.set({ preset: id });
        toast(`Saved as "${kept.name}".`, { kind: "good" });
      } else {
        toast("The preset tray is full. Forget one and try again.", { kind: "bad" });
      }
    });
  },
  deletePreset: (id: string) => {
    const prefs = store.state.prefs;
    if (!prefs) return;
    void savePrefs({ saved: prefs.saved.filter((p) => p.id !== id) });
    // The controls stay exactly where they are. Deleting the tile that named them is not
    // a reason to change the picture on the stage.
    if (store.state.preset === id) store.set({ preset: null });
  },
  baseSettings,
  changeSetting: (key, value) => {
    assignSetting(store.state.settings, key, value);
    store.touch("settings");
    // Choosing the denoiser before it is installed is not an error — the trace runs without
    // it — but the stage says why nothing looks cleaner, and how to get it.
    const den = store.state.caps?.denoiser;
    if (key === "cleanUpDamage" && value !== "off" && den?.supported && !den.installed) {
      store.set({ stageState: { kind: "denoiserMissing" } });
    }
    controlChanged();
  },
  resetGroup: (group) => {
    const base = baseSettings();
    for (const c of store.state.caps?.controls ?? []) {
      if (c.group === group) assignSetting(store.state.settings, c.key, base[c.key]);
    }
    store.touch("settings");
    controlChanged();
  },
  traceNow: () => void trace("final"),
  cancel: () => {
    void api.cancelTrace();
    store.set({ tracing: false, stageState: store.state.svg ? { kind: "cancelled" } : { kind: "empty" } });
  },
  snap: (from, to) => void snap(from, to),
  setColourGroups: (groups) => {
    store.set({ colourGroups: groups, paletteSelection: [] });
    // Exactly what moving a control does: a draft now, the full trace once things settle.
    controlChanged();
  },
  openExport: () => openExportSheet(store, rail, traceAndWait),
  copySvg: async () => {
    if (!store.state.svg) return;
    await writeText(store.state.svg);
    toast("SVG copied. Paste straight into Figma or Illustrator.");
  },
  saveCard: () => openCardComposer(store),
  openDenoiser: () => openDenoiserModal(store),
  jumpToWorst: () => jumpToWorst(store, workspace.viewer),
  backToAuto: () => {
    const auto = store.state.auto;
    if (auto) restore({ settings: auto.settings, preset: auto.preset, colourGroups: store.state.colourGroups });
  },
  hideAutoNote: () => store.set({ autoNoteHidden: true }),
  openWizard: () => wizard.open(),
};
const rail = createRail(store, railActs);

const wizardActs: WizardActions = {
  ...railActs,
  restore,
  setOnOpen: (onOpen) => void savePrefs({ onOpen }),
  markSeen: () => void savePrefs({ seenFirstRun: true }),
};
const wizard = createWizard(store, rail, wizardActs, previews);
workspace.mount(createChooser(store, wizardActs, () => wizard.open()));

async function snap(from: string, to: string): Promise<void> {
  const st = store.state;
  if (!st.svg || !st.report) return;
  try {
    const result = await api.snapInks(st.svg, [{ from, to }], st.report.tracedPx, st.report.tracedPx);
    store.set({ svg: result.svg, palette: result.inks });
  } catch (e) {
    toast(String(e), { kind: "bad" });
  }
}

let minifyView: HTMLElement | null = null;
let fabView: HTMLElement | null = null;
let batchView: HTMLElement | null = null;

function renderTab(): void {
  const st = store.state;
  if (st.tab === "vectorize") {
    fill(content, workspace.el, rail);
  } else if (st.tab === "minify") {
    minifyView ??= createMinify(store);
    fill(content, minifyView);
  } else if (st.tab === "fabricate") {
    fabView ??= createFabricate(store);
    fill(content, fabView);
  } else {
    batchView ??= createBatch(store);
    fill(content, batchView);
  }
}

function renderAppBar(): void {
  const st = store.state;

  fill(
    appbar,
    h("span.brand", null, appMark(18), "Inkvec Studio Lite"),
    st.tab === "vectorize" && st.source
      ? h(
          "div.filechip",
          null,
          h("span.name", null, st.source.name),
          h("span.muted.num", null, `${st.source.width} × ${st.source.height} · ${st.source.container}`),
        )
      : null,
    h(
      "div.seg",
      { style: { marginLeft: "auto" } },
      ...(
        [
          ["vectorize", "Vectorize"],
          ["minify", "Minify SVG"],
          ["fabricate", "Fabricate"],
          ["batch", "Batch"],
        ] as const
      ).map(([id, label]) =>
        h("button", { "aria-pressed": String(st.tab === id), onclick: () => store.set({ tab: id }) }, label),
      ),
    ),
    h(
      "div",
      { style: { marginLeft: "auto", display: "flex", alignItems: "center", gap: "2px" } },
      h("button.btn.ghost.compact", { onclick: () => void chooseFile() }, `Open…`),
      h(
        "button.btn.ghost.compact",
        {
          disabled: !(st.prefs?.recent.length),
          onclick: (e: Event) => openRecent(e.currentTarget as HTMLElement),
        },
        "Recent",
      ),
      h("button.btn.ghost.compact", { onclick: () => store.set({ screen: "settings" }) }, "Settings"),
      h("button.btn.ghost.compact", { onclick: () => store.set({ screen: "about" }) }, "About"),
      h("button.btn.ghost.compact", { title: "The user guide (F1)", onclick: () => openHelp(helpPageFor(store.state)) }, "Help"),
      h("div.sep"),
      windowControls(),
    ),
  );
  appbar.setAttribute("data-tauri-drag-region", "");
}

function openRecent(anchor: HTMLElement): void {
  const recent = store.state.prefs?.recent ?? [];
  openPopover(
    anchor,
    h(
      "div.menu",
      null,
      ...recent.map((path) =>
        h(
          "button.item",
          {
            onclick: () => {
              closeOverlay();
              void openPath(path);
            },
          },
          h("span", { style: { flex: "1", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" } }, path.split(/[\\/]/).pop()),
        ),
      ),
      h("div.rule"),
      h(
        "button.item",
        {
          onclick: () => {
            closeOverlay();
            void chooseFile();
          },
        },
        h("span", { style: { flex: "1" } }, "Open…"),
        h("span.when", null, `${modKey(store.state.caps?.platform)}+O`),
      ),
    ),
  );
}

async function savePrefs(patch: Partial<Prefs>): Promise<void> {
  const current = store.state.prefs;
  if (!current) return;
  const next = { ...current, ...patch };
  const saved = await api.savePrefs(next);
  store.set({ prefs: saved });
  applyTheme(saved.theme);
}

function applyTheme(theme: Prefs["theme"]): void {
  const dark = theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

// ------------------------------------------------------------------ keyboard ---

function keyboard(e: KeyboardEvent): void {
  const mod = e.metaKey || e.ctrlKey;
  const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement;

  if (mod && e.key.toLowerCase() === "o") {
    e.preventDefault();
    void chooseFile();
    return;
  }
  if (mod && e.key.toLowerCase() === "e" && store.state.svg) {
    e.preventDefault();
    openExportSheet(store, rail, traceAndWait);
    return;
  }
  if (mod && e.key === ",") {
    e.preventDefault();
    store.set({ screen: "settings" });
    return;
  }
  if (typing) return;

  // ⌘1–⌘8 switch presets, for people who already know them.
  if (mod && /^[1-8]$/.test(e.key)) {
    const preset = store.state.caps?.presets[Number(e.key) - 1];
    if (preset) {
      e.preventDefault();
      store.set({ preset: preset.id, settings: { ...preset.settings } });
      controlChanged();
    }
    return;
  }
  if (e.key === "Escape") {
    if (store.state.screen) store.set({ screen: null });
    // Escape in the wizard keeps what is chosen so far; on the chooser it means Auto.
    else if (wizard.isOpen()) wizard.close(true);
    else if (store.state.chooser) store.set({ chooser: false });
    else if (store.state.tracing) {
      void api.cancelTrace();
      store.set({ tracing: false, stageState: store.state.svg ? { kind: "cancelled" } : { kind: "empty" } });
    }
  }
}

// ---------------------------------------------------------------- drag and drop ---

/**
 * The whole window responds to a drag, not a small box.
 *
 * Tauri reports the paths rather than the bytes, which is what we want: the file is read
 * in Rust and never crosses the IPC boundary as a blob.
 */
async function wireDragDrop(): Promise<void> {
  await getCurrentWebview().onDragDropEvent(async (event) => {
    if (event.payload.type === "over") {
      store.set({ dragging: true });
      document.body.classList.add("dragging");
      return;
    }
    if (event.payload.type === "leave") {
      store.set({ dragging: false });
      document.body.classList.remove("dragging");
      return;
    }
    store.set({ dragging: false });
    document.body.classList.remove("dragging");

    const paths = event.payload.paths ?? [];
    if (!paths.length) return;

    // An SVG belongs to the Minify tab and an image to Vectorize, whichever tab is
    // showing: dropping a file should do the obvious thing with it.
    const svgs = paths.filter((p) => p.toLowerCase().endsWith(".svg"));
    // Dropped on Vectorize, an SVG is something to re-trace clean.
    if (svgs.length === 1 && paths.length === 1 && store.state.tab === "vectorize") {
      await openPath(svgs[0]);
      return;
    }
    if (svgs.length) {
      // On the Fabricate tab an SVG is something to cut; anywhere else, to minify.
      const target = store.state.tab === "fabricate" ? "fabricate" : "minify";
      store.set({ tab: target });
      renderTab();
      const view = (target === "fabricate" ? fabView : minifyView) as
        | (HTMLElement & { acceptDropped?: (p: string) => Promise<boolean> })
        | null;
      await view?.acceptDropped?.(svgs[0]);
      return;
    }
    if (paths.length > 1) {
      store.set({ tab: "batch" });
      toast(`${paths.length} files dropped — choose a folder in the Batch tab to queue them.`);
      return;
    }
    store.set({ tab: "vectorize" });
    await openPath(paths[0]);
  });
}

// ---------------------------------------------------------------------- start ---

async function start(): Promise<void> {
  const app = document.getElementById("app");
  if (!app) return;
  app.append(appbar, content, screens);

  renderAppBar();
  renderTab();
  store.on(["tab", "source", "prefs", "caps"], renderAppBar);
  store.on(["tab"], renderTab);

  window.addEventListener("keydown", keyboard);
  installHelp(store);
  window
    .matchMedia("(prefers-color-scheme: dark)")
    .addEventListener("change", () => applyTheme(store.state.prefs?.theme ?? "system"));

  // Links that leave the app open in the system browser, never in the webview. Any
  // http(s) or mailto link qualifies, marked or not: a link that navigates the webview
  // would replace the app with a web page that has no way back.
  document.addEventListener("click", (e) => {
    const a = (e.target as HTMLElement)?.closest?.("a[href]");
    const href = a?.getAttribute("href") ?? "";
    if (a && /^(https?:|mailto:)/i.test(href)) {
      e.preventDefault();
      openUrl(href).catch(() => toast("Could not open the link in your browser.", { kind: "bad" }));
    }
  });

  await events.traceStage(({ generation, name, ms }) => {
    if (generation !== watching) return;
    const stages = [...store.state.liveStages];
    const existing = stages.find((s) => s.name === name);
    if (existing) existing.ms += ms;
    else stages.push({ name, ms });
    store.set({ liveStages: stages });
  });
  await events.traceDone(({ generation, outcome }) => {
    if (generation !== store.state.generation) return;
    // The confidence bands do not ride on the event, and are not fetched here either: the
    // viewer asks for them by generation the first time Certainty is shown.
    applyOutcome(outcome, generation);
  });
  // A second launch (the context menu's "Vectorize with Inkvec" while the app is open)
  // hands its file to this window.
  await events.openPath((path) => void openFromOutside(path));
  await events.batchRow((row) => {
    const b = store.state.batch;
    const at = b.rows.findIndex((r) => r.id === row.id);
    if (at >= 0) b.rows[at] = row;
    store.touch("batch");
  });
  await events.batchTotals((totals) => {
    store.state.batch.totals = totals;
    store.touch("batch");
  });
  await events.batchFinished((rows) => {
    const b = store.state.batch;
    b.rows = rows;
    b.running = false;
    b.paused = false;
    store.touch("batch");
    const failed = rows.filter((r) => r.state === "failed").length;
    toast(
      failed
        ? `Batch finished · ${failed} failed. Sort failures first to see them.`
        : `Batch finished · ${rows.filter((r) => r.state === "done").length} written.`,
      { kind: failed ? "bad" : "good" },
    );
  });

  const [caps, prefs] = await Promise.all([api.capabilities(), api.loadPrefs()]);
  store.set({ caps, prefs, settings: prefs.trace });
  applyTheme(prefs.theme);
  progress(`Engine ${caps.engineVersion} · ${caps.buildTarget}`, 0.55);
  samples = await api.listSamples();
  store.touch("source");

  progress("Preparing the workspace…", 0.85);
  await wireDragDrop();
  progress("Ready", 1);
  void api.appReady().catch(() => {});

  // The file the app was launched with, if any: the context menu's "Vectorize with Inkvec".
  const launched = await api.launchPath().catch(() => null);
  if (launched) void openFromOutside(launched);

  // The update check, if it is on. Its result only ever reaches the status strip.
  if (prefs.checkUpdates) {
    api
      .checkUpdate()
      .then((update) => store.set({ update }))
      .catch(() => {});
  }
}

/** Open a file handed to the app from outside it (the command line, a second launch). */
async function openFromOutside(path: string): Promise<void> {
  store.set({ tab: "vectorize", screen: null });
  await openPath(path);
}

/** Tell the splash window how far start-up has got. It is decoration: never worth a failure. */
function progress(text: string, fraction: number): void {
  void api.startupProgress(text, fraction).catch(() => {});
}

void start().catch((e) => {
  // Show the window either way: a start-up error is only readable if the window is.
  void api.appReady().catch(() => {});
  const app = document.getElementById("app");
  if (app) {
    fill(
      app,
      h(
        "div.firstrun",
        null,
        h("span.serif", { style: { fontSize: "24px" } }, "Inkvec Studio Lite could not start"),
        h("span.faint", { style: { maxWidth: "50ch", textAlign: "center" } }, String(e)),
      ),
    );
  }
});

/** Exported so a test harness can drive the shell without a window. */
export { applyOutcome, assignSetting, DEFAULT_SETTINGS, store, trace };
