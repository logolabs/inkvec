/**
 * A browser stand-in for the Rust backend, so the interface can be opened, looked at and
 * screenshotted in an ordinary browser without building the desktop app.
 *
 * `npm run mock` serves `dev/index.html`, which installs this before the app starts. The
 * traces are not live: each sample maps to an SVG the real CLI wrote (`dev/traced/`), and
 * the numbers in the report are counted from that SVG. The control table is the real one,
 * extracted from `options.rs` by `gen_controls.py`. Nothing here ships.
 */

import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";

import controls from "./controls.json";
import structures from "./structure.json";

const samplePngs = import.meta.glob("../src-tauri/samples/*.png", { eager: true, query: "?url", import: "default" }) as Record<
  string,
  string
>;
const tracedSvgs = import.meta.glob("./traced/*.svg", { eager: true, query: "?raw", import: "default" }) as Record<
  string,
  string
>;

const png = (name: string) => samplePngs[`../src-tauri/samples/${name}.png`];
const svgOf = (name: string) => tracedSvgs[`./traced/${name}.svg`] ?? "";

const SAMPLES = [
  { file: "flat-logo.png", label: "Flat logo" },
  { file: "crest-filigree.png", label: "Crest, filigree" },
  { file: "icon-64.png", label: "64 px icon" },
  { file: "signature-bw.png", label: "Signature" },
];

const STAGES = ["intake", "palette", "planar map", "boundary solve", "symmetry", "repair", "segments", "lambda", "wrote"];

const DEFAULTS = {
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
};

const PRESETS = [
  ["logo", "Logo", "The default", {}],
  ["icon", "Icon", "Small flat marks", { speckleFloor: 1, maxColours: 16 }],
  ["fine-detail", "Fine detail", "Filigree, crests", { precision: 0.05, traceSize: 2048 }],
  ["fewer-paths", "Fewer paths", "Smallest file", { fewerPaths: true, colourMerging: 0.07 }],
  ["photo-or-scan", "Photo or scan", "Photographed or screenshotted", { cleanUpDamage: "auto" }],
  ["black-and-white", "Black & white", "Stamps, signatures", { blackAndWhite: true }],
  ["line-art", "Line art", "Uniform-stroke drawings", { lineArt: true }],
  ["editable", "Editable", "Tidy nodes for an artist to edit", { editability: true }],
].map(([id, name, subtitle, patch]) => ({
  id,
  name,
  subtitle,
  settings: { ...DEFAULTS, ...(patch as object) },
  wantsDenoiser: id === "photo-or-scan",
}));

const prefs = {
  outputFolder: null,
  theme: (new URLSearchParams(location.search).get("theme") ?? "dark") as string,
  threads: null,
  draftPx: 512,
  draftSeconds: 1,
  settleMs: 800,
  checkUpdates: false,
  channel: "stable",
  trace: { ...DEFAULTS },
  recent: ["C:\\Brand\\northwind-logo.png", "C:\\Brand\\crest.jpg"],
  seenFirstRun: true,
  saved: [],
};

let current = "flat-logo.png";
let generation = 0;

function report(svg: string, tracedPx: number, structure: unknown, editable: boolean) {
  const nums = (svg.match(/-?\d+\.?\d*/g) ?? []).length;
  const paths = (svg.match(/<path/g) ?? []).length;
  const segments = (svg.match(/[CLcl]/g) ?? []).length;
  const fills = new Set((svg.match(/fill="#[0-9a-fA-F]{6}"/g) ?? []).map((f) => f.slice(6, 13)));
  return {
    // Editability costs a little colour accuracy, and the mock says so too.
    meanDe00: editable ? 0.131 : 0.094,
    medianDe00: 0.061,
    worstDe00: 0.71,
    coordinates: nums,
    paths,
    segments,
    colours: fills.size,
    bytes: svg.length,
    minifiedBytes: Math.round(svg.length * 0.88),
    structure,
    seconds: 1.42,
    tracedPx,
  };
}

function palette(svg: string) {
  const fills = [...new Set((svg.match(/fill="#[0-9a-fA-F]{6}"/g) ?? []).map((f) => f.slice(6, 13)))].slice(0, 8);
  return fills.map((hex, i) => ({ traced: hex, hex, share: Math.max(0.02, 0.5 / (i + 1)), snappedDe00: null }));
}

async function runTrace(tier: string, gen: number, settings: Record<string, unknown>) {
  const draft = tier === "draft";
  for (const name of STAGES) {
    await new Promise((r) => setTimeout(r, draft ? 30 : 140));
    await emit("trace:stage", { generation: gen, name, ms: draft ? 12 : 60 + Math.random() * 300 });
  }
  const stem = current.replace(".png", "");
  const editable = Boolean(settings.editability);
  const svg = svgOf(editable ? `${stem}.edit` : stem);
  const structure = (structures as Record<string, unknown>)[`${stem}:${editable ? "edit" : "default"}`];
  const tracedPx = draft ? 512 : 2048;
  await emit("trace:done", {
    generation: gen,
    outcome: {
      state: "traced",
      tier,
      svg,
      report: report(svg, tracedPx, structure, editable),
      palette: palette(svg),
      losses:
        current === "crest-filigree.png"
          ? [
              {
                kind: "detail",
                text: "Hairlines under 1 px in the filigree were merged into their neighbours.",
                why: "The source resolves them at less than one pixel, so there is no boundary to measure.",
                link: null,
              },
            ]
          : [],
      worstCorner: { x: 212, y: 148, de00: 0.71 },
      stages: [],
      engineLog: [],
      tracedPx,
      oversized: false,
      sourcePx: [512, 512],
    },
  });
}

mockWindows("main");
mockIPC(
  (cmd, args) => {
    const a = (args ?? {}) as Record<string, any>;
    switch (cmd) {
      case "capabilities":
        return {
          version: "0.1.5",
          engineVersion: "0.1.5",
          buildTarget: "x86_64-windows-msvc",
          platform: "windows",
          controls,
          presets: PRESETS,
          stages: STAGES,
          denoiser: { supported: true, installed: false, path: null, bytes: null, repo: "logolabs/inkvec-restorer", sha256: "" },
        };
      case "load_prefs":
      case "reset_prefs":
        return prefs;
      case "save_prefs":
        Object.assign(prefs, a.prefs);
        return prefs;
      case "list_samples":
        return SAMPLES.map((s) => ({ ...s, preview: png(s.file.replace(".png", "")) }));
      case "open_sample": {
        current = a.name;
        const stem = a.name.replace(".png", "");
        return { name: a.name, path: null, width: stem === "icon-64" ? 64 : 512, height: stem === "icon-64" ? 64 : 512, container: "PNG", lossy: false, preview: png(stem) };
      }
      case "open_path":
        current = "flat-logo.png";
        return { name: "northwind-logo.png", path: a.path, width: 512, height: 512, container: "PNG", lossy: false, preview: png("flat-logo") };
      case "start_trace": {
        const gen = ++generation;
        void runTrace(a.request.tier, gen, a.request.settings);
        return gen;
      }
      case "cancel_trace":
        return null;
      case "snap_inks":
        return { svg: a.svg, inks: palette(a.svg) };
      case "match_palette":
        return [];
      case "plan_export":
        return [
          { name: "logo.svg", bytes: 4210, group: "Vector" },
          { name: "logo.min.svg", bytes: 3702, group: "Vector" },
          { name: "logo-512.png", bytes: 21800, group: "Raster" },
        ];
      case "write_export":
        return [];
      case "minify_svg":
        return null;
      case "cli_status":
      case "context_menu_status":
        return { available: true, installed: false, path: null, note: null };
      case "denoiser_status":
        return { supported: true, installed: false, path: null, bytes: null, repo: "logolabs/inkvec-restorer", sha256: "" };
      case "third_party_notices":
        return "Third-party notices (mock).";
      case "check_update":
        return { latest: null, newer: false, url: "", offline: null };
      default:
        return null;
    }
  },
  { shouldMockEvents: true },
);

// The window-drag and window-control commands are no-ops in a browser.
(window as unknown as { __MOCKED__: boolean }).__MOCKED__ = true;
