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

const params = new URLSearchParams(location.search);

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
  // `?fresh=1` is a first launch: the introduction to the wizard shows.
  seenFirstRun: params.get("fresh") !== "1",
  // `?onopen=auto|custom` is a remembered choice; the default asks.
  onOpen: params.get("onopen") ?? "ask",
  saved: [],
};

// `?launch=C:/path/logo.png` is the file the app was launched with (the context menu).
let launchPath: string | null = params.get("launch");

// Every command the app sent, with when, so a test can time the open-to-trace path.
const mockLog: { cmd: string; t: number }[] = [];
(window as unknown as { __mockLog: typeof mockLog }).__mockLog = mockLog;
// A second launch while the app is open: what the single-instance plugin forwards.
(window as unknown as { __mockSecondLaunch: (path: string) => Promise<void> }).__mockSecondLaunch = (path: string) =>
  emit("open:path", path);
// A file dropped on the window: what the webview reports for a drag and drop.
(window as unknown as { __mockDrop: (paths: string[]) => Promise<void> }).__mockDrop = (paths: string[]) =>
  emit("tauri://drag-drop", { paths, position: { x: 400, y: 300 } });

let current = "flat-logo.png";
let currentLossy = false;
let generation = 0;
let previewGeneration = 0;
/** The interactive trace in flight, which a preview waits behind as the backend's do. */
let mainTrace: Promise<void> = Promise.resolve();
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

/**
 * A preview draft, roughly as the engine would draw it: the sample's own trace, recoloured
 * to two inks for Black & white and outlined for Line art, so the tiles differ visibly.
 * Mock-only, like everything here.
 */
function previewSvg(settings: Record<string, unknown>): string {
  const stem = current.replace(".png", "");
  let svg = svgOf(settings.editability && current !== EMOJI ? `${stem}.edit` : stem);
  if (settings.blackAndWhite) {
    svg = svg.replace(/(fill|stop-color)="#([0-9a-fA-F]{6})"/g, (_m, attr: string, hex: string) => {
      const [r, g, b] = [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16));
      return `${attr}="${0.2126 * r + 0.7152 * g + 0.0722 * b < 150 ? "#111111" : "#ffffff"}"`;
    });
  }
  if (settings.lineArt) {
    svg = svg.replace(/<path([^>]*?)fill="(#[0-9a-fA-F]{6})"/g, '<path$1fill="none" stroke="$2" stroke-width="3"');
  }
  return svg;
}

async function runPreview(gen: number, settings: Record<string, unknown>) {
  // Behind whatever the viewer is waiting for, then about a draft's time.
  await mainTrace;
  if (gen !== previewGeneration) return;
  await new Promise((r) => setTimeout(r, 260 + Math.random() * 180));
  if (gen !== previewGeneration) return;
  const svg = previewSvg(settings);
  const structure = (structures as Record<string, unknown>)[`${current.replace(".png", "")}:default`];
  await emit("preview:done", {
    generation: gen,
    outcome: {
      state: "traced",
      tier: "draft",
      svg,
      report: { ...report(svg, 512, structure, Boolean(settings.editability)), meanDe00: settings.blackAndWhite ? 1.8 : 0.12 },
      palette: [],
      losses: [],
      worstCorner: null,
      stages: [],
      engineLog: [],
      bands: null,
      tracedPx: 512,
      oversized: false,
      sourcePx: [512, 512],
    },
  });
}

mockWindows("main");
mockIPC(
  (cmd, args) => {
    const a = (args ?? {}) as Record<string, any>;
    mockLog.push({ cmd, t: performance.now() });
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
        currentLossy = false;
        const stem = a.name.replace(".png", "");
        const side = stem === "icon-64" ? 64 : stem === "peach-emoji" ? 128 : 512;
        return { name: a.name, path: null, width: side, height: side, container: "PNG", lossy: false, preview: png(stem) };
      }
      case "open_path": {
        // A JPEG path opens as a lossy photo of the crest, so the Clean-up step has a reason
        // to appear; anything else is the flat logo.
        const jpeg = /\.jpe?g$/i.test(a.path);
        current = jpeg ? "crest-filigree.png" : "flat-logo.png";
        currentLossy = jpeg;
        const name = String(a.path).split(/[\\/]/).pop() ?? "image.png";
        return {
          name,
          path: a.path,
          width: 512,
          height: 512,
          container: jpeg ? "JPEG" : "PNG",
          lossy: jpeg,
          preview: png(jpeg ? "crest-filigree" : "flat-logo"),
        };
      }
      case "open_bytes":
        // The Minify tab's "rebuild": an SVG re-traced from its render. The flat logo stands in.
        current = "flat-logo.png";
        currentLossy = false;
        return { name: a.name ?? "pasted image", path: null, width: 512, height: 512, container: "PNG", lossy: false, preview: png("flat-logo") };
      case "start_trace": {
        const gen = ++generation;
        mainTrace = runTrace(a.request.tier, gen, a.request.settings);
        return gen;
      }
      case "start_preview": {
        const gen = ++previewGeneration;
        void runPreview(gen, a.request.settings);
        return gen;
      }
      case "cancel_previews":
        previewGeneration++;
        return null;
      case "source_facts":
        return {
          noiseLevels: currentLossy ? 3.4 : 0.5,
          hasAlpha: current === EMOJI || current === "icon-64.png",
          clearShare: current === EMOJI ? 0.41 : current === "icon-64.png" ? 0.3 : 0,
        };
      case "launch_path": {
        const path = launchPath;
        launchPath = null;
        return path;
      }
      case "cancel_trace":
        return null;
      case "trace_bands":
        return bandsByGeneration.get(a.generation) ?? null;
      case "snap_inks": {
        // Every snap at once, as `api::snap_inks` does: A→B and B→C paint A as B, not C.
        const to = new Map((a.snaps as { from: string; to: string }[]).map((s) => [s.from.toLowerCase(), s.to.toLowerCase()]));
        const svg = (a.svg as string).replace(/ (fill|stroke)="([^"]*)"/g, (all, attr, v) =>
          to.has(v.toLowerCase()) ? ` ${attr}="${to.get(v.toLowerCase())}"` : all,
        );
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
