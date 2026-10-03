/**
 * The app shell, for both builds: Inkvec Studio on the desktop and Inkvec Studio Lite in a
 * browser tab (`lib/platform.ts` says which).
 *
 * Owns the window chrome, the tab switcher, the keyboard, and the trace loop that ties a
 * moving control to a draft and a settled one to a full trace.
 *
 * The trace loop is the part worth reading. Every change to a control starts a draft at
 * once, so the viewer keeps up with the slider, and arms a timer; when the controls have
 * been still for `settleMs` the full-resolution trace is queued and swapped in with a
 * visible crossfade. Draft and final are two states of one result, never a silent
 * substitution. Its bookkeeping (generations, events that arrive early or late, Cancel) is
 * `lib/traceflow.ts`, which has no DOM or backend in it and is tested on its own; this file
 * wires it to the store and the backend and decides what a finished trace does to the
 * interface.
 *
 * Also here: opening an image (every entry point goes through `openWith`), the actions the
 * rail and the wizard call, preferences, the keyboard, drag and drop, and start-up.
 */

import { getCurrentWebview } from "@tauri-apps/api/webview";

import { fill, h } from "./lib/dom";
import {
  api,
  events,
  type ColourGroup,
  type DenoiserFetch,
  type Outcome,
  type Prefs,
  type SampleInfo,
  type Settings,
  type Snap,
  type Traced,
  type TraceMode,
} from "./lib/ipc";
import { appliesTo, initial, plannedTracePx, Store, type State } from "./lib/state";
import { applyRemembered, currentInterface, traceForKeeping, watchRemembered } from "./lib/remember";
import { APP_NAME, copyText, openExternal, pickedFile, pickedPath, pickFiles, WEB, type Picked } from "./lib/platform";
import { DEFAULT_SETTINGS, presetIndexForKey, resolvePreset as resolvePresetIn } from "./lib/presets";
import { Latest, TraceLoop, type Tier } from "./lib/traceflow";
import { markFramed, mountWebChrome, pastedImages, takeLaunch, webDrops, type Launch } from "./lib/web/chrome";
import { Previews } from "./lib/previews";
import { applyProgress, startClock, type TraceProgress } from "./lib/live";
import { createRail, PROMOTED, type RailActions } from "./components/rail";
import { createChooser, createWizard, type Snapshot, type WizardActions } from "./components/wizard";
import { proposeGroups } from "./components/palette";
import { openCardComposer } from "./components/card";
import { openExportSheet } from "./components/exportsheet";
import { installHelp } from "./components/help";
import { toast } from "./components/overlays";
import { renderAppBar } from "./components/appbar";
import { fetchLine, sentence } from "./components/denoiserfetch";
import { createBatch } from "./views/batch";
import { createMinify } from "./views/minify";
import { createFabricate } from "./views/fabricate";
import { createScreens, openDenoiserModal } from "./views/screens";
import { createWorkspace, jumpToWorst } from "./views/workspace";

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

/** The bundled sample images the empty stage offers, once the backend has listed them. */
let samples: SampleInfo[] = [];
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

/**
 * The engine each recent trace was started with, by generation. The Fast draft of an image
 * just opened runs the Fast engine whatever the controls say, so the status strip reads the
 * engine from here rather than from the controls. Kept as long as the loop keeps a trace's
 * colour groups: an older result is never shown.
 */
const enginesSent = new Map<number, TraceMode>();
const ENGINES_KEPT = 8;

/**
 * The trace loop (`lib/traceflow.ts`), wired to this store and the backend: which trace the
 * interface is waiting for, what each backend event does, and the settle timer.
 */
const loop = new TraceLoop<TraceProgress, Outcome, ColourGroup[]>({
  hasSource: () => Boolean(store.state.source),
  groups: () => store.state.colourGroups,
  // `fast`: the Fast draft of an image just opened (`fastDraftFirst`); the controls keep theirs.
  start: async (tier, fast) => {
    const settings: Settings = fast ? { ...traceSettings(), mode: "fast" } : traceSettings();
    const generation = await api.startTrace(settings, tier);
    enginesSent.set(generation, settings.mode);
    for (const old of enginesSent.keys()) if (old < generation - ENGINES_KEPT) enginesSent.delete(old);
    return generation;
  },
  settleMs: () => store.state.prefs?.settleMs ?? 800,
  shown: () => ({ generation: store.state.generation, tracing: store.state.tracing }),
  began: (generation, tier, asked) =>
    store.set({
      generation,
      tracing: true,
      tracingTier: tier,
      tracingEngine: enginesSent.get(generation) ?? store.state.settings.mode,
      liveStages: [],
      liveNow: null,
      traceLog: [],
      traceStarted: asked,
      traceEnded: 0,
    }),
  // The engine said what it is doing.
  progressed: (p) => store.set(applyProgress(store.state, p, performance.now())),
  // The confidence bands do not ride on the outcome, and are not fetched here either: the
  // viewer asks for them by generation the first time Certainty is shown.
  finished: (outcome, generation) => applyOutcome(outcome, generation),
  failed: (e) =>
    store.set({ tracing: false, liveNow: null, traceEnded: performance.now(), stageState: { kind: "failed", message: String(e) } }),
  cancel: () => {
    void api.cancelTrace();
    store.set({
      tracing: false,
      liveNow: null,
      traceEnded: performance.now(),
      stageState: store.state.svg ? { kind: "cancelled" } : { kind: "empty" },
    });
  },
});

/** Start a trace. A draft keeps up with a moving control; a final is what gets exported. */
function trace(tier: Tier): Promise<void> {
  return loop.trace(tier);
}

/**
 * Stop the trace in flight and go back to the last result. The full trace a moved control
 * had armed goes too (`TraceLoop.cancel`).
 */
function cancelTrace(): void {
  loop.cancel();
}

/** A control moved: draft now, full trace once the controls have been still. */
function controlChanged(): void {
  loop.controlChanged();
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
    loop.clearSettle();
    void trace("final");
  });
}

/** Which trace outcome arrived last, so one whose snapped paint finishes late is dropped. */
const outcomes = new Latest();

/**
 * A trace has finished. The user's snaps belong to the image rather than to one trace, so a
 * traced drawing is painted with them before it is shown: what is on screen, Copy SVG and
 * every exported format then agree, and the export's own fresh trace keeps them too.
 */
function applyOutcome(outcome: Outcome, generation = store.state.generation): void {
  const seq = outcomes.take();
  if (outcome.state !== "traced" || !store.state.snaps.length) {
    showOutcome(outcome, generation);
    return;
  }
  paintWithSnaps(outcome).then(
    (painted) => {
      if (outcomes.isNewest(seq)) showOutcome(outcome, generation, painted);
    },
    (e) => {
      if (!outcomes.isNewest(seq)) return;
      toast(`The snapped colours could not be applied to this trace: ${e}`, { kind: "bad" });
      showOutcome(outcome, generation);
    },
  );
}

/** `traced` painted with the snaps as they stand, however many times they change meanwhile. */
async function paintWithSnaps(traced: Traced): Promise<{ svg: string; inks: Traced["palette"] }> {
  for (;;) {
    const used = store.state.snaps;
    if (!used.length) return { svg: traced.svg, inks: traced.palette };
    const px = traced.report.tracedPx;
    const painted = await api.snapInks(traced.svg, used, px, px);
    if (store.state.snaps === used) return painted;
  }
}

/**
 * Put a finished trace on screen, or say why there is nothing to show. `painted` is the
 * drawing with the user's snaps applied, when there are any. A cancelled trace shows
 * nothing: whoever cancelled it has already put the interface back.
 */
function showOutcome(
  outcome: Outcome,
  generation: number,
  painted: { svg: string; inks: Traced["palette"] } | null = null,
): void {
  // A newer trace may have started while this one's snapped paint was being applied. The
  // drawing is still the newest finished one and is shown, but the trace in flight is not
  // this one, so the interface stays tracing and its log stays that trace's.
  const running = generation !== store.state.generation && store.state.tracing;
  if (outcome.state === "cancelled") return;
  if (!running) store.set({ liveNow: null, traceEnded: performance.now() });
  if (outcome.state === "traced") {
    const wasDraft = store.state.result?.tier === "draft";
    // A draft is smaller than a final, so only a final is ever the yardstick: the readout
    // compares this full trace with the one before it, and a draft in between changes
    // nothing about what that one was.
    const before = store.state.result?.tier === "final" ? store.state.report : store.state.previous;
    store.set({
      result: outcome,
      resultGeneration: generation,
      resultEngine: enginesSent.get(generation) ?? store.state.settings.mode,
      resultGroups: loop.groupsFor(generation) ?? store.state.resultGroups,
      bandsMissing: false,
      previous: outcome.tier === "final" ? before : store.state.previous,
      svg: painted?.svg ?? outcome.svg,
      report: outcome.report,
      palette: painted?.inks ?? outcome.palette,
      losses: outcome.losses,
      worstCorner: outcome.worstCorner,
      stageState: { kind: "drawing" },
      tracing: running,
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

  if (running) return;
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
  // Whatever was tracing belongs to the image being replaced: the backend stops it when the
  // new one opens, and nothing it still sends is wanted. Nor is a full trace a draft of the
  // last image had queued.
  loop.detach();
  store.set({
    tracing: false,
    liveNow: null,
    liveStages: [],
    traceLog: [],
    stageState: { kind: "decoding" },
    svg: null,
    report: null,
    previous: null,
    palette: [],
    losses: [],
    result: null,
    // Snaps, colour groups and what was proposed or turned down name colours of the image
    // that was open; the next one starts without any.
    snaps: [],
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
    // A large image in the browser: a Fast draft first, then this full trace.
    await loop.open(fastDraftFirst());
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

/**
 * Whether an image just opened is drawn by the Fast engine at the draft size before its full
 * trace (`TraceLoop.open`): in Inkvec Studio Lite, when the full trace is a Quality one larger
 * than the draft size. A browser tab's Quality trace of a 2048 px image takes 4-33 s and the
 * Fast draft well under one (r2-product); at or under the draft size there is no draft to
 * make (the engine would make the "draft" the final). The desktop's own threads trace a
 * 2048 px image in about two seconds, and it opens with the full trace alone, as before.
 */
function fastDraftFirst(): boolean {
  const st = store.state;
  if (!WEB || st.settings.mode === "fast") return false;
  const full = plannedTracePx(st, "final");
  const draft = plannedTracePx(st, "draft");
  return full !== null && draft !== null && full > draft;
}

/** What opening an image offers besides Auto: the chooser card, the wizard, or nothing. */
function offerChoice(): void {
  const on = store.state.prefs?.onOpen ?? "ask";
  if (on === "custom") wizard.open();
  else if (on === "ask") store.set({ chooser: true });
}

/** Open an image by its path on this computer (the desktop's dialogs, drops and Recent menu). */
async function openPath(path: string): Promise<void> {
  await openWith(async () => {
    const info = await api.openPath(path);
    store.set({ source: info });
    const prefs = await api.loadPrefs();
    store.set({ prefs });
  });
}

/** Open a file the user chose or dropped: by path on the desktop, by its bytes in a browser. */
async function openPicked(picked: Picked): Promise<void> {
  if (picked.path) return openPath(picked.path);
  const file = picked.file;
  if (!file) return;
  await openWith(async () => {
    const info = await api.openBytes(new Uint8Array(await file.arrayBuffer()), picked.name);
    store.set({ source: info });
  });
}

/** Ask for an image (or an SVG to re-trace) with the platform's file dialog, and open it. */
async function chooseFile(): Promise<void> {
  const [picked] = await pickFiles([
    // An SVG is accepted too: it is traced from its render, which is how a messy drawing
    // comes back as clean shapes.
    { name: "Images and SVG", extensions: ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "svg"] },
  ]);
  if (picked) await openPicked(picked);
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
  afterReset: (fresh) => {
    applyTheme(fresh.theme);
    putBack(fresh);
  },
  close: () => store.set({ screen: null }),
});

/**
 * A control in the rail, the wizard or on the stage changed one setting: write it, say so if
 * it asks for a denoiser that is not ready, and trace.
 */
function onSettingChanged<K extends keyof Settings>(key: K, value: Settings[K]): void {
  assignSetting(store.state.settings, key, value);
  store.touch("settings");
  const den = store.state.caps?.denoiser;
  if (WEB) {
    if (key === "cleanUpDamage" && value !== "off") noteDenoiserWait();
  } else if (key === "cleanUpDamage" && value !== "off" && den?.supported && !den.installed) {
    store.set({ stageState: { kind: "denoiserMissing" } });
  }
  controlChanged();
}

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
    cancel: cancelTrace,
    retryAt: (px) => {
      store.state.settings.traceSize = px;
      store.touch("settings");
      void trace("final");
    },
    jumpToWorst: () => jumpToWorst(store, workspace.viewer),
    openDenoiser: () => openDenoiserModal(store),
    retryDenoiser: () => railActs.retryDenoiser(),
    showUpdate: () => {
      const u = store.state.update;
      if (u) void openExternal(u.url);
    },
    markSeen: () => void savePrefs({ seenFirstRun: true }),
    changeSetting: onSettingChanged,
  },
  () => samples,
);

/** A preset id to the settings it means, against the presets loaded now (`lib/presets.ts`). */
function resolvePreset(id: string): { settings: Settings; wantsDenoiser: boolean } | null {
  return resolvePresetIn(store.state.caps, store.state.prefs, id);
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
    // A modal here would be one the user did not ask for. In a browser the denoiser
    // downloads by itself, and the rail says how far it has got instead.
    if (WEB) {
      if (preset.settings.cleanUpDamage !== "off") noteDenoiserWait();
    } else if (preset.wantsDenoiser && store.state.caps?.denoiser.supported && !store.state.caps.denoiser.installed) {
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
  changeSetting: onSettingChanged,
  // Only the rows the group is showing, which are the ones its count counted. A control hidden
  // because the engine that is on does not read it keeps its value until that engine is back,
  // and the three drawn above the tabs (the engine among them) are not the group's to reset:
  // Reset on Detail used to switch Fast back to Quality.
  resetGroup: (group) => {
    const base = baseSettings();
    const now = { ...store.state.settings };
    for (const c of store.state.caps?.controls ?? []) {
      if (c.group === group && !PROMOTED.has(c.key) && appliesTo(c, now)) assignSetting(store.state.settings, c.key, base[c.key]);
    }
    store.touch("settings");
    controlChanged();
  },
  traceNow: () => void trace("final"),
  cancel: cancelTrace,
  snap: (changes) => void snap(changes),
  setColourGroups: (groups) => {
    store.set({ colourGroups: groups, paletteSelection: [] });
    // Exactly what moving a control does: a draft now, the full trace once things settle.
    controlChanged();
  },
  openExport: () => openExportSheet(store, rail, traceAndWait),
  copySvg: async () => {
    if (!store.state.svg) return;
    await copyText(store.state.svg);
    toast("SVG copied. Paste straight into Figma or Illustrator.");
  },
  saveCard: () => openCardComposer(store),
  openDenoiser: () => openDenoiserModal(store),
  retryDenoiser: () => void api.denoiserDownload().catch((e) => toast(String(e), { kind: "bad" })),
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

/**
 * Snap inks, each named by the colour it was traced as. A later snap of the same ink
 * replaces the earlier one, and a snap back to the traced colour removes it.
 *
 * The drawing is always repainted from the trace's own SVG with every snap at once, never
 * by rewriting the painted one: "Snap N inks" is one rewrite rather than N racing each
 * other, and snapping an ink twice finds it by the colour it was traced as.
 */
async function snap(changes: Snap[]): Promise<void> {
  const st = store.state;
  const result = st.result;
  if (!result || !changes.length) return;
  const before = st.snaps;
  const named = new Set(changes.map((c) => c.from.toLowerCase()));
  const snaps = [
    ...st.snaps.filter((s) => !named.has(s.from.toLowerCase())),
    ...changes.filter((c) => c.to.toLowerCase() !== c.from.toLowerCase()),
  ];
  // Kept before the rewrite comes back, so a trace landing meanwhile is painted with it.
  store.set({ snaps });
  try {
    const painted = await paintWithSnaps(result);
    // A newer trace landed meanwhile: it was painted with these snaps as it arrived.
    if (store.state.result !== result) return;
    store.set({ svg: painted.svg, palette: painted.inks });
  } catch (e) {
    if (store.state.snaps === snaps) store.set({ snaps: before });
    toast(String(e), { kind: "bad" });
  }
}

/** The other tabs' views, built the first time each is shown and kept from then on. */
let minifyView: HTMLElement | null = null;
let fabView: HTMLElement | null = null;
let batchView: HTMLElement | null = null;

/** Show the selected tab's view under the app bar. */
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

/** Draw the app bar (`components/appbar.ts`) from the state as it is now. */
function drawAppBar(): void {
  renderAppBar(appbar, store, { openFile: () => void chooseFile(), openPath: (path) => void openPath(path) });
}

/**
 * Save a change to the preferences. What is sent always carries the trace controls and the
 * interface as they are now, not as they were when the preferences were loaded: sending the
 * loaded copy back used to put an old `trace` over the one the backend had kept.
 */
async function savePrefs(patch: Partial<Prefs>): Promise<void> {
  const current = store.state.prefs;
  if (!current) return;
  const next = { ...current, trace: traceForKeeping(store.state.settings), ui: currentInterface(store.state), ...patch };
  const saved = await api.savePrefs(next);
  store.set({ prefs: saved });
  applyTheme(saved.theme);
}

/**
 * The remembered choices changed (`lib/remember.ts`). Saved without announcing new
 * preferences to the store: the interface already shows every one of them, and a panel
 * rebuilt for its own echo would lose a slider from under the pointer.
 */
async function autosave(patch: Pick<Prefs, "trace" | "ui">): Promise<void> {
  const current = store.state.prefs;
  if (!current) return;
  try {
    const saved = await api.savePrefs({ ...current, ...patch });
    // Anything the user changed in Settings meanwhile arrives by `savePrefs`, not here.
    if (store.state.prefs === current) store.state.prefs = saved;
  } catch {
    // Nowhere to write (a full disk, blocked storage): the session carries on unsaved.
  }
}

/** The tabs this build has. */
const TABS: readonly State["tab"][] = WEB ? ["vectorize", "minify", "fabricate"] : ["vectorize", "minify", "fabricate", "batch"];

/** Put the interface back as the preferences remember it, and the theme with it. */
function putBack(prefs: Prefs): void {
  applyRemembered(store, prefs, TABS, (id) => resolvePreset(id) !== null);
}

/** Light or dark, as the preferences say; "system" follows the operating system's setting. */
function applyTheme(theme: Prefs["theme"]): void {
  const dark = theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

// ------------------------------------------------------------------ keyboard ---

/**
 * The window's shortcuts: Ctrl/⌘+O opens, +E exports, +, opens Settings, +1…9 and +0 pick
 * a preset, and Escape closes whatever is on top or cancels the trace. Only the first three
 * work while typing in a field.
 */
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

  // ⌘1–⌘9 and ⌘0 switch presets, for people who already know them.
  const presetAt = presetIndexForKey(e.key);
  if (mod && presetAt !== null) {
    const preset = store.state.caps?.presets[presetAt];
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
    else if (store.state.tracing) cancelTrace();
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
  if (WEB) {
    webDrops(store, (files) => void dropped(files.map(pickedFile)));
    return;
  }
  pastedImages((files) => void dropped(files.map(pickedFile)));
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

    await dropped((event.payload.paths ?? []).map(pickedPath));
  });
}

/** Files dropped on the window (or pasted, in a browser): the obvious thing with each. */
async function dropped(files: Picked[]): Promise<void> {
  if (!files.length) return;
  // An SVG belongs to the Minify tab and an image to Vectorize, whichever tab is
  // showing: dropping a file should do the obvious thing with it.
  const svgs = files.filter((p) => p.name.toLowerCase().endsWith(".svg"));
  // Dropped on Vectorize, an SVG is something to re-trace clean.
  if (svgs.length === 1 && files.length === 1 && store.state.tab === "vectorize") {
    await openPicked(svgs[0]);
    return;
  }
  if (svgs.length) {
    // On the Fabricate tab an SVG is something to cut; anywhere else, to minify.
    const target = store.state.tab === "fabricate" ? "fabricate" : "minify";
    store.set({ tab: target });
    renderTab();
    const view = (target === "fabricate" ? fabView : minifyView) as
      | (HTMLElement & { acceptDropped?: (p: Picked) => Promise<boolean> })
      | null;
    await view?.acceptDropped?.(svgs[0]);
    return;
  }
  if (files.length > 1) {
    if (WEB) {
      toast(`${files.length} files dropped. ${APP_NAME} opens one image at a time; the desktop app has Batch.`);
      await openPicked(files[0]);
      return;
    }
    store.set({ tab: "batch" });
    toast(`${files.length} files dropped — choose a folder in the Batch tab to queue them.`);
    return;
  }
  store.set({ tab: "vectorize" });
  await openPicked(files[0]);
}

// ---------------------------------------------------------------------- start ---

/**
 * Start the app: mount the shell, subscribe to the backend's events, load the capabilities
 * and preferences, put the interface back as it was left, and open whatever the app was
 * launched with. Each step reports to the splash window (desktop) or loading screen (browser).
 */
async function start(): Promise<void> {
  const app = document.getElementById("app");
  if (!app) return;
  app.append(appbar, content, screens);
  if (WEB) {
    mountWebChrome(app);
    markFramed();
  }

  drawAppBar();
  renderTab();
  store.on(["tab", "source", "prefs", "caps"], drawAppBar);
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
      openExternal(href).catch(() => toast("Could not open the link in your browser.", { kind: "bad" }));
    }
  });

  // The live counters: ten times a second while a trace runs, written in place.
  const syncClock = startClock(
    () => store.state.tracing,
    () => store.state.traceStarted,
    () => store.state.liveNow?.since ?? null,
  );
  store.on(["tracing", "liveNow", "traceLog", "liveStages", "tab", "railTab"], syncClock);
  await events.traceProgress((p) => loop.onProgress(p));
  await events.traceDone(({ generation, outcome }) => loop.onDone(generation, outcome));
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

  if (WEB) await events.denoiserFetch(denoiserFetched);

  const [caps, prefs] = await Promise.all([api.capabilities(), api.loadPrefs()]);
  store.set({ caps, prefs, settings: prefs.trace });
  applyTheme(prefs.theme);
  // The interface as it was left, before anything is shown; then kept as it changes.
  putBack(prefs);
  watchRemembered(store, (patch) => void autosave(patch));
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
  // In the browser: a sample or a file the presentation page handed over.
  if (WEB) void takeLaunch().then(openLaunch);

  // The update check, if it is on. Its result only ever reaches the status strip.
  if (prefs.checkUpdates) {
    api
      .checkUpdate()
      .then((update) => store.set({ update }))
      .catch(() => {});
  }
}

/** Open what the Space's presentation page asked for: a bundled sample, or a dropped file. */
async function openLaunch(launch: Launch): Promise<void> {
  if (!launch) return;
  store.set({ tab: "vectorize", screen: null });
  if ("sample" in launch) {
    await openWith(async () => store.set({ source: await api.openSample(launch.sample) }));
  } else {
    await openWith(async () => store.set({ source: await api.openBytes(launch.bytes, launch.name) }));
  }
}

/** Open a file handed to the app from outside it (the command line, a second launch). */
async function openFromOutside(path: string): Promise<void> {
  store.set({ tab: "vectorize", screen: null });
  await openPath(path);
}

// ------------------------------------------------------ the browser's denoiser ---
//
// Inkvec Studio Lite fetches the denoiser in the background from the moment the engine has
// arrived (lib/web/engine.ts). A trace that asks for it before it is ready is traced without
// it; these say so once, keep the rail's note current, and trace again when it is ready.

/** The denoiser was asked for (a control, a preset): if it is not ready, say what happens. */
function noteDenoiserWait(): void {
  const f = store.state.denoiserFetch;
  if (!store.state.caps?.denoiser.supported || f?.phase === "ready") return;
  const line = fetchLine(f, true);
  toast(
    f?.phase === "failed"
      ? "The denoiser did not download. The trace runs without it."
      : `${sentence(line ?? "Starting the denoiser…")} The trace shown is without it until it is ready, then it traces again by itself.`,
    f?.phase === "failed" ? { kind: "bad", action: { label: "Retry", run: () => railActs.retryDenoiser() } } : {},
  );
}

/**
 * The browser's denoiser download moved on: keep the capabilities' "installed" current, trace
 * again once it is ready if the controls want it, and say so once if it failed.
 */
function denoiserFetched(f: DenoiserFetch): void {
  const before = store.state.denoiserFetch?.phase;
  store.set({ denoiserFetch: f });
  const caps = store.state.caps;
  // Stored or started: Settings and the dialogs read "installed" from the capabilities.
  if ((f.phase === "stored" || f.phase === "ready") && caps && !caps.denoiser.installed) {
    void api.denoiserStatus().then((status) => {
      const now = store.state.caps;
      if (now) store.set({ caps: { ...now, denoiser: status } });
    });
  }
  const wanted = store.state.settings.cleanUpDamage !== "off";
  if (f.retrace && wanted && store.state.source) {
    toast("The denoiser is ready. Tracing again with it.", { kind: "good" });
    void trace("final");
  } else if (f.phase === "failed" && before !== "failed" && wanted) {
    toast(`The denoiser did not download: ${f.message ?? "the connection dropped"}.`, {
      kind: "bad",
      action: { label: "Retry", run: () => railActs.retryDenoiser() },
    });
  }
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
        h("span.serif", { style: { fontSize: "24px" } }, `${APP_NAME} could not start`),
        h("span.faint", { style: { maxWidth: "50ch", textAlign: "center" } }, String(e)),
      ),
    );
  }
});

/**
 * The store, for the dev mock (`dev/boot.ts`), which puts it on the window for
 * `tools/mock_checks.py` to read.
 */
export { store };
