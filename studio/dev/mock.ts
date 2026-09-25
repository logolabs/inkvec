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
import { paletteOf, regroup } from "./mockpalette";

const samplePngs = import.meta.glob("../src-tauri/samples/*.png", { eager: true, query: "?url", import: "default" }) as Record<
  string,
  string
>;
const tracedSvgs = import.meta.glob("./traced/*.svg", { eager: true, query: "?raw", import: "default" }) as Record<
  string,
  string
>;

// The Fabricate tab's answers, written by the real engine for a traced twemoji unicorn at
// 60 mm, small enough to trip the thin-part and speck checks
// (`cargo run -p inkvec-fab --example fab` with FAB_JSON=1), one per mode.
const fabJson = import.meta.glob("./fab/*/*.json", { eager: true, import: "default" }) as Record<string, unknown>;
const fabUnicorn = (import.meta.glob("./fab/unicorn.svg", { eager: true, query: "?raw", import: "default" }) as Record<string, string>)["./fab/unicorn.svg"];
const FAB_DIR: Record<string, string> = { singleColour: "single", layered: "layered", inlay: "inlay", sticker: "sticker", stencil: "stencil", lines: "lines" };

// A noto emoji the real CLI traced (`inkvec emoji_u1f351.png -o peach.svg`): gradients, and
// flat inks a rounding apart, which is what colour groups are for. Mock-only; not a sample
// the app ships.
const emojiPngs = import.meta.glob("./emoji/*.png", { eager: true, query: "?url", import: "default" }) as Record<string, string>;
const emojiSvgs = import.meta.glob("./emoji/*.svg", { eager: true, query: "?raw", import: "default" }) as Record<string, string>;
const EMOJI = "peach-emoji.png";

const png = (name: string) => (name === "peach-emoji" ? emojiPngs["./emoji/peach.png"] : samplePngs[`../src-tauri/samples/${name}.png`]);
const svgOf = (name: string) =>
  name.startsWith("peach-emoji") ? emojiSvgs["./emoji/peach.svg"] ?? "" : tracedSvgs[`./traced/${name}.svg`] ?? "";
// Confidence bands for each sample, written by `inkvec <png> --uncertainty dev/bands/<name>.svg`.
const bandSvgs = import.meta.glob("./bands/*.svg", { eager: true, query: "?raw", import: "default" }) as Record<string, string>;

const SAMPLES = [
  { file: "flat-logo.png", label: "Flat logo" },
  { file: "crest-filigree.png", label: "Crest, filigree" },
  { file: "icon-64.png", label: "64 px icon" },
  { file: "signature-bw.png", label: "Signature" },
  { file: EMOJI, label: "Emoji, gradients" },
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
  traceTransparency: true,
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
// The bands of each trace, kept back as the backend keeps them and served by generation
// through `trace_bands` when the Certainty view asks. Exposed so a test can plant a
// drawing's bands under the generation it applies.
const bandsByGeneration = new Map<number, string | null>();
(window as unknown as { __mockBands: Map<number, string | null> }).__mockBands = bandsByGeneration;

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

async function runTrace(tier: string, gen: number, settings: Record<string, unknown>) {
  const draft = tier === "draft";
  for (const name of STAGES) {
    await new Promise((r) => setTimeout(r, draft ? 30 : 140));
    await emit("trace:stage", { generation: gen, name, ms: draft ? 12 : 60 + Math.random() * 300 });
  }
  const stem = current.replace(".png", "");
  const editable = Boolean(settings.editability);
  // Colour groups, roughly as the engine applies them; see `mockpalette.ts`.
  const grouped = await regroup(svgOf(editable && current !== EMOJI ? `${stem}.edit` : stem), (settings.colourGroups as never) ?? []);
  const svg = grouped.svg;
  const inks = await paletteOf(svg);
  const structure = (structures as Record<string, unknown>)[`${stem}:${editable ? "edit" : "default"}`];
  const tracedPx = draft ? 512 : 2048;
  bandsByGeneration.set(gen, bandSvgs[`./bands/${stem}.svg`] ?? null);
  // A few traces back, like the backend's cache: an older generation answers null.
  for (const old of [...bandsByGeneration.keys()]) if (old < gen - 4) bandsByGeneration.delete(old);
  await emit("trace:done", {
    generation: gen,
    outcome: {
      state: "traced",
      tier,
      svg,
      report: report(svg, tracedPx, structure, editable),
      palette: inks,
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
      engineLog: grouped.lines,
      bands: null,
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
        const side = stem === "icon-64" ? 64 : stem === "peach-emoji" ? 128 : 512;
        return { name: a.name, path: null, width: side, height: side, container: "PNG", lossy: false, preview: png(stem) };
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
      case "trace_bands":
        return bandsByGeneration.get(a.generation) ?? null;
      case "snap_inks": {
        let svg = a.svg as string;
        for (const s of a.snaps as { from: string; to: string }[]) svg = svg.split(`fill="${s.from}"`).join(`fill="${s.to}"`);
        return paletteOf(svg).then((inks) => {
          for (const ink of inks) {
            const s = (a.snaps as { from: string; to: string }[]).find((x) => ink.kind === "flat" && x.to === ink.hex);
            if (s) Object.assign(ink, { traced: s.from, snappedDe00: 1.2 });
          }
          return { svg, inks };
        });
      }
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
      case "fab_analyze":
        return fabJson["./fab/layered/analysis.json"];
      case "fab_prepare":
        return fabJson[`./fab/${FAB_DIR[a.options?.mode] ?? "layered"}/plan.json`];
      case "plugin:dialog|open":
        // An SVG picker gets the fixture drawing; any other picker is cancelled.
        return a.options?.filters?.some((f: { extensions: string[] }) => f.extensions.includes("svg")) ? "C:/mock/unicorn.svg" : null;
      case "read_text_file":
        return fabUnicorn;
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
