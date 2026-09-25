/**
 * The store, and the formatting rules every readout shares.
 *
 * One mutable object and a set of subscribers. Panels subscribe to the keys they care
 * about and rebuild themselves when one changes, which keeps the viewer — the only part
 * that is expensive to rebuild — out of the update path entirely.
 */

import type {
  BatchRow,
  BatchTotals,
  Capabilities,
  ColourGroup,
  FabAnalysis,
  FabOptions,
  FabPlan,
  Ink,
  Loss,
  MinifyResult,
  MinifySettings,
  PresetId,
  Prefs,
  Report,
  Settings,
  SourceInfo,
  Stage,
  Traced,
  UpdateInfo,
  WorstCorner,
} from "./ipc";

/** Which top-level tab is showing. */
export type Tab = "vectorize" | "minify" | "fabricate" | "batch";

/** Which full-window screen is showing over the tabs, if any. */
export type Screen = null | "settings" | "about";

/** What the stage is showing, when it is not showing a drawing. */
export type StageState =
  | { kind: "empty" }
  | { kind: "decoding" }
  | { kind: "drawing" }
  | { kind: "flat" }
  | { kind: "undecodable"; message: string }
  | { kind: "outOfMemory"; neededGb: number; suggestPx: number }
  | { kind: "failed"; message: string }
  | { kind: "cancelled" }
  /** A preset wants the denoiser and it is not installed. Not an error: the trace runs. */
  | { kind: "denoiserMissing" }
  /** The update check could not reach the network. Tracing never needed one. */
  | { kind: "offline"; message: string };

/** A proposed colour group; `edited` once the user has changed it, so a new proposal keeps it. */
export type Suggestion = ColourGroup & { edited?: boolean };

/** The two halves of the rail. */
export type RailTab = "result" | "tune";

/** How the two panes are arranged. */
export type ViewMode = "side" | "wipe" | "ab";

export interface State {
  caps: Capabilities | null;
  prefs: Prefs | null;

  tab: Tab;
  screen: Screen;

  source: SourceInfo | null;
  settings: Settings;
  /** A built-in `PresetId`, or a saved preset's id. `null` once a control is moved. */
  preset: string | null;

  /** The trace the interface is waiting for, or 0. */
  generation: number;
  tracing: boolean;
  /** Which tier the trace in flight is, so the size shown for it is the size it runs at. */
  tracingTier: "draft" | "final";
  /** Stages of the trace in flight, in the order they finished. */
  liveStages: Stage[];

  result: Traced | null;
  /** The trace that produced `result`: what the backend keeps its confidence bands under. */
  resultGeneration: number;
  /** The backend had no confidence bands for `result`, so Certainty has nothing to show. */
  bandsMissing: boolean;
  /**
   * The full trace before the current one, for the same image: what the readout's
   * deltas are measured against, so moving a control shows what it bought or cost.
   */
  previous: Report | null;
  /** The drawing as it is painted now: the trace's own SVG, or one with snapped fills. */
  svg: string | null;
  palette: Ink[];
  /**
   * Colours the user has asked to be drawn as one, for this image. Sent with every trace;
   * never saved with the preferences or a preset, never given to a batch, and emptied when
   * another image is opened.
   */
  colourGroups: ColourGroup[];
  /**
   * Proposed groups the palette offers to accept, for this image. `null` until the first
   * full trace has proposed any.
   */
  groupSuggestions: Suggestion[] | null;
  /** The colour groups the drawing on screen was traced with, which its merge report describes. */
  resultGroups: ColourGroup[];
  /** Suggestions the user dismissed, by `groupSignature`, so they are not offered again. */
  dismissedGroups: string[];
  /** "Simplify to N colours": propose merging down to this many inks, or null for near duplicates only. */
  simplifyTo: number | null;
  /** Palette members picked for the keyboard/click "Merge" path. */
  paletteSelection: string[];
  /**
   * The paint values (`fill`/`stroke` attribute values) to single out in the vector pane
   * while a swatch, group or member is hovered or focused; null when nothing is.
   */
  hoverFill: string[] | null;
  losses: Loss[];
  report: Report | null;
  worstCorner: WorstCorner | null;
  stageState: StageState;
  /** Set for one beat after a draft is replaced by a final, so the swap is visible. */
  justSwapped: boolean;

  view: ViewMode;
  show: { fill: boolean; wireframe: boolean; anchors: boolean; handles: boolean; certainty: boolean };
  zoom: number;
  pan: { x: number; y: number };
  /** Whether the view is the fitted one, as opposed to a zoom the user chose. */
  fitted: boolean;
  /** A/B and the hold-to-flick key show the source instead of the vector. */
  flicked: boolean;
  wipe: number;
  detail: boolean;

  /** Which half of the rail is showing: what came out, or the controls that make it. */
  railTab: RailTab;
  /** Which control groups are folded open in the Tune tab. */
  groupsOpen: Record<string, boolean>;
  exportOpen: boolean;
  dragging: boolean;

  minify: {
    name: string | null;
    before: string | null;
    settings: MinifySettings;
    result: MinifyResult | null;
    error: string | null;
  };

  /** The Fabricate tab: the drawing, what is in it, the request and what came back. */
  fab: {
    name: string | null;
    svg: string | null;
    analysis: FabAnalysis | null;
    options: FabOptions;
    plan: FabPlan | null;
    error: string | null;
    /** Which sheet the stage shows; -1 for all of them stacked. */
    shown: number;
    /** Lengths are shown in millimetres or inches; the engine always works in mm. */
    unit: "mm" | "in";
    /** Whether the preflight's problems are drawn over the sheets. */
    showProblems: boolean;
    /** The material preset last chosen, if the options still match it. */
    preset: string | null;
    /** Whether saving also writes a DXF, for CAD and CNC programs. */
    dxf: boolean;
    gcode: boolean;
  };

  batch: {
    folder: string | null;
    outputDir: string | null;
    preset: PresetId;
    skipExisting: boolean;
    rows: BatchRow[];
    totals: BatchTotals | null;
    running: boolean;
    paused: boolean;
    failuresFirst: boolean;
  };

  update: UpdateInfo | null;
}

/** The starting state, before anything has been loaded. */
export function initial(settings: Settings, minify: MinifySettings): State {
  return {
    caps: null,
    prefs: null,
    tab: "vectorize",
    screen: null,
    source: null,
    settings,
    preset: "logo",
    generation: 0,
    tracing: false,
    tracingTier: "final",
    liveStages: [],
    result: null,
    resultGeneration: 0,
    bandsMissing: false,
    previous: null,
    svg: null,
    palette: [],
    colourGroups: [],
    groupSuggestions: null,
    resultGroups: [],
    dismissedGroups: [],
    simplifyTo: null,
    paletteSelection: [],
    hoverFill: null,
    losses: [],
    report: null,
    worstCorner: null,
    stageState: { kind: "empty" },
    justSwapped: false,
    view: "side",
    show: { fill: true, wireframe: false, anchors: false, handles: false, certainty: false },
    zoom: 1,
    pan: { x: 0, y: 0 },
    fitted: true,
    flicked: false,
    wipe: 0.5,
    detail: false,
    railTab: "result",
    groupsOpen: { Detail: true, Colour: true, Shape: true, Output: false },
    exportOpen: false,
    dragging: false,
    minify: { name: null, before: null, settings: minify, result: null, error: null },
    fab: { name: null, svg: null, analysis: null, options: defaultFabOptions(), plan: null, error: null, shown: -1, unit: "mm", showProblems: true, preset: null, dxf: false, gcode: false },
    batch: {
      folder: null,
      outputDir: null,
      preset: "logo",
      skipExisting: true,
      rows: [],
      totals: null,
      running: false,
      paused: false,
      failuresFirst: false,
    },
    update: null,
  };
}

/** The Fabricate tab's starting request: the engine's own defaults. */
export function defaultFabOptions(): FabOptions {
  return {
    mode: "singleColour",
    widthMm: 100,
    include: [],
    order: [],
    bleedMm: 0.8,
    minFeatureMm: 0.8,
    removeThin: false,
    stickerMarginMm: 3,
    bridgeMm: 1.5,
    stencilMarginMm: 10,
    maxLineMm: 0,
    penMm: 0.4,
    registration: true,
    weedBorderMm: 0,
    mirror: false,
    cutStyle: "filled",
    sizeCheckMm: 0,
    dogboneMm: 0,
    gcodeFeedMmMin: 1000,
    gcodePower: 1000,
    gcodePasses: 1,
    fileUnits: "mm",
    kerfMm: 0,
    toleranceMm: 0.05,
    mergeDeltaE: 3,
  };
}

type Listener = (changed: Set<keyof State>) => void;

/** A store with per-key change notification. */
export class Store {
  private listeners = new Set<Listener>();

  constructor(public state: State) {}

  /** Merge `patch` into the state and tell everyone which keys moved. */
  set(patch: Partial<State>): void {
    const changed = new Set<keyof State>();
    for (const [k, v] of Object.entries(patch) as [keyof State, never][]) {
      if (this.state[k] !== v) {
        this.state[k] = v;
        changed.add(k);
      }
    }
    if (changed.size) this.emit(changed);
  }

  /** Announce that `keys` changed after mutating them in place. */
  touch(...keys: (keyof State)[]): void {
    this.emit(new Set(keys));
  }

  /** Run `fn` whenever one of `keys` changes; returns an unsubscribe. */
  on(keys: (keyof State)[], fn: () => void): () => void {
    const wanted = new Set(keys);
    const listener: Listener = (changed) => {
      for (const k of changed) {
        if (wanted.has(k)) {
          fn();
          return;
        }
      }
    };
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private emit(changed: Set<keyof State>) {
    for (const l of [...this.listeners]) l(changed);
  }
}

// ----------------------------------------------------------------- formatting ---

/**
 * Every numeral in the app goes through one of these.
 *
 * The voice note in the brief is that numbers carry a unit and something to compare
 * against, and that the report should read as instrument output. Consistent rounding is
 * most of that: a figure that shows three decimals in one panel and one in another reads
 * as two different instruments.
 */

/** 1382 → "1,382". */
export function count(n: number): string {
  return n.toLocaleString("en-US");
}

/** Bytes at the precision a reader can act on. */
export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** A colour difference, always to two decimals: 0.1 and 0.12 are different claims. */
export function de00(n: number | null | undefined): string {
  return n === null || n === undefined ? "—" : n.toFixed(2);
}

/** Seconds, or milliseconds when that is the honest unit. */
export function seconds(n: number): string {
  return n < 1 ? `${Math.round(n * 1000)} ms` : `${n.toFixed(2)} s`;
}

/**
 * The longer side, in pixels, a trace will actually run at.
 *
 * The trace-size control is a *cap*: an input larger than it is traced at the cap and the SVG
 * is written back at full size, and a smaller input is traced at the size it arrived. A draft
 * is capped again at the draft size. Showing the setting itself, as the interface used to,
 * told somebody with a 622 px logo that it was being traced at 2048 px.
 */
export function plannedTracePx(st: State, tier: "draft" | "final"): number | null {
  if (!st.source) return null;
  const longer = Math.max(st.source.width, st.source.height);
  const cap = tier === "draft" ? Math.min(st.prefs?.draftPx ?? 512, st.settings.traceSize) : st.settings.traceSize;
  return Math.min(cap, longer);
}

/** A share of the canvas. */
export function percent(fraction: number, decimals = 0): string {
  return `${(fraction * 100).toFixed(decimals)}%`;
}

/** "4 min 12 s", for a batch's remaining time. */
export function duration(total: number): string {
  const s = Math.max(0, Math.round(total));
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  return `${m} min ${s % 60} s`;
}

/** The modifier key this platform writes, for keyboard hints. */
export function modKey(platform: string | undefined): string {
  return platform === "macos" ? "⌘" : "Ctrl";
}
