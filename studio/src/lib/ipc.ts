/**
 * The backend, typed.
 *
 * Every shape here mirrors a `serde` struct in `src-tauri`. They are written out rather
 * than generated because there are about twenty of them and a code generator is a build
 * step somebody has to maintain; the backend's tests assert the field names, so a rename
 * on either side shows up as a type error here or a failing test there.
 */

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";

import { webBackend } from "./web/engine";

// ------------------------------------------------------------------- transport ---
//
// One interface, two backends. On the desktop every command is a Tauri `invoke` and every
// event a Tauri `listen`, exactly as they always were. In the browser build (Inkvec Studio
// Lite, `__INKVEC_WEB__`) the same names reach `web/engine.ts`, which answers them with the
// same core compiled to WebAssembly. `__INKVEC_WEB__` is a build-time constant, so each
// bundle keeps only its own branch.

function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return __INKVEC_WEB__ ? webBackend().invoke<T>(cmd, args) : tauriInvoke<T>(cmd, args);
}

function listen<T>(event: string, fn: EventCallback<T>): Promise<UnlistenFn> {
  return __INKVEC_WEB__
    ? webBackend().listen<T>(event, fn as (e: { payload: T }) => void)
    : tauriListen<T>(event, fn);
}

// ---------------------------------------------------------------------- settings ---

export type Cleanup = "off" | "auto" | "on";
export type TraceMode = "quality" | "fast";

export interface Settings {
  mode: TraceMode;
  precision: number;
  speckleFloor: number;
  traceSize: number;
  timeLimit: number;
  maxColours: number;
  colourMerging: number;
  flatFills: boolean;
  blackAndWhite: boolean;
  cleanUpDamage: Cleanup;
  matchRepeatedShapes: boolean;
  matchThreshold: number;
  fewerPaths: boolean;
  lineArt: boolean;
  repairRings: boolean;
  editability: boolean;
  /** What one Bézier curve costs the fit, in parameters. A line costs 2. */
  bezierCost: number;
  /** The turn, in degrees, at a join that is charged as a full corner. */
  cornerAngle: number;
  minify: boolean;
  transparentBackground: boolean;
  margin: number;
  holesAsCutouts: boolean;
  traceTransparency: boolean;
  /**
   * Colours to draw as one, for the image that is open. Not a control: the palette panel
   * owns them (`State.colourGroups`) and they are added to the settings only when a trace is
   * started, so presets, saved presets and preferences never carry them.
   */
  colourGroups?: ColourGroup[];
}

/** A traced flat ink, by the colour it was measured as, and what to paint it. Mirrors `api::Snap`. */
export interface Snap {
  from: string;
  to: string;
}

/**
 * Fills to draw as one. Mirrors `options::ColourGroup`; each member is spelt as the engine's
 * `--merge-colors` grammar spells it: `#rrggbb`, or a gradient's stops joined by `>`.
 */
export interface ColourGroup {
  members: string[];
  /** `#rrggbb` a flat colour, `@n` the n-th member (1-based), null the most-used member. */
  target: string | null;
}

export type PresetId =
  | "logo"
  | "icon"
  | "fine-detail"
  | "fewer-paths"
  | "photo-or-scan"
  | "black-and-white"
  | "line-art"
  | "editable";

export interface Control {
  group: "Detail" | "Colour" | "Shape" | "Output";
  key: keyof Settings;
  label: string;
  unit: string;
  kind: "range" | "switch" | "tri" | "choice";
  min: number;
  max: number;
  curve: number;
  decimals: number;
  stops: { at: number; label: string }[];
  help: string;
}

export interface PresetInfo {
  id: PresetId;
  name: string;
  subtitle: string;
  settings: Settings;
  wantsDenoiser: boolean;
}

export interface DenoiserStatus {
  supported: boolean;
  installed: boolean;
  path: string | null;
  bytes: number | null;
  repo: string;
  sha256: string;
}

/**
 * Inkvec Studio Lite only: where the denoiser's own download and start stand. The browser
 * fetches it in the background when the page opens (unless the browser asks to save data),
 * and a trace that wants it runs without it until it is `ready`, then again with it. The
 * desktop never sends this; its denoiser is a download the user starts, reported by
 * `denoiser:progress` and `denoiser:done`.
 */
export interface DenoiserFetch {
  /**
   * `idle`: not fetched and not being fetched (skipped to save data, cancelled, or not
   * started yet); `downloading`; `stored`: in this browser's storage, not started;
   * `preparing`: the runtime and session starting; `ready`; `failed`.
   */
  phase: "idle" | "downloading" | "stored" | "preparing" | "ready" | "failed";
  got: number;
  total: number | null;
  /** Why it failed, in words. */
  message?: string;
  /** The background download was skipped because the browser asked to save data. */
  saveData?: boolean;
  /** A trace ran without the denoiser while it was not ready: trace again now that it is. */
  retrace?: boolean;
}

export interface Capabilities {
  version: string;
  engineVersion: string;
  buildTarget: string;
  platform: string;
  controls: Control[];
  presets: PresetInfo[];
  stages: string[];
  denoiser: DenoiserStatus;
}

export type Theme = "system" | "dark" | "light";

/**
 * A preset the user saved. Unlike the built-in seven it is a whole snapshot of the
 * controls, and its id is a string rather than a `PresetId` — saved presets are a
 * Vectorize-tab affordance and never reach the batch queue, whose CSV column has to say
 * which of the seven a row was measured with.
 */
export interface SavedPreset {
  id: string;
  name: string;
  settings: Settings;
}

/** What opening an image offers besides the automatic trace, which always starts at once. */
export type OnOpen = "ask" | "auto" | "custom";

/**
 * The interface as it was left (`Interface` in `studio/core/src/interface.rs`): remembered
 * as it changes, put back before the first paint. Nothing about the image on screen.
 */
export interface InterfacePrefs {
  preset: string | null;
  tab: "vectorize" | "minify" | "fabricate" | "batch";
  railTab: "result" | "tune";
  groupsOpen: Record<string, boolean>;
  view: "side" | "wipe" | "ab";
  wipe: number;
  show: { fill: boolean; wireframe: boolean; anchors: boolean; handles: boolean; certainty: boolean };
  detail: boolean;
  export: Formats | null;
  minify: MinifySettings | null;
  minifyBackdrop: "auto" | "light" | "dark";
  minifyView: "side" | "wipe" | "ab";
  /** The Fabricate request without one drawing's colours (`include`, `order`). */
  fab: Partial<FabOptions> | null;
  fabUnit: "mm" | "in";
  fabShowProblems: boolean;
  fabPreset: string | null;
  fabDxf: boolean;
  fabGcode: boolean;
  batchPreset: string | null;
  batchSkipExisting: boolean;
  batchFailuresFirst: boolean;
}

export interface Prefs {
  /** The schema the preferences were written with; the backend brings older ones forward. */
  schema: number;
  outputFolder: string | null;
  theme: Theme;
  threads: number | null;
  draftPx: number;
  draftSeconds: number;
  settleMs: number;
  checkUpdates: boolean;
  channel: "stable" | "prerelease";
  trace: Settings;
  recent: string[];
  /** Whether the first-run introduction to the wizard has been seen. */
  seenFirstRun: boolean;
  onOpen: OnOpen;
  saved: SavedPreset[];
  /** Absent from a backend older than this interface (the dev mock): read with defaults. */
  ui?: InterfacePrefs;
  /** The desktop window's place. The desktop backend owns it; the interface passes it through. */
  window?: unknown;
}

/** What the wizard knows about the open image that a trace's report does not say. */
export interface SourceFacts {
  /** Pixel noise in 8-bit levels; a clean render reads about 0.5. */
  noiseLevels: number;
  hasAlpha: boolean;
  /** Share of the canvas that is mostly clear. */
  clearShare: number;
}

// ------------------------------------------------------------------------ traces ---

export interface SourceInfo {
  name: string;
  path: string | null;
  width: number;
  height: number;
  container: string;
  lossy: boolean;
  preview: string;
}

export interface SampleInfo {
  file: string;
  label: string;
  preview: string;
}

/**
 * How editable the drawing is, counted from the SVG itself: the three habits of a
 * hand-drawn file (handles on the axes, smooth joins, nodes that share a coordinate),
 * as the counts behind each ratio. Mirrors `inkvec_svgmin::Structure`.
 */
export interface Structure {
  nodes: number;
  cubics: number;
  handles: number;
  axisHandles: number;
  joins: number;
  smoothJoins: number;
  alignedNodes: number;
}

export interface Report {
  meanDe00: number | null;
  medianDe00: number | null;
  worstDe00: number | null;
  coordinates: number;
  paths: number;
  segments: number;
  colours: number;
  bytes: number;
  minifiedBytes: number | null;
  structure: Structure;
  seconds: number;
  tracedPx: number;
}

export interface Ink {
  /** The measured colour; a gradient's first stop. */
  traced: string;
  /** What it is painted as now; a gradient's first stop. */
  hex: string;
  share: number;
  snappedDe00: number | null;
  kind: "flat" | "gradient";
  /** Every `fill` / `stroke` value that paints with it: `#aabbcc` or `url(#g12)`. */
  keys: string[];
  /** Stop colours in offset order; `[hex]` for a flat ink. */
  stops: string[];
  gradient?: "linear" | "radial";
}

export interface Loss {
  kind: string;
  text: string;
  why: string;
  link: { label: string; href: string } | null;
}

export interface WorstCorner {
  x: number;
  y: number;
  de00: number;
}

export interface Stage {
  name: string;
  ms: number;
}

export interface Traced {
  tier: "draft" | "final";
  svg: string;
  report: Report;
  palette: Ink[];
  losses: Loss[];
  worstCorner: WorstCorner | null;
  stages: Stage[];
  engineLog: string[];
  /**
   * Each boundary's confidence band, an SVG in the drawing's coordinates (colour traces).
   * Null when the backend keeps them to itself, to be fetched with `api.traceBands`.
   */
  bands: string | null;
  tracedPx: number;
  oversized: boolean;
  sourcePx: [number, number];
}

export type Outcome =
  | ({ state: "traced" } & Traced)
  | { state: "flat" }
  | { state: "undecodable"; message: string }
  | { state: "outOfMemory"; neededGb: number; suggestPx: number }
  | { state: "failed"; message: string };

// --------------------------------------------------------------------- fabricate ---

export type FabMode = "singleColour" | "layered" | "inlay" | "sticker" | "stencil" | "lines";
export type CutStyle = "filled" | "hairline";
export type FileUnits = "mm" | "px96" | "px72";

/** A fabrication request; lengths in millimetres. Mirrors `inkvec_fab::Options`. */
export interface FabOptions {
  mode: FabMode;
  widthMm: number;
  include: string[];
  order: string[];
  bleedMm: number;
  minFeatureMm: number;
  removeThin: boolean;
  stickerMarginMm: number;
  bridgeMm: number;
  stencilMarginMm: number;
  /** Widest part drawn as one line in "lines" mode; 0 for any width. */
  maxLineMm: number;
  /** The pen or tool line width in "lines" mode. */
  penMm: number;
  registration: boolean;
  weedBorderMm: number;
  mirror: boolean;
  cutStyle: CutStyle;
  /** Side of a square cut beside the design, to measure after cutting; 0 for none. */
  sizeCheckMm: number;
  /** Router bit radius for dogbones at inside corners; 0 for none. */
  dogboneMm: number;
  /** G-code cutting speed, mm per minute. */
  gcodeFeedMmMin: number;
  /** G-code power, in the controller's S units. */
  gcodePower: number;
  /** Times the G-code cuts each path. */
  gcodePasses: number;
  /** How the saved files state their size. */
  fileUnits: FileUnits;
  kerfMm: number;
  toleranceMm: number;
  mergeDeltaE: number;
}

export interface FabColour {
  hex: string;
  coverage: number;
  background: boolean;
  gradient: boolean;
  translucent: boolean;
}

export interface FabAnalysis {
  sizePx: [number, number];
  aspect: number;
  colours: FabColour[];
  items: number;
  nodes: number;
  unsupported: string[];
}

export interface FabCheck {
  level: "info" | "warn" | "error";
  code: string;
  message: string;
}

export interface FabLayer {
  name: string;
  hex: string;
  svg: string;
  nodes: number;
  parts: number;
  areaMm2: number;
  materialMm: [number, number];
}

export interface FabPlan {
  layers: FabLayer[];
  previewSvg: string;
  problemsSvg: string;
  combinedSvg: string;
  dxf: string;
  gcode: string;
  sizeMm: [number, number];
  checks: FabCheck[];
}

// ------------------------------------------------------------------------ minify ---

export interface MinifySettings {
  tolerancePx: number;
  judgePx: number;
  cornerDegrees: number;
  documentCleanup: boolean;
}

export interface MinifyResult {
  svg: string;
  bytesBefore: number;
  bytesAfter: number;
  numbersBefore: number;
  numbersAfter: number;
  pathsBefore: number;
  pathsAfter: number;
  segmentsBefore: number;
  segmentsAfter: number;
  primitives: number;
  guarded: number;
  toleranceUnits: number;
  differenceDe00: number | null;
  ms: number;
  removed: { what: string; amount: string }[];
}

// ------------------------------------------------------------------------- batch ---

export type RowState = "queued" | "running" | "done" | "failed" | "skipped";

export interface BatchRow {
  id: number;
  path: string;
  file: string;
  preset: PresetId;
  state: RowState;
  de00: number | null;
  coordinates: number | null;
  outBytes: number | null;
  destination: string;
  message: string | null;
  seconds: number | null;
}

export interface BatchTotals {
  finished: number;
  total: number;
  failed: number;
  skipped: number;
  bytesWritten: number;
  sourceBytes: number;
  meanDe00: number | null;
  elapsed: number;
  remaining: number | null;
}

export interface BatchPlan {
  files: string[];
  preset: PresetId;
  overrides: [number, PresetId][];
  outputDir: string;
  skipExisting: boolean;
}

// ------------------------------------------------------------------------ export ---

export interface Formats {
  svg: boolean;
  svgMinified: boolean;
  pngSizes: number[];
  favicon: boolean;
  assetPack: boolean;
}

export interface PlannedFile {
  name: string;
  bytes: number;
  group: string;
}

export interface ExportRequest {
  svg: string;
  report: { meanDe00: number | null; coordinates: number; paths: number; tracedPx: number };
  palette: { hex: string; traced: string; share: number; stops?: string[] }[];
  losses: { text: string }[];
  formats: Formats;
}

/** Where a desktop integration stands: the `inkvec` command, the context-menu entry. */
export interface IntegrationStatus {
  available: boolean;
  installed: boolean;
  path: string | null;
  note: string | null;
}

export interface UpdateInfo {
  latest: string | null;
  newer: boolean;
  url: string;
  offline: string | null;
}

// -------------------------------------------------------------------- the calls ---

export const api = {
  capabilities: () => invoke<Capabilities>("capabilities"),
  /** One real step of start-up, for the splash window's status line and bar. */
  startupProgress: (text: string, progress: number) => invoke<void>("startup_progress", { text, progress }),
  /** The interface is ready: swap the splash for the app. */
  appReady: () => invoke<void>("app_ready"),
  openPath: (path: string) => invoke<SourceInfo>("open_path", { path }),
  /** A Uint8Array only in the browser build; Tauri's JSON arguments want a plain array. */
  openBytes: (bytes: number[] | Uint8Array, name?: string) =>
    invoke<SourceInfo>("open_bytes", { bytes, name: name ?? null }),
  openSample: (name: string) => invoke<SourceInfo>("open_sample", { name }),
  listSamples: () => invoke<SampleInfo[]>("list_samples"),

  startTrace: (settings: Settings, tier: "draft" | "final") =>
    invoke<number>("start_trace", { request: { settings, tier } }),
  cancelTrace: () => invoke<void>("cancel_trace"),
  /**
   * A wizard preview: a draft of `settings` that does not become the drawing. It never
   * retires the trace in flight and waits behind it; the result is a `preview:done` event.
   */
  startPreview: (settings: Settings) =>
    invoke<number>("start_preview", { request: { settings, tier: "draft" } }),
  /** Retire every preview in flight or waiting; the viewer's trace is not touched. */
  cancelPreviews: () => invoke<void>("cancel_previews"),
  /** Noise and transparency of the open image, from the raster a trace at `maxDim` reads. */
  sourceFacts: (maxDim: number) => invoke<SourceFacts>("source_facts", { maxDim }),
  /** The image the app was launched to open (the context menu), once. */
  launchPath: () => invoke<string | null>("launch_path"),
  /**
   * The confidence bands of trace `generation`, asked for only when Certainty is shown.
   * Megabytes on a detailed drawing, so they are not in every result; null once the
   * backend no longer holds that trace, or when it had none.
   */
  traceBands: (generation: number) => invoke<string | null>("trace_bands", { generation }),

  /**
   * Paint `svg` (a trace's own drawing) with every snap at once. A snap names its ink by
   * the colour it was measured as, and still finds it in a re-trace that measured it a
   * hair apart (`api::SNAP_REACH_DE00`).
   */
  snapInks: (svg: string, snaps: Snap[], width: number, height: number) =>
    invoke<{ svg: string; inks: Ink[] }>("snap_inks", { svg, snaps, width, height }),
  matchPalette: (traced: string[], pasted: string) =>
    invoke<{ from: string; to: string; de00: number }[]>("match_palette", { traced, pasted }),

  planExport: (request: ExportRequest) => invoke<PlannedFile[]>("plan_export", { request }),
  writeExport: (request: ExportRequest, destination: string) =>
    invoke<string[]>("write_export", { request, destination }),

  minify: (svg: string, settings: MinifySettings) =>
    invoke<MinifyResult>("minify_svg", { svg, settings }),
  fabAnalyze: (svg: string) => invoke<FabAnalysis>("fab_analyze", { svg }),
  fabPrepare: (svg: string, options: FabOptions) => invoke<FabPlan>("fab_prepare", { svg, options }),
  readTextFile: (path: string) => invoke<string>("read_text_file", { path }),
  saveBytes: (path: string, bytes: number[]) => invoke<void>("save_bytes", { path, bytes }),

  batchScan: (folder: string) => invoke<string[]>("batch_scan", { folder }),
  batchStart: (plan: BatchPlan) => invoke<void>("batch_start", { plan }),
  batchPause: (paused: boolean) => invoke<void>("batch_pause", { paused }),
  batchCancel: () => invoke<void>("batch_cancel"),
  batchStatsCsv: () => invoke<string>("batch_stats_csv"),

  denoiserStatus: () => invoke<DenoiserStatus>("denoiser_status"),
  denoiserDownload: () => invoke<void>("denoiser_download"),
  denoiserCancel: () => invoke<void>("denoiser_cancel"),
  denoiserRemove: () => invoke<DenoiserStatus>("denoiser_remove"),

  loadPrefs: () => invoke<Prefs>("load_prefs"),
  savePrefs: (prefs: Prefs) => invoke<Prefs>("save_prefs", { prefs }),
  resetPrefs: () => invoke<Prefs>("reset_prefs"),

  thirdPartyNotices: () => invoke<string>("third_party_notices"),
  checkUpdate: () => invoke<UpdateInfo>("check_update"),

  cliStatus: () => invoke<IntegrationStatus>("cli_status"),
  installCli: () => invoke<IntegrationStatus>("install_cli"),
  removeCli: () => invoke<IntegrationStatus>("remove_cli"),
  contextMenuStatus: () => invoke<IntegrationStatus>("context_menu_status"),
  installContextMenu: () => invoke<IntegrationStatus>("install_context_menu"),
  removeContextMenu: () => invoke<IntegrationStatus>("remove_context_menu"),
};

// ----------------------------------------------------------------------- events ---

export const events = {
  traceStage: (fn: (e: { generation: number; name: string; ms: number }) => void) =>
    listen<{ generation: number; name: string; ms: number }>("trace:stage", (e) => fn(e.payload)),
  traceDone: (fn: (e: { generation: number; outcome: Outcome }) => void) =>
    listen<{ generation: number; outcome: Outcome }>("trace:done", (e) => fn(e.payload)),
  previewDone: (fn: (e: { generation: number; outcome: Outcome }) => void) =>
    listen<{ generation: number; outcome: Outcome }>("preview:done", (e) => fn(e.payload)),
  /** A second launch asked this window to open a file (the context menu, while running). */
  openPath: (fn: (path: string) => void) => listen<string>("open:path", (e) => fn(e.payload)),
  batchRow: (fn: (row: BatchRow) => void) =>
    listen<BatchRow>("batch:row", (e) => fn(e.payload)),
  batchTotals: (fn: (t: BatchTotals) => void) =>
    listen<BatchTotals>("batch:totals", (e) => fn(e.payload)),
  batchFinished: (fn: (rows: BatchRow[]) => void) =>
    listen<BatchRow[]>("batch:finished", (e) => fn(e.payload)),
  denoiserProgress: (fn: (e: { got: number; total: number | null }) => void) =>
    listen<{ got: number; total: number | null }>("denoiser:progress", (e) => fn(e.payload)),
  denoiserDone: (
    fn: (e: { ok: boolean; message?: string; status: DenoiserStatus }) => void,
  ) =>
    listen<{ ok: boolean; message?: string; status: DenoiserStatus }>("denoiser:done", (e) =>
      fn(e.payload),
    ),
  /** Inkvec Studio Lite only (see `DenoiserFetch`); the desktop never sends it. */
  denoiserFetch: (fn: (e: DenoiserFetch) => void) => listen<DenoiserFetch>("denoiser:fetch", (e) => fn(e.payload)),
};

export type { UnlistenFn };
