/**
 * The backend, typed.
 *
 * Every shape here mirrors a `serde` struct in `src-tauri`. They are written out rather
 * than generated because there are about twenty of them and a code generator is a build
 * step somebody has to maintain; the backend's tests assert the field names, so a rename
 * on either side shows up as a type error here or a failing test there.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ---------------------------------------------------------------------- settings ---

export type Cleanup = "off" | "auto" | "on";

export interface Settings {
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
  kind: "range" | "switch" | "tri";
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

export interface Prefs {
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
  seenFirstRun: boolean;
  saved: SavedPreset[];
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
  traced: string;
  hex: string;
  share: number;
  snappedDe00: number | null;
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
  palette: { hex: string; traced: string; share: number }[];
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
  openBytes: (bytes: number[], name?: string) =>
    invoke<SourceInfo>("open_bytes", { bytes, name: name ?? null }),
  openSample: (name: string) => invoke<SourceInfo>("open_sample", { name }),
  listSamples: () => invoke<SampleInfo[]>("list_samples"),

  startTrace: (settings: Settings, tier: "draft" | "final") =>
    invoke<number>("start_trace", { request: { settings, tier } }),
  cancelTrace: () => invoke<void>("cancel_trace"),

  snapInks: (svg: string, snaps: { from: string; to: string }[], width: number, height: number) =>
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
};

export type { UnlistenFn };
