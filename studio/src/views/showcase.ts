/**
 * The Showcase screen: before and after, with the geometry showing.
 *
 * Twenty real brand logos and the benchmark's logos, icons and emoji, each traced by Inkvec
 * (the build named in the data) and by the other engines, side by side with the raster they
 * were given: fill, wireframe, every anchor and every handle, zoomable, all panes moving
 * together. Under it, the summary tables. The data is `web/showcase.json` and one
 * `web/showcase/<key>.json` per case, written by `tools/showcase_data.py` -- the same files
 * the Space's presentation page reads -- and bundled into the app so the screen works
 * offline: the index is a chunk loaded when the screen opens, each case a chunk loaded when
 * it is picked.
 */

import { fill, h } from "../lib/dom";
import type { Store } from "../lib/state";
import { openExternal } from "../lib/platform";
import { windowControls } from "../components/wincontrols";

interface EngineSummary {
  id: string;
  name: string;
  n: number;
  de_mean: number;
  de_median: number;
  dists: number;
  dino: number;
  geometry_mean: number;
  geometry_median: number;
  coordinates: number;
  seconds_median: number;
}

interface CaseMetrics {
  de1024: number;
  coordinates: number;
  geometry_ratio: number;
  paths: number;
  bytes: number;
}

interface Case {
  key: string;
  label: string;
  what: string;
  artist: { paths: number; coordinates: number; geometry_params: number };
  thumb: string;
  m: Record<string, CaseMetrics>;
}

interface CaseBody {
  input: string;
  svg: Record<string, string>;
}

interface ShowcaseData {
  traced: string;
  inkvec: { git: string };
  run: string;
  date: string;
  seed: string;
  cases: number;
  families: string[];
  size: number;
  gallery_engines: string[];
  engines: EngineSummary[];
  best_de_wins: Record<string, number>;
  pick_seed: string;
  brands: { cases: number; engines: EngineSummary[]; best_de_wins: Record<string, number> };
  corpus: { cases: number; engines: EngineSummary[]; best_de_wins: Record<string, number> };
  sets: { id: string; label: string; cases: Case[] }[];
}

/** Short names for the gallery's compare buttons. */
const SHORT: Record<string, string> = {
  inkvec: "Inkvec",
  "vtracer-1.0-simplify": "VTracer 1.0, best flags",
  "trazor-auto": "Trazor",
  "vtracer-1.0-default": "VTracer 1.0",
  "vtracer-default": "VTracer 0.6",
};

type Layer = "fill" | "wire" | "anchor" | "handle";

/**
 * Where the data comes from. The desktop app bundles it (a chunk for the index, one per
 * case). The browser build is served beside the Space's front page, which already serves the
 * same files at its root, so it fetches those rather than carrying a second copy.
 */
const CASE_FILES = __INKVEC_WEB__ ? {} : import.meta.glob("../../../web/showcase/*.json");
const fetchJson = (path: string) =>
  fetch(new URL(path, document.baseURI)).then((r) => {
    if (!r.ok) throw new Error(`${path}: HTTP ${r.status}`);
    return r.json();
  });

let data: Promise<ShowcaseData> | null = null;
function load(): Promise<ShowcaseData> {
  data ??= __INKVEC_WEB__
    ? (fetchJson("../showcase.json") as Promise<ShowcaseData>)
    : import("../../../web/showcase.json").then((m) => (m.default ?? m) as unknown as ShowcaseData);
  return data;
}

/** Each case's raster and SVGs, loaded when it is opened. */
function loadCase(key: string): Promise<CaseBody> {
  if (__INKVEC_WEB__) return fetchJson(`../showcase/${key}.json`) as Promise<CaseBody>;
  const loader = (CASE_FILES as Record<string, () => Promise<unknown>>)[`../../../web/showcase/${key}.json`];
  if (!loader) return Promise.reject(new Error(`no data for ${key}`));
  return loader().then((m) => ((m as { default?: unknown }).default ?? m) as CaseBody);
}

export function showcaseScreen(store: Store, close: () => void): HTMLElement {
  const body = h("div.screenbody", null, h("div.sc-loading.faint", null, "Loading the showcase…"));
  void load().then(
    (d) => fill(body, content(d)),
    (e) => fill(body, h("div.sc-loading.faint", null, `The showcase could not be loaded: ${String(e)}`)),
  );
  void store;
  return h(
    "div.screen",
    null,
    h(
      "div.screenbar",
      { "data-tauri-drag-region": "" },
      h("span", { style: { fontSize: "12.5px", fontWeight: "600" } }, "Showcase"),
      h("div.spacer"),
      h("button.btn.compact", { onclick: close }, "Done"),
      h("div.sep"),
      windowControls(),
    ),
    body,
  );
}

// ------------------------------------------------------------------ the page ---

function content(d: ShowcaseData): HTMLElement {
  let cur = d.sets[0].cases[0];
  let other = "vtracer-1.0-simplify";
  let layer: Layer = "fill";
  let token = 0;

  const viewer = createViewer();
  const chips = h("div.sc-chips");
  const caption = h("p.sc-caption");
  const list = h("div.sc-list");
  const tiles = new Map<string, HTMLElement>();
  const others = d.gallery_engines.filter((e) => e !== "inkvec");

  const seg = (items: [string, string][], current: () => string, pick: (k: string) => void) => {
    const el = h("div.seg");
    const paint = () => {
      for (const b of el.querySelectorAll("button")) b.setAttribute("aria-pressed", String(b.dataset.k === current()));
    };
    for (const [k, label] of items) {
      el.append(h("button", { "data-k": k, onclick: () => { pick(k); paint(); } }, label));
    }
    paint();
    return el;
  };

  const show = async () => {
    const c = cur;
    const mine = ++token;
    for (const [key, b] of tiles) b.setAttribute("aria-pressed", String(key === c.key));
    const isBrand = c.what.includes("brand") || c.key.startsWith("brand");
    fill(
      caption,
      h("b", null, c.label),
      ` · ${c.what}`,
      isBrand
        ? h(
            "span.faint",
            { style: { fontSize: "11px", marginLeft: "6px" }, title: "Nominative benchmark demonstration. All trademarks belong to their respective owners." },
            "(nominative benchmark)",
          )
        : null,
    );
    const us = c.m.inkvec;
    const them = c.m[other];
    fill(
      chips,
      h("span.sc-legend.faint", null, h("i.sc-dot.a"), "anchor", h("i.sc-dot.hd"), "control point"),
      chip(`Inkvec dE00 ${us.de1024.toFixed(3)}`, true),
      chip(`${SHORT[other]} dE00 ${them.de1024.toFixed(3)}`),
      chip(`Inkvec ${us.paths} paths · ${us.coordinates} coordinates · ${kb(us.bytes)}`, true),
      chip(`${SHORT[other]} ${them.paths} paths · ${them.coordinates} coordinates · ${kb(them.bytes)}`),
      chip(`The artist's file: ${c.artist.paths} paths · ${c.artist.coordinates} coordinates`),
    );
    const body = await loadCase(c.key);
    if (mine !== token) return;
    viewer.set(body.input, body.svg.inkvec, body.svg[other], "Inkvec", SHORT[other] ?? other);
    viewer.layer(layer);
  };

  // Every set in one panel that scrolls inside itself, as tall as the viewer beside it, a
  // heading per set; each thumbnail opens its case, and the arrow keys move through them.
  const order: Case[] = [];
  new ResizeObserver(() => {
    const hgt = viewer.el.getBoundingClientRect().height;
    if (hgt > 200) list.style.height = `${hgt}px`;
  }).observe(viewer.el);
  const pick = (c: Case, focus: boolean) => {
    cur = c;
    void show();
    const b = tiles.get(c.key);
    if (!b) return;
    // Scroll the list, never the screen: the selected case stays in view inside it.
    const pr = list.getBoundingClientRect();
    const br = b.getBoundingClientRect();
    const head = 34;
    if (br.top < pr.top + head) list.scrollTop -= pr.top + head - br.top;
    else if (br.bottom > pr.bottom) list.scrollTop += br.bottom - pr.bottom + 6;
    if (focus) b.focus({ preventScroll: true });
  };
  list.addEventListener("keydown", (e: KeyboardEvent) => {
    const at = order.findIndex((c) => c.key === cur.key);
    const steps: Record<string, number> = { ArrowDown: 3, ArrowUp: -3, ArrowRight: 1, ArrowLeft: -1, Home: -at, End: order.length - 1 - at };
    const step = steps[e.key];
    if (step === undefined) return;
    e.preventDefault();
    pick(order[Math.min(order.length - 1, Math.max(0, at + step))], true);
  });
  for (const s of d.sets) {
    list.append(h("div.sc-sethead", null, s.label, h("span", null, String(s.cases.length))));
    list.append(
      h(
        "div.sc-tiles",
        null,
        ...s.cases.map((c) => {
          const b = h(
            "button.sc-case",
            { onclick: () => pick(c, false), title: `${c.label} · ${c.what}` },
            h("span.sc-thumb", null, h("img", { src: c.thumb, alt: "", loading: "lazy" })),
            h("span.sc-name", null, c.label),
          );
          tiles.set(c.key, b);
          order.push(c);
          return b;
        }),
      ),
    );
  }

  const tools = h(
    "div.sc-tools",
    null,
    h("span.eyebrow", null, "Compare with"),
    seg(others.map((e) => [e, SHORT[e] ?? e]), () => other, (k) => { other = k; void show(); }),
    h("span.eyebrow", null, "Show"),
    seg(
      [["fill", "Fill"], ["wire", "Wireframe"], ["anchor", "Anchors"], ["handle", "Handles"]],
      () => layer,
      (k) => { layer = k as Layer; viewer.layer(layer); },
    ),
    h("span.eyebrow", null, "Zoom"),
    ...[1, 4, 12].map((z) => h("button.btn.compact.ghost", { onclick: () => viewer.zoom(z) }, `${z}×`)),
  );

  void show();

  const ink = d.engines.find((e) => e.id === "inkvec");
  const wins = d.best_de_wins.inkvec ?? 0;
  const bwins = d.brands.best_de_wins.inkvec ?? 0;
  const cwins = d.corpus.best_de_wins.inkvec ?? 0;
  return h(
    "div.sc-page",
    null,
    h(
      "div.sc-head",
      null,
      h("h1.serif", null, "Before and after, with the geometry showing"),
      h(
        "p.faint",
        null,
        `Real brand logos, the benchmark's cases and more icons and emoji, all in the list on the left, each traced by Inkvec (build ${d.inkvec.git}, ${d.traced}) and by the other engines. Left, the raster; middle, Inkvec's SVG; right, the engine you compare with. Scroll to zoom and drag to pan, all three together; Anchors shows every on-curve point, Handles every control point with its tangent.`,
      ),
    ),
    h("div.sc-main", null, list, h("div.sc-stagecol", null, tools, viewer.el, caption, chips)),
    h(
      "div.sc-results",
      null,
      h("h2.serif", null, "Inkvec vs the field"),
      h(
        "p.faint",
        null,
        `${d.cases} benchmark cases (real icons, emoji, synthetic probes and real brand logos; selection fixed from the seed ${d.seed}), each rendered from its source SVG at ${d.size} px, traced by every engine from that raster and scored against the render: CIEDE2000 colour difference, DISTS and DINOv3 perceptual similarity, and geometry against the artist's own file (1.00× is exactly the artist's number of parameters). Inkvec had the lowest colour difference on ${wins} of ${d.cases}; in the other sets, on ${bwins} of ${d.brands.cases} brand logos and ${cwins} of ${d.corpus.cases} more icons and emoji.`,
      ),
      ink
        ? h(
            "div.sc-bignums",
            null,
            bignum("median colour difference", ink.de_median.toFixed(3), "dE00; under 1 is invisible"),
            bignum("geometry vs the artist", `${ink.geometry_mean.toFixed(2)}×`, `mean; median ${ink.geometry_median.toFixed(2)}×`),
            bignum("DINOv3 similarity", ink.dino.toFixed(4), "1 is identical"),
            bignum("seconds per case", ink.seconds_median.toFixed(2), "median, native command line"),
          )
        : null,
      table(`${d.cases} benchmark cases, re-run at ${d.size} px`, d.engines),
      table(`${d.brands.cases} brand logos`, d.brands.engines),
      table(`${d.corpus.cases} more icons and emoji`, d.corpus.engines),
      h(
        "p.faint.sc-note",
        null,
        `Means over the cases except seconds (median). Every case is rendered at ${d.size} px on white, traced by every engine from that raster and scored at 1024 px: the pictures and the tables are the same traces. Inkvec was traced by the build named above; the other engines have not changed. The benchmark of record is the ${d.date} run at 512 px (out/${d.run}/results.json, bench/crosscompare_competitors.py); these tables re-run its cases at ${d.size} px, where a logo's fine detail survives. The brand logos beyond the first twenty, and the third set, were picked by the seed ${d.pick_seed} alone (hash order, a quota per kind), not by how any engine traces them. Computed by tools/showcase_data.py. `,
        h("a", { href: "#", onclick: (e: Event) => { e.preventDefault(); void openExternal("https://github.com/logolabs/inkvec#results"); } }, "Method and every case"),
      ),
      h(
        "p.faint.sc-note",
        { style: { marginTop: "10px", borderTop: "1px solid var(--rule2)", paddingTop: "10px", lineHeight: "1.6" } },
        h("b", { style: { color: "var(--dim)" } }, "Trademark notice & fair-use disclaimer: "),
        "All third-party trademarks, brand names, and logos displayed in this showcase are the property of their respective owners. Their depiction is solely for nominative, non-commercial educational benchmarking and comparative vectorization demonstration. Inkvec and LogoLabs are not affiliated with, sponsored by, or endorsed by these trademark holders.",
      ),
    ),
  );
}

function table(caption: string, rows: EngineSummary[]): HTMLElement {
  return h(
    "table.sc-table",
    null,
    h("caption", null, caption),
    h(
      "thead",
      null,
      h("tr", null, ...["Engine", "mean dE00", "median dE00", "DISTS", "DINO", "geometry", "coordinates", "seconds"].map((t) => h("th", null, t))),
    ),
    h(
      "tbody",
      null,
      ...rows.map((e) =>
        h(
          `tr${e.id === "inkvec" ? ".win" : ""}`,
          null,
          h("td", null, e.name),
          h("td.num", null, e.de_mean.toFixed(3)),
          h("td.num", null, e.de_median.toFixed(3)),
          h("td.num", null, e.dists.toFixed(4)),
          h("td.num", null, e.dino.toFixed(4)),
          h("td.num", null, `${e.geometry_mean.toFixed(2)}×`),
          h("td.num", null, Math.round(e.coordinates).toString()),
          h("td.num", null, e.seconds_median.toFixed(2)),
        ),
      ),
    ),
  );
}

const kb = (b: number) => `${(b / 1024).toFixed(1)} KB`;
const chip = (text: string, win = false) => h(`span.sc-chip${win ? ".win" : ""}`, null, text);
const bignum = (k: string, v: string, s: string) =>
  h("div.sc-bignum", null, h("span.eyebrow", null, k), h("span.v.serif", null, v), h("span.faint", null, s));

// ------------------------------------------------------------------ the viewer ---

interface Viewer {
  el: HTMLElement;
  set(input: string, ours: string, theirs: string, oursLabel: string, theirsLabel: string): void;
  layer(l: Layer): void;
  zoom(z: number): void;
}

/**
 * Three panes -- the raster, Inkvec's SVG, the other engine's -- sharing one camera. Zoom
 * resizes the camera box rather than scaling it, so the SVGs re-render crisp at any size;
 * only the pan is a transform.
 */
function createViewer(): Viewer {
  const panes = [0, 1, 2].map(() => {
    const cam = h("div.sc-cam");
    const tag = h("span.sc-tag");
    const pane = h("div.sc-pane", null, cam, tag);
    return { pane, cam, tag };
  });
  const el = h("div.sc-viewer", null, ...panes.map((p) => p.pane));
  const st = { z: 1, x: 0, y: 0 };
  let vectors: VectorLayer[] = [];

  const view = () => {
    for (const p of panes) {
      p.cam.style.transform = `translate(${st.x}px, ${st.y}px)`;
      p.cam.style.width = p.cam.style.height = `${st.z * 100}%`;
    }
  };
  let drag: [number, number] | null = null;
  for (const { pane } of panes) {
    pane.addEventListener(
      "wheel",
      (ev) => {
        ev.preventDefault();
        const r = pane.getBoundingClientRect();
        const mx = ev.clientX - r.left;
        const my = ev.clientY - r.top;
        const nz = Math.min(40, Math.max(1, st.z * Math.exp(-ev.deltaY * 0.0015)));
        const k = nz / st.z;
        st.x = mx - (mx - st.x) * k;
        st.y = my - (my - st.y) * k;
        st.z = nz;
        view();
      },
      { passive: false },
    );
    pane.addEventListener("pointerdown", (ev) => {
      drag = [ev.clientX - st.x, ev.clientY - st.y];
      pane.setPointerCapture(ev.pointerId);
    });
    pane.addEventListener("pointermove", (ev) => {
      if (!drag) return;
      st.x = ev.clientX - drag[0];
      st.y = ev.clientY - drag[1];
      view();
    });
    const up = () => (drag = null);
    pane.addEventListener("pointerup", up);
    pane.addEventListener("pointercancel", up);
    pane.addEventListener("dblclick", () => {
      st.z = 1;
      st.x = st.y = 0;
      view();
    });
  }

  return {
    el,
    set(input, ours, theirs, oursLabel, theirsLabel) {
      fill(panes[0].cam, h("img.sc-raster", { src: input, alt: "", draggable: "false" }));
      vectors = [vectorLayer(ours), vectorLayer(theirs)];
      fill(panes[1].cam, vectors[0].root);
      fill(panes[2].cam, vectors[1].root);
      panes[0].tag.textContent = "Raster";
      panes[1].tag.textContent = `${oursLabel} · ${vectors[0].anchors} anchors`;
      panes[2].tag.textContent = `${theirsLabel} · ${vectors[1].anchors} anchors`;
      view();
    },
    layer(l) {
      for (const v of vectors) v.set(l);
    },
    zoom(z) {
      const r = panes[0].pane.getBoundingClientRect();
      st.z = z;
      st.x = (r.width - r.width * z) / 2;
      st.y = (r.height - r.height * z) / 2;
      view();
    },
  };
}

interface VectorLayer {
  root: HTMLElement;
  anchors: number;
  set(l: Layer): void;
}

const SVG_NS = "http://www.w3.org/2000/svg";

function parseSvg(text: string): SVGSVGElement | null {
  const doc = new DOMParser().parseFromString(text, "image/svg+xml");
  const svg = doc.documentElement;
  if (!(svg instanceof SVGSVGElement)) return null;
  // Some engines write only a width and a height; without a viewBox the drawing would not
  // scale to the pane.
  if (!svg.getAttribute("viewBox")) {
    const w = parseFloat(svg.getAttribute("width") ?? "");
    const hgt = parseFloat(svg.getAttribute("height") ?? "");
    if (w > 0 && hgt > 0) svg.setAttribute("viewBox", `0 0 ${w} ${hgt}`);
  }
  svg.removeAttribute("width");
  svg.removeAttribute("height");
  svg.setAttribute("preserveAspectRatio", "xMidYMid meet");
  return document.importNode(svg, true);
}

/** A trace as three stacked layers: its fill, its wireframe, and its anchors and handles. */
function vectorLayer(text: string): VectorLayer {
  const fillSvg = parseSvg(text);
  const wire = parseSvg(text);
  const root = h("div.sc-layer");
  if (!fillSvg || !wire) return { root, anchors: 0, set: () => {} };
  wire.classList.add("sc-wire");
  const o = overlay(fillSvg);
  root.append(fillSvg, wire, o.svg);
  const set = (l: Layer) => {
    const geo = l === "anchor" || l === "handle";
    fillSvg.style.display = l === "wire" ? "none" : "";
    fillSvg.style.opacity = geo ? "0.35" : "1";
    wire.style.display = l === "fill" ? "none" : "";
    o.anchors.style.display = geo ? "" : "none";
    o.handles.style.display = o.handleDots.style.display = l === "handle" ? "" : "none";
  };
  set("fill");
  return { root, anchors: o.count, set };
}

/** Every anchor and handle of `src`'s paths, circles and ellipses, as an overlay SVG. */
function overlay(src: SVGSVGElement): { svg: SVGSVGElement; anchors: SVGPathElement; handles: SVGPathElement; handleDots: SVGPathElement; count: number } {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", src.getAttribute("viewBox") ?? "0 0 100 100");
  svg.setAttribute("preserveAspectRatio", "xMidYMid meet");
  svg.setAttribute("class", "sc-ov");
  let a = "";
  let hl = "";
  let hd = "";
  let count = 0;
  const f = (v: number) => v.toFixed(2);
  for (const p of src.querySelectorAll("path")) {
    const [tx, ty] = offsetOf(p);
    const g = parsePath(p.getAttribute("d") ?? "", tx, ty);
    count += g.anchors.length;
    for (const [x, y] of g.anchors) a += `M${f(x)} ${f(y)}h0.001`;
    for (const [[x0, y0], [x1, y1]] of g.handles) {
      hl += `M${f(x0)} ${f(y0)}L${f(x1)} ${f(y1)}`;
      hd += `M${f(x1)} ${f(y1)}h0.001`;
    }
  }
  for (const c of src.querySelectorAll("circle, ellipse")) {
    const [tx, ty] = offsetOf(c);
    a += `M${f(Number(c.getAttribute("cx") ?? 0) + tx)} ${f(Number(c.getAttribute("cy") ?? 0) + ty)}h0.001`;
    count++;
  }
  const mk = (d: string, cls: string) => {
    const p = document.createElementNS(SVG_NS, "path");
    p.setAttribute("d", d || "M0 0");
    p.setAttribute("class", cls);
    svg.append(p);
    return p;
  };
  const handles = mk(hl, "h");
  const handleDots = mk(hd, "hd");
  const anchors = mk(a, "a");
  return { svg, anchors, handles, handleDots, count };
}

/** The translation of an element and its ancestor groups (the only transform tracers write). */
function offsetOf(e: Element): [number, number] {
  let x = 0;
  let y = 0;
  for (let n: Element | null = e; n && n.tagName.toLowerCase() !== "svg"; n = n.parentElement) {
    const m = (n.getAttribute("transform") ?? "").match(/translate\(\s*(-?[\d.e+-]+)[ ,]*(-?[\d.e+-]+)?\s*\)/i);
    if (m) {
      x += parseFloat(m[1]);
      y += parseFloat(m[2] ?? "0");
    }
  }
  return [x, y];
}

type Pt = [number, number];

/** The on-curve points of a path, and each curve's control points with the point they pull. */
function parsePath(d: string, tx: number, ty: number): { anchors: Pt[]; handles: [Pt, Pt][] } {
  const toks = d.match(/[MLHVCSQTAZmlhvcsqtaz]|-?\d*\.?\d+(?:e[-+]?\d+)?/gi) ?? [];
  const anchors: Pt[] = [];
  const handles: [Pt, Pt][] = [];
  let i = 0;
  let cx = 0;
  let cy = 0;
  let sx = 0;
  let sy = 0;
  let cmd = "";
  let pc: "C" | "Q" | null = null;
  let px = 0;
  let py = 0;
  const num = () => parseFloat(toks[i++]);
  const push = (x: number, y: number) => anchors.push([x + tx, y + ty]);
  const hd = (ax: number, ay: number, bx: number, by: number) => handles.push([[ax + tx, ay + ty], [bx + tx, by + ty]]);
  while (i < toks.length) {
    if (/[A-Za-z]/.test(toks[i])) {
      cmd = toks[i++];
      if (cmd === "Z" || cmd === "z") {
        cx = sx;
        cy = sy;
        pc = null;
        continue;
      }
    }
    if (!cmd) break;
    const rel = cmd === cmd.toLowerCase();
    const C = cmd.toUpperCase();
    const ox = rel ? cx : 0;
    const oy = rel ? cy : 0;
    if (C === "M" || C === "L" || C === "T") {
      const x = num() + ox;
      const y = num() + oy;
      if (C === "T") {
        const x1 = pc === "Q" ? 2 * cx - px : cx;
        const y1 = pc === "Q" ? 2 * cy - py : cy;
        hd(cx, cy, x1, y1);
        hd(x, y, x1, y1);
        pc = "Q";
        px = x1;
        py = y1;
      } else pc = null;
      cx = x;
      cy = y;
      if (C === "M") {
        sx = x;
        sy = y;
        cmd = rel ? "l" : "L";
      }
      push(x, y);
    } else if (C === "H") {
      cx = num() + ox;
      push(cx, cy);
      pc = null;
    } else if (C === "V") {
      cy = num() + oy;
      push(cx, cy);
      pc = null;
    } else if (C === "C" || C === "S") {
      let x1: number;
      let y1: number;
      if (C === "C") {
        x1 = num() + ox;
        y1 = num() + oy;
      } else {
        x1 = pc === "C" ? 2 * cx - px : cx;
        y1 = pc === "C" ? 2 * cy - py : cy;
      }
      const x2 = num() + ox;
      const y2 = num() + oy;
      const x = num() + ox;
      const y = num() + oy;
      hd(cx, cy, x1, y1);
      hd(x, y, x2, y2);
      cx = x;
      cy = y;
      push(x, y);
      pc = "C";
      px = x2;
      py = y2;
    } else if (C === "Q") {
      const x1 = num() + ox;
      const y1 = num() + oy;
      const x = num() + ox;
      const y = num() + oy;
      hd(cx, cy, x1, y1);
      hd(x, y, x1, y1);
      cx = x;
      cy = y;
      push(x, y);
      pc = "Q";
      px = x1;
      py = y1;
    } else if (C === "A") {
      i += 5;
      cx = num() + ox;
      cy = num() + oy;
      push(cx, cy);
      pc = null;
    } else {
      i++;
    }
  }
  return { anchors, handles };
}
