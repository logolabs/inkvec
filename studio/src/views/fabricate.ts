/**
 * The Fabricate tab: turn a drawing into sheets a cutter can use.
 *
 * Vinyl in one colour or stacked in several, colours laid edge to edge, a print-then-cut
 * sticker, or cut lines for a laser. The engine (`inkvec-fab`) does the geometry in
 * millimetres; this tab asks the questions a person at a cutter actually has — what
 * material, how big, which colours, in what order — and shows, before anything is cut,
 * what will go wrong and where: a gradient that has to become one vinyl, a sliver too thin
 * to weed (drawn in red on the sheet), a file the cutter's software will refuse.
 *
 * It takes the drawing the Vectorize tab just traced or any SVG, and it is fast enough
 * (tens of milliseconds) to rebuild the sheets on every change, as Minify does.
 */

import { fill, h, icon } from "../lib/dom";
import { copyText, pickFiles, readPickedText, revealAction, saveFiles, WEB, type Picked } from "../lib/platform";
import { api, type FabLayer, type FabMode, type FabOptions, type FabPlan, type FileUnits } from "../lib/ipc";
import { count, percent, type Store } from "../lib/state";
import { toast } from "../components/overlays";
import { art } from "./minify";

const MM_PER_IN = 25.4;

const MODES: [FabMode, string, string][] = [
  ["singleColour", "One colour", "Everything chosen, joined into one sheet of one material."],
  [
    "layered",
    "Layered",
    "One sheet per colour. Each runs under the colours above it by the bleed, so the stack has no gaps; registration marks line the sheets up.",
  ],
  ["inlay", "Inlay", "One sheet per colour, cut exactly, laid edge to edge. Thinner, but unforgiving to line up."],
  ["sticker", "Sticker", "The artwork with a contour around it, to print and then cut."],
  [
    "stencil",
    "Stencil",
    "A sheet with the artwork cut out. Counters that would fall out, like the middle of an O, are held by thin bridges.",
  ],
  [
    "lines",
    "Lines",
    "For a pen, a scoring blade or a laser line: every line in the drawing is followed once, along its centre, instead of round both of its edges. Strokes in the file are used as drawn; lines inside filled shapes are found and checked against the shape.",
  ],
];

/** How the saved files state their size, and which programs read each correctly. */
const FILE_UNITS: [FileUnits, string, string][] = [
  ["mm", "Millimetres", "Exact by the SVG standard, and read correctly by most programs. If yours imports at the wrong size, choose the pixel rate it uses."],
  ["px96", "Pixels, 96/in", "The CSS pixel: Inkscape, LightBurn, Carbide Create, Fusion and browsers read a bare pixel size at 96 per inch."],
  ["px72", "Pixels, 72/in", "Cricut Design Space and Silhouette Studio read a bare pixel size at 72 per inch."],
];

/** What people actually put in the machine, each a set of choices made for them. */
const PRESETS: { id: string; label: string; glyph: string; note: string; apply: (inks: number) => Partial<FabOptions> }[] = [
  {
    id: "vinyl",
    label: "Adhesive vinyl",
    glyph: "layers",
    note: "Cricut, Silhouette, Brother. Filled shapes, stacked by colour with a 0.8 mm overlap, marks to line up, nothing narrower than 0.8 mm.",
    apply: (inks) => ({ mode: inks > 1 ? "layered" : "singleColour", cutStyle: "filled", mirror: false, registration: true, bleedMm: 0.8, minFeatureMm: 0.8, kerfMm: 0 }),
  },
  {
    id: "htv",
    label: "Iron-on",
    glyph: "zap",
    note: "Heat-transfer vinyl is cut face down, so every sheet is mirrored. Layers overlap by 0.3 mm (0.25–0.38 mm is the press-safe range) and nothing is narrower than 1 mm. Glitter, holographic and puff go on top only.",
    apply: (inks) => ({ mode: inks > 1 ? "layered" : "singleColour", cutStyle: "filled", mirror: true, registration: true, bleedMm: 0.3, minFeatureMm: 1.0, kerfMm: 0 }),
  },
  {
    id: "sticker",
    label: "Sticker",
    glyph: "square",
    note: "Print the artwork, then the machine cuts the contour around it.",
    apply: () => ({ mode: "sticker", cutStyle: "filled", mirror: false, stickerMarginMm: 3, kerfMm: 0 }),
  },
  {
    id: "stencil",
    label: "Stencil",
    glyph: "copy",
    note: "Mylar or card with the design cut out, for paint or etching cream. Loose centres are bridged.",
    apply: () => ({ mode: "stencil", cutStyle: "filled", mirror: false, bridgeMm: 1.5, stencilMarginMm: 10, minFeatureMm: 1.5, kerfMm: 0 }),
  },
  {
    id: "pen",
    label: "Pen or score",
    glyph: "pen",
    note: "A pen in the cutter, a scoring or foil tool, a plotter, or a laser engraving lines: each line is drawn once, down its middle. Set the tip width so the preview shows what the pen will leave.",
    apply: () => ({ mode: "lines", penMm: 0.4, maxLineMm: 0, mirror: false }),
  },
  {
    id: "laser",
    label: "Laser or sign cutter",
    glyph: "crosshair",
    note: "Hairlines in LightBurn's layer colours, inner shapes cut first, parts grown for a 0.15 mm kerf (CO₂ 0.1–0.2 mm, diode 0.15–0.3 mm; measure yours). A DXF is saved too.",
    apply: () => ({ mode: "inlay", cutStyle: "hairline", mirror: false, registration: false, kerfMm: 0.15, minFeatureMm: 0.3 }),
  },
];

export function createFabricate(store: Store): HTMLElement {
  const tools = h("div.viewertools");
  const pane = h("div.pane", null);
  // Sheets are seen as they will be cut: on a light mat, where a thin red cut line and a
  // pale vinyl both show.
  const panes = h("div.panes.onlight", null, pane);
  const stage = h("section.stage", null, tools, panes);
  const rail = h("aside.minifyrail");
  const el = h("div", { style: { flex: "1", minHeight: "0", display: "flex" } }, stage, rail);

  let pending = 0;
  const run = async () => {
    const f = store.state.fab;
    if (!f.svg) return;
    const token = ++pending;
    try {
      const plan = await api.fabPrepare(f.svg, f.options);
      if (token !== pending) return;
      f.plan = plan;
      f.error = null;
      if (f.shown >= plan.layers.length) f.shown = -1;
    } catch (e) {
      if (token !== pending) return;
      f.plan = null;
      f.error = String(e);
    }
    store.touch("fab");
  };

  /** Take a drawing: measure it, choose sensible colours, and build the sheets. */
  const take = async (name: string, svg: string) => {
    const f = store.state.fab;
    f.name = name;
    f.svg = svg;
    f.plan = null;
    f.error = null;
    f.shown = -1;
    try {
      const a = await api.fabAnalyze(svg);
      f.analysis = a;
      const inks = a.colours.filter((c) => !c.background).map((c) => c.hex);
      f.options.include = inks;
      // Largest at the bottom: how a traced logo's colours stack in nearly every case.
      f.options.order = [...inks];
      // More than one ink is almost always meant to be cut in layers.
      if (inks.length > 1 && f.options.mode === "singleColour") f.options.mode = "layered";
    } catch (e) {
      f.analysis = null;
      f.error = String(e);
    }
    store.touch("fab");
    await run();
  };

  const openSvg = async () => {
    const [picked] = await pickFiles([{ name: "SVG", extensions: ["svg"] }]);
    if (!picked) return;
    try {
      await take(picked.name, await readPickedText(picked));
    } catch (e) {
      toast(String(e), { kind: "bad" });
    }
  };

  const useTrace = async () => {
    const st = store.state;
    if (!st.svg) return;
    await take(st.source?.name ? `${st.source.name.replace(/\.[^.]+$/, "")}.svg` : "trace.svg", st.svg);
  };

  const acceptDropped = async (picked: Picked) => {
    if (!picked.name.toLowerCase().endsWith(".svg")) return false;
    await take(picked.name, await readPickedText(picked));
    return true;
  };
  (el as HTMLElement & { acceptDropped?: typeof acceptDropped }).acceptDropped = acceptDropped;

  const change = (patch: Partial<FabOptions>, keepPreset = false) => {
    const f = store.state.fab;
    Object.assign(f.options, patch);
    if (!keepPreset) f.preset = null;
    store.touch("fab");
    void run();
  };

  const render = () => {
    const f = store.state.fab;
    renderStage(store, tools, pane);
    if (!f.svg) {
      fill(rail, emptyRail(store, openSvg, useTrace));
      return;
    }
    fill(
      rail,
      h(
        "div.railscroll",
        { style: { gap: "16px", padding: "20px" } },
        sourceLine(store, openSvg, useTrace),
        f.error ? h("div.card", null, h("span.eyebrow", null, "This file could not be prepared"), h("span.faint", null, f.error)) : null,
        presetCard(store, change),
        modeCard(store, change),
        sizeCard(store, change),
        coloursCard(store, change),
        optionsCard(store, change),
        preflightCard(store),
        cutListCard(store),
      ),
      footer(store),
    );
  };

  store.on(["fab"], render);
  // The trace can change under the tab; offer it again when it does.
  store.on(["svg"], () => {
    if (store.state.tab === "fabricate") render();
  });
  render();
  return el;
}

/** A length in the chosen unit, for reading. */
function len(store: Store, mm: number, digits = 1): string {
  return store.state.fab.unit === "in" ? `${(mm / MM_PER_IN).toFixed(digits + 1)} in` : `${mm.toFixed(digits)} mm`;
}

/**
 * A preflight sentence in the chosen unit. The engine measures and writes millimetres;
 * someone working in inches should read inches, areas included.
 */
function inUnit(store: Store, message: string): string {
  if (store.state.fab.unit !== "in") return message;
  return message
    .replace(/(\d+(?:\.\d+)?) mm²/g, (_m, n) => `${(Number(n) / (MM_PER_IN * MM_PER_IN)).toFixed(4)} in²`)
    .replace(/(\d+(?:\.\d+)?) mm(?![²\w])/g, (_m, n) => `${(Number(n) / MM_PER_IN).toFixed(3)} in`);
}

/** A width by height in the chosen unit. */
function size2(store: Store, wh: [number, number]): string {
  return store.state.fab.unit === "in"
    ? `${(wh[0] / MM_PER_IN).toFixed(2)} × ${(wh[1] / MM_PER_IN).toFixed(2)} in`
    : `${wh[0].toFixed(1)} × ${wh[1].toFixed(1)} mm`;
}

/** Problems the overlay draws: the thin-part and speck findings. */
function problemCount(plan: FabPlan | null): number {
  return plan ? plan.checks.filter((c) => c.code === "thin" || c.code === "gaps" || c.code === "specks").length : 0;
}

/** Before anything is open the stage itself is the invitation, as it is on the other tabs. */
function emptyStage(): HTMLElement {
  return h(
    "div.drop.solid",
    null,
    h("span.glyph", null, icon("layers", 32)),
    h(
      "div",
      { style: { display: "flex", flexDirection: "column", gap: "6px" } },
      h("span.headline", null, "Drop an SVG here"),
      h("span.faint", { style: { fontSize: "13px" } }, "or open one, or bring over a trace, from the panel on the right"),
    ),
  );
}

function swatch(hex: string, size = 10): HTMLElement {
  return h("span", {
    style: {
      display: "inline-block",
      width: `${size}px`,
      height: `${size}px`,
      borderRadius: "2px",
      background: hex,
      border: "1px solid var(--rule2)",
      flex: "none",
    },
  });
}

function renderStage(store: Store, tools: HTMLElement, pane: HTMLElement): void {
  const f = store.state.fab;
  const plan = f.plan;
  const problems = problemCount(plan);
  const pick = (i: number) => {
    f.shown = i;
    store.touch("fab");
  };
  fill(
    tools,
    h(
      "div.seg",
      { role: "group", "aria-label": "Sheet" },
      h("button", { "aria-pressed": String(f.shown === -1), disabled: !plan, onclick: () => pick(-1) }, "All sheets"),
      ...(plan?.layers ?? []).map((l: FabLayer, i: number) =>
        h(
          "button",
          {
            "aria-pressed": String(f.shown === i),
            title: `${l.name}: ${size2(store, l.materialMm)} of material, ${l.parts} pieces, ${count(l.nodes)} path segments`,
            onclick: () => pick(i),
          },
          swatch(l.hex),
          h("span", { style: { marginLeft: "6px" } }, (plan?.layers.length ?? 0) > 1 ? `${i + 1}` : l.name),
        ),
      ),
    ),
    plan && problems
      ? h(
          "button.btn.ghost.compact",
          {
            "aria-pressed": String(f.showProblems),
            style: { marginLeft: "12px", color: f.showProblems ? "#e5484d" : undefined },
            title: "Draw the parts too thin to weed (red), the waste gaps too narrow to weed (amber) and the loose specks (ringed)",
            onclick: () => {
              f.showProblems = !f.showProblems;
              store.touch("fab");
            },
          },
          icon("alert", 13),
          h("span", { style: { marginLeft: "6px" } }, f.showProblems ? "Showing problems" : "Show problems"),
        )
      : null,
    h("span.muted", { style: { marginLeft: "auto", fontSize: "11.5px" } }, plan
        ? `${size2(store, plan.sizeMm)}${
            f.options.mode === "lines"
              ? ""
              : f.options.registration && plan.layers.length > 1 && (f.options.mode === "layered" || f.options.mode === "inlay")
                ? ", marks included"
                : f.options.weedBorderMm > 0
                  ? ", border included"
                  : ""
          }`
        : ""),
  );
  const lines = f.options.mode === "lines";
  // Lines mode shows what the pen draws, over a ghost of the drawing it came from.
  const shown = plan ? (f.shown >= 0 ? plan.layers[f.shown]?.svg : lines ? plan.combinedSvg : plan.previewSvg) : null;
  // The overlay shares the sheets' size and origin, and the same grid cell as the sheet,
  // so the two fit the pane identically and line up at any size.
  const cell = (node: HTMLElement, over = false) => {
    node.style.gridArea = "1 / 1";
    if (over) {
      node.style.pointerEvents = "none";
      node.style.opacity = "0.85";
    }
    return node;
  };
  const layered = shown
    ? h(
        "div",
        { style: { display: "grid", placeSelf: "stretch", minWidth: "0", minHeight: "0" } },
        lines && plan && f.shown < 0 ? ghost(cell(art(plan.previewSvg))) : null,
        cell(art(shown)),
        plan && problems && f.showProblems ? cell(art(plan.problemsSvg), true) : null,
      )
    : f.svg
      ? art(f.svg)
      : emptyStage();
  fill(
    pane,
    h(
      "span.eyebrow.panelabel",
      null,
      f.shown >= 0 && plan
        ? `${lines ? "Pen" : "Sheet"} ${f.shown + 1} · ${plan.layers[f.shown].name} · ${size2(store, plan.layers[f.shown].materialMm)}`
        : lines
          ? "What the pen draws, over the drawing"
          : "All sheets, stacked",
    ),
    layered,
  );
}

/** What one output file is called: a sheet of material, or one pen's drawing. */
function sheetWord(store: Store, n: number): string {
  const word = store.state.fab.options.mode === "lines" ? "pen drawing" : "sheet";
  return n === 1 ? word : `${word}s`;
}

/** A layer drawn faintly, for reference under another. */
function ghost(node: HTMLElement): HTMLElement {
  node.style.opacity = "0.18";
  node.style.pointerEvents = "none";
  return node;
}

function emptyRail(store: Store, openSvg: () => void, useTrace: () => void): HTMLElement {
  const traced = !!store.state.svg;
  return h(
    "div.railscroll",
    { style: { justifyContent: "center", alignItems: "center", textAlign: "center", gap: "16px" } },
    h("span.serif", { style: { fontSize: "20px" } }, "Make it real"),
    h(
      "span.faint",
      { style: { fontSize: "12.5px", lineHeight: "1.6", maxWidth: "32ch" } },
      "Prepare a drawing for a vinyl cutter, a sticker or a laser: real sizes, one sheet per colour, and a check of what will go wrong before anything is cut.",
    ),
    traced ? h("button.btn.primary", { onclick: useTrace }, "Use the current trace") : null,
    h(traced ? "button.btn" : "button.btn.primary", { onclick: openSvg }, "Open an SVG"),
  );
}

function sourceLine(store: Store, openSvg: () => void, useTrace: () => void): HTMLElement {
  const f = store.state.fab;
  return h(
    "div",
    { style: { display: "flex", alignItems: "center", gap: "8px" } },
    icon("file", 14),
    h("span.dim", { style: { flex: "1", fontSize: "12.5px", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" } }, f.name ?? "drawing"),
    store.state.svg ? h("button.btn.ghost.compact", { onclick: useTrace, title: "Use what the Vectorize tab traced" }, "Current trace") : null,
    h("button.btn.ghost.compact", { onclick: openSvg }, "Open…"),
  );
}

function presetCard(store: Store, change: (p: Partial<FabOptions>, keepPreset?: boolean) => void): HTMLElement {
  const f = store.state.fab;
  const inks = f.options.include.length;
  const chosen = PRESETS.find((p) => p.id === f.preset);
  return h(
    "div.card",
    null,
    h("span.eyebrow", null, "Material"),
    h(
      "div",
      { style: { display: "grid", gridTemplateColumns: "1fr 1fr", gap: "6px" } },
      ...PRESETS.map((p) =>
        h(
          "button.btn",
          {
            "aria-pressed": String(f.preset === p.id),
            style: {
              justifyContent: "flex-start",
              gap: "8px",
              borderColor: f.preset === p.id ? "var(--accent)" : undefined,
              background: f.preset === p.id ? "var(--accent-soft)" : undefined,
            },
            onclick: () => {
              f.preset = p.id;
              f.dxf = p.id === "laser" || p.id === "pen";
              change(p.apply(inks), true);
            },
          },
          icon(p.glyph, 14),
          p.label,
        ),
      ),
    ),
    h("span.muted", { style: { fontSize: "11.5px", lineHeight: "1.5" } }, chosen ? chosen.note : "Pick what goes in the machine, or set everything below by hand."),
  );
}

function modeCard(store: Store, change: (p: Partial<FabOptions>) => void): HTMLElement {
  const o = store.state.fab.options;
  const about = MODES.find((m) => m[0] === o.mode)?.[2] ?? "";
  return h(
    "div.card",
    null,
    h("span.eyebrow", null, "Making"),
    h(
      "div.seg",
      // Six choices do not fit the rail in one row: three by two, each the same width.
      { role: "group", "aria-label": "What is being made", style: { display: "grid", gridTemplateColumns: "repeat(3, 1fr)" } },
      ...MODES.map(([id, label]) => h("button", { "aria-pressed": String(o.mode === id), onclick: () => change({ mode: id }) }, label)),
    ),
    h("span.muted", { style: { fontSize: "11.5px", lineHeight: "1.5" } }, about),
  );
}

/** A small number field that applies on change. */
function numField(value: number, apply: (n: number) => void, width = "58px", min = 0): HTMLInputElement {
  return h("input", {
    class: "inlinefield",
    type: "text",
    inputmode: "decimal",
    value: String(value),
    style: { width },
    onchange: (e: Event) => {
      const n = Number((e.target as HTMLInputElement).value);
      if (Number.isFinite(n) && n >= min) apply(n);
    },
  }) as HTMLInputElement;
}

/** A length field in the chosen unit, applying millimetres. */
function lenField(store: Store, mm: number, apply: (mm: number) => void, width = "58px"): HTMLInputElement {
  const inches = store.state.fab.unit === "in";
  const shown = inches ? Number((mm / MM_PER_IN).toFixed(3)) : Number(mm.toFixed(2));
  return numField(shown, (n) => apply(inches ? n * MM_PER_IN : n), width);
}

function unitWord(store: Store): string {
  return store.state.fab.unit === "in" ? "in" : "mm";
}

function sizeCard(store: Store, change: (p: Partial<FabOptions>) => void): HTMLElement {
  const f = store.state.fab;
  const aspect = f.analysis?.aspect ?? 1;
  const w = f.options.widthMm;
  const u = unitWord(store);
  return h(
    "div.card",
    null,
    h(
      "div.cardhead",
      null,
      h("span.eyebrow", null, "Size"),
      h(
        "div.seg",
        { role: "group", "aria-label": "Units" },
        ...(["mm", "in"] as const).map((unit) =>
          h(
            "button",
            {
              "aria-pressed": String(f.unit === unit),
              onclick: () => {
                f.unit = unit;
                store.touch("fab");
              },
            },
            unit,
          ),
        ),
      ),
    ),
    h(
      "span.sentence",
      null,
      "The design is ",
      lenField(store, w, (mm) => mm > 0 && change({ widthMm: mm })),
      ` ${u} wide and `,
      lenField(store, w * aspect, (mm) => mm > 0 && change({ widthMm: mm / aspect })),
      ` ${u} tall.`,
    ),
    switchRow(
      "Cut a size-check square",
      f.options.sizeCheckMm > 0,
      () => change({ sizeCheckMm: f.options.sizeCheckMm > 0 ? 0 : f.unit === "in" ? MM_PER_IN : 20 }),
      f.options.sizeCheckMm > 0
        ? `A ${len(store, f.options.sizeCheckMm, 1)} square below the design. Measure it once cut: any other size means the program rescaled the file on import.`
        : "Wrong size on import is the commonest cutter complaint. A small square below the design shows it before a whole sheet is wasted.",
    ),
    h(
      "div",
      { style: { display: "flex", flexDirection: "column", gap: "6px" } },
      h("span.dim", { style: { fontSize: "12.5px" } }, "The files state their size in"),
      h(
        "div.seg",
        { role: "group", "aria-label": "Size written in", style: { display: "grid", gridTemplateColumns: "repeat(3, 1fr)" } },
        ...FILE_UNITS.map(([id, label]) =>
          h("button", { "aria-pressed": String(f.options.fileUnits === id), onclick: () => change({ fileUnits: id }) }, label),
        ),
      ),
      h("span.muted", { style: { fontSize: "11px", lineHeight: "1.45" } }, FILE_UNITS.find((x) => x[0] === f.options.fileUnits)?.[2] ?? ""),
    ),
  );
}

function coloursCard(store: Store, change: (p: Partial<FabOptions>) => void): HTMLElement | null {
  const f = store.state.fab;
  const a = f.analysis;
  if (!a) return null;
  const o = f.options;
  const stacked = o.mode === "layered";
  // In layered mode the list is the stack, bottom first; otherwise it is the file's order.
  const listed = stacked
    ? [...o.order.filter((x) => o.include.includes(x)), ...a.colours.map((c) => c.hex).filter((x) => !o.include.includes(x))]
    : a.colours.map((c) => c.hex);
  const info = (hex: string) => a.colours.find((c) => c.hex === hex);
  const move = (hex: string, by: number) => {
    const order = o.order.filter((x) => o.include.includes(x));
    const i = order.indexOf(hex);
    const j = i + by;
    if (i < 0 || j < 0 || j >= order.length) return;
    [order[i], order[j]] = [order[j], order[i]];
    change({ order });
  };
  return h(
    "div.card",
    null,
    h(
      "div.cardhead",
      null,
      h("span.eyebrow", null, stacked ? "Colours, bottom sheet first" : "Colours"),
      h("span.muted", { style: { fontSize: "11px" } }, `${o.include.length} of ${a.colours.length}`),
    ),
    ...listed.map((hex) => {
      const c = info(hex);
      if (!c) return null;
      const on = o.include.includes(hex);
      const badges = [c.background ? "page" : null, c.gradient ? "gradient" : null, c.translucent ? "translucent" : null].filter(Boolean);
      return h(
        "div",
        { style: { display: "flex", alignItems: "center", gap: "8px", fontSize: "12.5px", opacity: on ? "1" : "0.55" } },
        h("button.switch", {
          role: "switch",
          "aria-checked": String(on),
          "aria-label": `Cut ${hex}`,
          onclick: () => {
            const include = on ? o.include.filter((x) => x !== hex) : [...o.include, hex];
            const order = on ? o.order : [...o.order.filter((x) => x !== hex), hex];
            change({ include, order });
          },
        }),
        swatch(hex, 16),
        h("span.num", { style: { flex: "1" } }, hex, badges.length ? h("span.muted", { style: { marginLeft: "6px", fontSize: "11px" } }, badges.join(" · ")) : null),
        h("span.muted.num", { style: { fontSize: "11px" } }, percent(c.coverage, 1)),
        stacked && on
          ? h(
              "span",
              { style: { display: "flex", gap: "2px" } },
              h("button.btn.ghost.compact", { title: "Lower in the stack", onclick: () => move(hex, -1) }, "↓"),
              h("button.btn.ghost.compact", { title: "Higher in the stack", onclick: () => move(hex, 1) }, "↑"),
            )
          : null,
      );
    }),
  );
}

function switchRow(label: string, on: boolean, flip: () => void, note?: string): HTMLElement {
  return h(
    "div",
    { style: { display: "flex", flexDirection: "column", gap: "3px" } },
    h(
      "div",
      { style: { display: "flex", alignItems: "center", justifyContent: "space-between" } },
      h("span.dim", { style: { fontSize: "12.5px" } }, label),
      h("button.switch", { role: "switch", "aria-checked": String(on), "aria-label": label, onclick: flip }),
    ),
    note ? h("span.muted", { style: { fontSize: "11px", lineHeight: "1.45" } }, note) : null,
  );
}

/** The options that matter when drawing lines: the pen, what counts as a line, the DXF. */
function linesCard(store: Store, change: (p: Partial<FabOptions>) => void): HTMLElement {
  const o = store.state.fab.options;
  const u = unitWord(store);
  return h(
    "div.card",
    null,
    h("span.eyebrow", null, "Drawing"),
    h("span.sentence", null, "The pen or tool draws ", lenField(store, o.penMm, (mm) => mm > 0 && change({ penMm: mm })), ` ${u} wide.`),
    h(
      "span.sentence",
      null,
      "Lines up to ",
      lenField(store, o.maxLineMm, (mm) => change({ maxLineMm: mm })),
      ` ${u} wide are drawn once (0 for any width); wider parts are drawn round their outline.`,
    ),
    switchRow(
      "Also save a DXF",
      store.state.fab.dxf,
      () => {
        store.state.fab.dxf = !store.state.fab.dxf;
        store.touch("fab");
      },
      "Lines as open polylines and outlines as closed ones, one layer per colour, in millimetres.",
    ),
    gcodeRows(store, change),
    h("span.sentence", null, "Lines stay within ", lenField(store, o.toleranceMm, (mm) => mm > 0 && change({ toleranceMm: mm })), ` ${u} of the drawing.`),
  );
}

/** The G-code switch and, when it is on, what the machine is told. */
function gcodeRows(store: Store, change: (p: Partial<FabOptions>) => void): HTMLElement {
  const f = store.state.fab;
  const o = f.options;
  const perMin = f.unit === "in" ? "in/min" : "mm/min";
  const speed = f.unit === "in" ? Number((o.gcodeFeedMmMin / MM_PER_IN).toFixed(1)) : Math.round(o.gcodeFeedMmMin);
  return h(
    "div",
    { style: { display: "flex", flexDirection: "column", gap: "8px" } },
    switchRow(
      "Also save G-code",
      f.gcode,
      () => {
        f.gcode = !f.gcode;
        store.touch("fab");
      },
      "For GRBL lasers and plotters: curves as G2/G3 arcs, holes cut first, origin at the lower left of the job. GRBL's laser mode ($32=1) keeps the beam off between cuts.",
    ),
    f.gcode
      ? h(
          "span.sentence",
          null,
          "Cut at ",
          numField(speed, (n) => n > 0 && change({ gcodeFeedMmMin: f.unit === "in" ? n * MM_PER_IN : n }), "64px"),
          ` ${perMin}, power S`,
          numField(o.gcodePower, (n) => change({ gcodePower: n }), "58px"),
          ", ",
          numField(o.gcodePasses, (n) => n >= 1 && change({ gcodePasses: Math.round(n) }), "40px", 1),
          o.gcodePasses === 1 ? " pass." : " passes.",
        )
      : null,
  );
}

function optionsCard(store: Store, change: (p: Partial<FabOptions>) => void): HTMLElement {
  const o = store.state.fab.options;
  if (o.mode === "lines") return linesCard(store, change);
  const u = unitWord(store);
  const sheets = o.mode === "layered" || o.mode === "inlay";
  return h(
    "div.card",
    null,
    h("span.eyebrow", null, "Cutting"),
    o.mode === "layered"
      ? h("span.sentence", null, "Each colour runs ", lenField(store, o.bleedMm, (mm) => change({ bleedMm: mm })), ` ${u} under the colours above it.`)
      : null,
    o.mode === "sticker"
      ? h("span.sentence", null, "The contour is ", lenField(store, o.stickerMarginMm, (mm) => change({ stickerMarginMm: mm })), ` ${u} outside the artwork.`)
      : null,
    o.mode === "stencil"
      ? h(
          "span.sentence",
          null,
          "Bridges are ",
          lenField(store, o.bridgeMm, (mm) => mm > 0 && change({ bridgeMm: mm })),
          ` ${u} wide; the sheet reaches `,
          lenField(store, o.stencilMarginMm, (mm) => change({ stencilMarginMm: mm })),
          ` ${u} past the artwork.`,
        )
      : null,
    h(
      "span.sentence",
      null,
      "Parts narrower than ",
      lenField(store, o.minFeatureMm, (mm) => mm > 0 && change({ minFeatureMm: mm })),
      ` ${u} are too thin to weed.`,
    ),
    switchRow("Remove them", o.removeThin, () => change({ removeThin: !o.removeThin }), "Drops the slivers and necks narrower than that from the sheets."),
    sheets ? switchRow("Registration marks", o.registration, () => change({ registration: !o.registration })) : null,
    h("span.sentence", null, "Weeding border ", lenField(store, o.weedBorderMm, (mm) => change({ weedBorderMm: mm })), ` ${u} outside (0 for none).`),
    switchRow(
      "Also save a DXF",
      store.state.fab.dxf,
      () => {
        store.state.fab.dxf = !store.state.fab.dxf;
        store.touch("fab");
      },
      "Every sheet as a layer of closed polylines in millimetres, curves as true arcs (which CAM turns into G2/G3 moves), for CAD, CNC and laser programs.",
    ),
    gcodeRows(store, change),
    o.mode === "sticker" || o.mode === "stencil"
      ? null
      : switchRow("Mirror", o.mirror, () => change({ mirror: !o.mirror }), "For heat-transfer vinyl, which is cut from the back."),
    h(
      "div",
      { style: { display: "flex", flexDirection: "column", gap: "6px" } },
      h("span.dim", { style: { fontSize: "12.5px" } }, "Cut lines as"),
      h(
        "div.seg",
        { role: "group", "aria-label": "Cut lines as" },
        h("button", { "aria-pressed": String(o.cutStyle === "filled"), onclick: () => change({ cutStyle: "filled" }) }, "Filled shapes"),
        h("button", { "aria-pressed": String(o.cutStyle === "hairline"), onclick: () => change({ cutStyle: "hairline" }) }, "Hairlines"),
      ),
      h(
        "span.muted",
        { style: { fontSize: "11px", lineHeight: "1.45" } },
        o.cutStyle === "filled"
          ? "What Cricut Design Space and Silhouette Studio expect: they cut around every shape."
          : "The laser and sign-cutter convention: only a hairline is a cut. Each sheet keeps its colour, so a laser program can give it its own operation.",
      ),
    ),
    o.cutStyle === "hairline"
      ? h(
          "span.sentence",
          null,
          "The cut removes ",
          lenField(store, o.kerfMm, (mm) => change({ kerfMm: mm })),
          ` ${u} (kerf); parts are grown by half of it so they come out the drawn size.`,
        )
      : null,
    h(
      "span.sentence",
      null,
      "Router bit ",
      lenField(store, 2 * o.dogboneMm, (mm) => change({ dogboneMm: mm / 2 })),
      ` ${u} across (0 for none): every inside corner gets a dogbone so parts seat square.`,
    ),
    h("span.sentence", null, "Cuts stay within ", lenField(store, o.toleranceMm, (mm) => mm > 0 && change({ toleranceMm: mm })), ` ${u} of the drawing.`),
  );
}

function preflightCard(store: Store): HTMLElement | null {
  const plan = store.state.fab.plan;
  if (!plan) return null;
  const nodes = plan.layers.reduce((s, l) => s + l.nodes, 0);
  const parts = plan.layers.reduce((s, l) => s + l.parts, 0);
  const worst = plan.checks.some((c) => c.level === "error") ? "error" : plan.checks.some((c) => c.level === "warn") ? "warn" : "ok";
  return h(
    "div.card",
    { style: worst === "ok" ? {} : { borderColor: worst === "error" ? "#e5484d" : "var(--accent)" } },
    h(
      "div.cardhead",
      null,
      h("span.eyebrow", null, "Before you cut"),
      h("span.muted", { style: { fontSize: "11px" } }, worst === "ok" ? "ready" : `${plan.checks.length} finding${plan.checks.length === 1 ? "" : "s"}`),
    ),
    h(
      "div.stats",
      null,
      ...(
        [
          [sheetWord(store, 2), String(plan.layers.length)],
          ["pieces", count(parts)],
          ["path segments", count(nodes)],
          ["whole job", size2(store, plan.sizeMm)],
        ] as [string, string][]
      ).map(([k, v]) => h("div", null, h("span.k", null, k), h("span.v", null, v))),
    ),
    plan.checks.length
      ? h(
          "div",
          { style: { display: "flex", flexDirection: "column", gap: "8px" } },
          ...plan.checks.map((c) =>
            h(
              "div",
              { style: { display: "flex", gap: "8px", alignItems: "flex-start", fontSize: "12px", lineHeight: "1.5" } },
              h(
                "span",
                { style: { color: c.level === "error" ? "#e5484d" : c.level === "warn" ? "var(--accent-text)" : "var(--muted)", flex: "none", paddingTop: "2px" } },
                icon(c.level === "info" ? "info" : "alert", 14),
              ),
              h("span.faint", null, inUnit(store, c.message)),
            ),
          ),
        )
      : h(
          "div",
          { style: { display: "flex", gap: "8px", alignItems: "center", fontSize: "12.5px" } },
          icon("check", 14),
          h("span.dim", null, "Nothing here will trouble the cutter."),
        ),
  );
}

/** Each sheet with the piece of material it needs: the shopping list. */
function cutListCard(store: Store): HTMLElement | null {
  const plan = store.state.fab.plan;
  if (!plan || !plan.layers.length) return null;
  const f = store.state.fab;
  return h(
    "div.card",
    null,
    h("span.eyebrow", null, store.state.fab.options.mode === "lines" ? "Pens, one per colour" : "Cut list"),
    ...plan.layers.map((l, i) =>
      h(
        "button",
        {
          style: {
            display: "flex",
            alignItems: "center",
            gap: "8px",
            fontSize: "12.5px",
            background: "none",
            border: "none",
            padding: "2px 0",
            color: "inherit",
            cursor: "pointer",
            textAlign: "left",
            fontWeight: f.shown === i ? "600" : "400",
          },
          title: "Show this sheet",
          onclick: () => {
            f.shown = i;
            store.touch("fab");
          },
        },
        h("span.muted.num", { style: { width: "16px" } }, String(i + 1)),
        swatch(l.hex, 14),
        h("span.num", { style: { flex: "1" } }, l.name),
        h("span.dim.num", null, size2(store, l.materialMm)),
        h("span.muted.num", { style: { width: "54px", textAlign: "right", fontSize: "11px" } }, `${l.parts} pc`),
      ),
    ),
  );
}

function footer(store: Store): HTMLElement {
  const f = store.state.fab;
  const plan = f.plan;
  const stem = (f.name ?? "drawing").replace(/\.svg$/i, "");
  const fileName = (p: FabPlan, i: number) => {
    const safe = p.layers[i].name.replace(/[^A-Za-z0-9_-]+/g, "");
    return p.layers.length > 1 ? `${stem}-${i + 1}-${safe}.svg` : `${stem}-cut.svg`;
  };
  return h(
    "div.railfoot",
    null,
    h(
      "div.row",
      null,
      h(
        "button.btn.primary",
        {
          style: { flex: "1" },
          disabled: !plan || plan.layers.length === 0,
          onclick: async () => {
            if (!plan) return;
            const files: { name: string; data: string }[] = plan.layers.map((l, i) => ({ name: fileName(plan, i), data: l.svg }));
            // One file with every sheet as its own group: what cutter software turns into
            // layers on import, and often the only file anyone needs.
            if (plan.layers.length > 1) files.push({ name: `${stem}-all.svg`, data: plan.combinedSvg });
            if (f.dxf) files.push({ name: `${stem}.dxf`, data: plan.dxf });
            if (f.gcode) files.push({ name: `${stem}.gcode`, data: plan.gcode });
            // A folder on the desktop; in a browser one download, a .zip when there are several.
            const saved = await saveFiles(files, `${stem}-sheets.zip`, "Choose a folder for the sheets");
            if (!saved) return;
            const many = plan.layers.length > 1;
            const what = `${plan.layers.length} ${sheetWord(store, plan.layers.length)}${many ? " and one combined file" : ""}`;
            toast(saved.folder ? `Saved ${what} to ${saved.folder}` : `Downloaded ${what}${files.length > 1 ? " as one .zip" : ""}`, {
              kind: "good",
              action: revealAction(saved.first),
            });
          },
        },
        `${WEB ? "Download" : "Save"} ${plan && plan.layers.length > 1 ? `${plan.layers.length} ${sheetWord(store, 2)}` : `the ${sheetWord(store, 1)}`}${WEB ? "" : "…"}`,
      ),
      h(
        "button.btn",
        {
          disabled: !plan,
          title: "Copies the shown sheet, or every sheet in one file",
          onclick: async () => {
            if (!plan) return;
            await copyText(f.shown >= 0 ? plan.layers[f.shown].svg : plan.combinedSvg);
            toast("Copied. Paste it into Design Space, Silhouette Studio or LightBurn.");
          },
        },
        "Copy",
      ),
    ),
    h("div.note", null, h("span", null, plan ? `Real size: ${len(store, plan.sizeMm[0])} wide` : "Real sizes"), h("span", null, "Nothing leaves this computer")),
  );
}
