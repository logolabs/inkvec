/**
 * The interface's remembered choices, for both builds: which tab, which half of the rail,
 * the viewer's arrangement and layers, the export formats, the Minify and Fabricate tabs'
 * controls, the batch queue's switches, and the selected preset.
 *
 * They travel inside the preferences (`Prefs.ui`, the core's `Interface`), so the desktop
 * keeps them in its preferences file and the browser in its storage, through the same
 * commands as every other preference, sanitised by the same code. They are put back once,
 * right after the preferences load and before the app is shown (the desktop's splash and
 * the browser's loading screen are still up), and saved a moment after they change: no
 * button, no flash of the defaults.
 *
 * Nothing about the image on screen is kept: its colour groups, snaps, zoom and pan belong
 * to it and start fresh with the next one.
 */

import type { Formats, InterfacePrefs, Prefs, Settings } from "./ipc";
import { defaultFabOptions, type State, type Store } from "./state";

/** The core's `Interface::default()`, for a backend that sends none (the dev mock). */
export function defaultInterface(): InterfacePrefs {
  return {
    preset: "logo",
    tab: "vectorize",
    railTab: "result",
    groupsOpen: {},
    view: "side",
    wipe: 0.5,
    show: { fill: true, wireframe: false, anchors: false, handles: false, certainty: false },
    detail: false,
    export: null,
    minify: null,
    minifyBackdrop: "auto",
    minifyView: "side",
    fab: null,
    fabUnit: "mm",
    fabShowProblems: true,
    fabPreset: null,
    fabDxf: false,
    fabGcode: false,
    batchPreset: null,
    batchSkipExisting: true,
    batchFailuresFirst: false,
  };
}

/** What components keep outside the store, and hand over as it changes. */
const local: Pick<InterfacePrefs, "export" | "minifyBackdrop" | "minifyView"> = {
  export: null,
  minifyBackdrop: "auto",
  minifyView: "side",
};

let schedule: () => void = () => {};

/** The choices as they were loaded, for a component that sets itself up from them. */
export function remembered(): typeof local {
  return { ...local };
}

/** A component's own choice changed (the export formats, the Minify backdrop): keep it. */
export function remember(patch: Partial<typeof local>): void {
  Object.assign(local, patch);
  schedule();
}

/** The export sheet's formats: the remembered ones, or the defaults. */
export function rememberedFormats(fallback: Formats): Formats {
  const f = local.export;
  return f ? { ...fallback, ...f, pngSizes: [...(f.pngSizes ?? fallback.pngSizes)] } : fallback;
}

/** The choices as the interface shows them now. */
export function currentInterface(st: State): InterfacePrefs {
  const options: Record<string, unknown> = { ...st.fab.options };
  delete options.include;
  delete options.order;
  return {
    preset: st.preset,
    tab: st.tab,
    railTab: st.railTab,
    groupsOpen: { ...st.groupsOpen },
    view: st.view,
    wipe: st.wipe,
    show: { ...st.show },
    detail: st.detail,
    export: local.export,
    minify: { ...st.minify.settings },
    minifyBackdrop: local.minifyBackdrop,
    minifyView: local.minifyView,
    fab: options,
    fabUnit: st.fab.unit,
    fabShowProblems: st.fab.showProblems,
    fabPreset: st.fab.preset,
    fabDxf: st.fab.dxf,
    fabGcode: st.fab.gcode,
    batchPreset: st.batch.preset,
    batchSkipExisting: st.batch.skipExisting,
    batchFailuresFirst: st.batch.failuresFirst,
  };
}

/** The trace controls as they are kept: without this image's colour groups. */
export function traceForKeeping(settings: Settings): Settings {
  const kept = { ...settings };
  delete kept.colourGroups;
  return kept;
}

/**
 * Put the remembered choices back. `tabs` are the tabs this build has (the browser has no
 * Batch); `presetExists` says whether a remembered preset id still names one.
 */
export function applyRemembered(
  store: Store,
  prefs: Prefs,
  tabs: readonly State["tab"][],
  presetExists: (id: string) => boolean,
): void {
  const st = store.state;
  const ui = { ...defaultInterface(), ...(prefs.ui ?? {}) };
  local.export = ui.export;
  local.minifyBackdrop = ui.minifyBackdrop;
  local.minifyView = ui.minifyView;
  store.set({
    // A preset that no longer exists (a saved one was deleted) is no longer selected.
    preset: ui.preset && presetExists(ui.preset) ? ui.preset : null,
    tab: tabs.includes(ui.tab) ? ui.tab : "vectorize",
    railTab: ui.railTab,
    groupsOpen: { ...st.groupsOpen, ...ui.groupsOpen },
    view: ui.view,
    wipe: ui.wipe,
    show: { ...st.show, ...ui.show },
    detail: ui.detail,
    minify: { ...st.minify, settings: { ...st.minify.settings, ...(ui.minify ?? {}) } },
    fab: {
      ...st.fab,
      options: { ...defaultFabOptions(), ...(ui.fab ?? {}), include: [], order: [] },
      unit: ui.fabUnit,
      showProblems: ui.fabShowProblems,
      preset: ui.fabPreset,
      dxf: ui.fabDxf,
      gcode: ui.fabGcode,
    },
    batch: {
      ...st.batch,
      preset: (ui.batchPreset && presetExists(ui.batchPreset) ? ui.batchPreset : st.batch.preset) as State["batch"]["preset"],
      skipExisting: ui.batchSkipExisting,
      failuresFirst: ui.batchFailuresFirst,
    },
  });
}

/** The keys of the state that hold a remembered choice. */
const WATCHED: (keyof State)[] = ["settings", "preset", "tab", "railTab", "groupsOpen", "view", "wipe", "show", "detail", "minify", "fab", "batch"];

/**
 * Save the choices a moment after they change, and at once when the page is hidden or
 * closed. `save` gets the interface and the trace controls as they are now; it is only
 * called when one of them differs from what was last saved, so a key that changes for
 * another reason (a Minify result arriving) costs a comparison and nothing more.
 */
export function watchRemembered(
  store: Store,
  save: (patch: { ui: InterfacePrefs; trace: Settings }) => void,
  delayMs = 500,
): () => void {
  let timer = 0;
  let last = JSON.stringify({ ui: currentInterface(store.state), trace: traceForKeeping(store.state.settings) });
  const flush = () => {
    window.clearTimeout(timer);
    timer = 0;
    if (!store.state.prefs) return;
    const patch = { ui: currentInterface(store.state), trace: traceForKeeping(store.state.settings) };
    const key = JSON.stringify(patch);
    if (key === last) return;
    last = key;
    save(patch);
  };
  schedule = () => {
    window.clearTimeout(timer);
    timer = window.setTimeout(flush, delayMs);
  };
  const stop = store.on(WATCHED, schedule);
  // Closing the tab or the window does not wait for a timer.
  const hidden = () => {
    if (document.visibilityState === "hidden" && timer) flush();
  };
  window.addEventListener("pagehide", flush);
  document.addEventListener("visibilitychange", hidden);
  return () => {
    stop();
    window.removeEventListener("pagehide", flush);
    document.removeEventListener("visibilitychange", hidden);
  };
}
