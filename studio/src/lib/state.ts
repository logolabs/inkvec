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
export type Tab = "vectorize" | "minify" | "batch";

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
  | { kind: "cancelled" };

/** How the two panes are arranged. */
export type ViewMode = "side" | "wipe" | "ab";

export interface State {
  caps: Capabilities | null;
  prefs: Prefs | null;

  tab: Tab;
  screen: Screen;

  source: SourceInfo | null;
  settings: Settings;
  preset: PresetId | null;

  /** The trace the interface is waiting for, or 0. */
  generation: number;
  tracing: boolean;
  /** Stages of the trace in flight, in the order they finished. */
  liveStages: Stage[];
  /** Whether the expanded stage list is showing. */
  stagesOpen: boolean;

  result: Traced | null;
  /** The drawing as it is painted now: the trace's own SVG, or one with snapped fills. */
  svg: string | null;
  palette: Ink[];
  losses: Loss[];
  report: Report | null;
  worstCorner: WorstCorner | null;
  stageState: StageState;
  /** Set for one beat after a draft is replaced by a final, so the swap is visible. */
  justSwapped: boolean;

  view: ViewMode;
  show: { fill: boolean; wireframe: boolean; anchors: boolean; handles: boolean };
  zoom: number;
  pan: { x: number; y: number };
  /** A/B and the hold-to-flick key show the source instead of the vector. */
  flicked: boolean;
  wipe: number;
  detail: boolean;

  advancedOpen: boolean;
  exportOpen: boolean;
  dragging: boolean;

  minify: {
    name: string | null;
    before: string | null;
    settings: MinifySettings;
    result: MinifyResult | null;
    error: string | null;
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
    liveStages: [],
    stagesOpen: false,
    result: null,
    svg: null,
    palette: [],
    losses: [],
    report: null,
    worstCorner: null,
    stageState: { kind: "empty" },
    justSwapped: false,
    view: "side",
    show: { fill: true, wireframe: false, anchors: false, handles: false },
    zoom: 1,
    pan: { x: 0, y: 0 },
    flicked: false,
    wipe: 0.5,
    detail: false,
    advancedOpen: false,
    exportOpen: false,
    dragging: false,
    minify: { name: null, before: null, settings: minify, result: null, error: null },
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
