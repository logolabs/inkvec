/**
 * The mock's stand-ins for two pieces of Rust: the gradient-aware palette
 * (`quality::palette`) and the engine's colour groups (`inkvec_trace::regroup`).
 *
 * The palette is measured as the backend measures it: every ink's paint replaced by a colour
 * that stands for it alone, drawn without antialiasing, and the pixels counted. The groups
 * are only roughly what the engine does: the engine recolours the image before tracing, so
 * the shapes between merged inks really join; here the finished drawing's fills are
 * repainted, which is enough to see what a group does. Nothing here ships.
 */

import { memberDistance, memberOf } from "../src/lib/colour";
import type { ColourGroup, Ink } from "../src/lib/ipc";

const PAINTS = ["fill", "stroke"] as const;

function hex6(v: string): string | null {
  const h = v.trim().replace(/^#/, "");
  const full = h.length === 3 ? [...h].map((c) => c + c).join("") : h;
  return /^[0-9a-f]{6}$/i.test(full) ? `#${full.toLowerCase()}` : null;
}

function parse(svg: string): Document {
  return new DOMParser().parseFromString(svg, "image/svg+xml");
}

/** Gradient id to its stops and kind. */
function gradients(doc: Document): Map<string, { stops: string[]; radial: boolean }> {
  const out = new Map<string, { stops: string[]; radial: boolean }>();
  for (const g of doc.querySelectorAll("linearGradient, radialGradient")) {
    const stops = [...g.querySelectorAll("stop")]
      .map((s) => hex6(s.getAttribute("stop-color") ?? ""))
      .filter((s): s is string => Boolean(s));
    if (g.id && stops.length) out.set(g.id, { stops, radial: g.localName === "radialGradient" });
  }
  return out;
}

/** The inks, without shares, in document order. */
function declared(doc: Document): Ink[] {
  const defs = gradients(doc);
  const inks: Ink[] = [];
  for (const el of doc.querySelectorAll("[fill], [stroke]")) {
    for (const attr of PAINTS) {
      const key = el.getAttribute(attr)?.trim();
      if (!key) continue;
      const flat = key.startsWith("#") ? hex6(key) : null;
      const id = /^url\(#([^)]+)\)$/.exec(key)?.[1];
      const def = id ? defs.get(id) : undefined;
      if (!flat && !def) continue;
      const stops = flat ? [flat] : def!.stops;
      const kind = flat ? "flat" : "gradient";
      const same = inks.find((i) => i.kind === kind && (i.keys.includes(key) || (flat ? i.hex === flat : i.stops.join() === stops.join())));
      if (same) {
        if (!same.keys.includes(key)) same.keys.push(key);
        continue;
      }
      inks.push({
        traced: stops[0],
        hex: stops[0],
        share: 0,
        snappedDe00: null,
        kind,
        keys: [key],
        stops,
        ...(def ? { gradient: def.radial ? "radial" : "linear" } : {}),
      } as Ink);
    }
  }
  return inks;
}

/** The ink standing for index `i`, on a grid of `levels` per channel; index 0 is black. */
function idColour(i: number, levels: number): string {
  const n = i + 1;
  const step = 255 / (levels - 1);
  const c = (d: number) => Math.round(d * step).toString(16).padStart(2, "0");
  return `#${c(n % levels)}${c(Math.floor(n / levels) % levels)}${c(Math.floor(n / (levels * levels)) % levels)}`;
}

/** Every ink the drawing paints with, and the share of the covered canvas each one takes. */
export async function paletteOf(svg: string): Promise<Ink[]> {
  const doc = parse(svg);
  const inks = declared(doc);
  if (!inks.length) return [];
  let levels = 2;
  while (levels ** 3 <= inks.length) levels++;
  const owner = new Map<string, number>();
  inks.forEach((ink, i) => ink.keys.forEach((k) => owner.set(k, i)));
  for (const el of doc.querySelectorAll("[fill], [stroke]")) {
    for (const attr of PAINTS) {
      const v = el.getAttribute(attr)?.trim();
      if (!v || v === "none") continue;
      const i = owner.get(v);
      el.setAttribute(attr, i === undefined ? "#000000" : idColour(i, levels));
    }
  }
  for (const el of doc.querySelectorAll("[opacity], [fill-opacity], [stroke-opacity]")) {
    for (const a of ["opacity", "fill-opacity", "stroke-opacity"]) {
      if (el.hasAttribute(a)) el.setAttribute(a, Number(el.getAttribute(a)) >= 0.5 ? "1" : "0");
    }
  }
  const root = doc.documentElement;
  root.setAttribute("shape-rendering", "crispEdges");
  const vb = (root.getAttribute("viewBox") ?? "0 0 256 256").split(/[\s,]+/).map(Number);
  const scale = 256 / Math.max(vb[2] || 1, vb[3] || 1);
  const w = Math.max(1, Math.round(vb[2] * scale));
  const h = Math.max(1, Math.round(vb[3] * scale));
  root.setAttribute("width", String(w));
  root.setAttribute("height", String(h));
  const url = URL.createObjectURL(new Blob([new XMLSerializer().serializeToString(doc)], { type: "image/svg+xml" }));
  try {
    const img = new Image();
    img.src = url;
    await img.decode();
    const cv = document.createElement("canvas");
    cv.width = w;
    cv.height = h;
    const ctx = cv.getContext("2d", { willReadFrequently: true })!;
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(img, 0, 0, w, h);
    const px = ctx.getImageData(0, 0, w, h).data;
    const counts = new Array(inks.length).fill(0);
    let covered = 0;
    const step = 255 / (levels - 1);
    for (let p = 0; p < px.length; p += 4) {
      if (px[p + 3] < 128) continue;
      covered++;
      let n = 0;
      let exact = true;
      [px[p], px[p + 1], px[p + 2]].forEach((v, k) => {
        const d = Math.round(v / step);
        if (Math.abs(d * step - v) > 2) exact = false;
        n += d * levels ** k;
      });
      if (exact && n > 0 && n <= inks.length) counts[n - 1]++;
    }
    inks.forEach((ink, i) => (ink.share = counts[i] / Math.max(1, covered)));
  } catch {
    // A drawing the browser will not render still has inks; they just go unmeasured.
  } finally {
    URL.revokeObjectURL(url);
  }
  return inks.sort((a, b) => b.share - a.share);
}

/**
 * The drawing with each group painted as one fill, and the engine-style report lines.
 *
 * A member is found as the engine finds it, by the nearest ink within a tolerance (the
 * engine's is 0.08 in OKLab, about 8 dE00). The group becomes its target: a flat colour, a
 * member (a gradient member's gradient then paints the whole group), or the member covering
 * the most of the drawing.
 */
export async function regroup(svg: string, groups: ColourGroup[]): Promise<{ svg: string; lines: string[] }> {
  if (!groups.length) return { svg, lines: [] };
  const inks = await paletteOf(svg);
  const doc = parse(svg);
  const lines: string[] = [];
  const claimed = new Set<Ink>();
  for (const g of groups) {
    const found: (Ink | null)[] = g.members.map((m) => {
      let best: Ink | null = null;
      let bestD = 8;
      for (const ink of inks) {
        if (claimed.has(ink) || (ink.kind === "gradient") !== m.includes(">")) continue;
        const d = memberDistance(m, memberOf(ink));
        if (d < bestD) {
          bestD = d;
          best = ink;
        }
      }
      return best;
    });
    const merged = found.filter((f): f is Ink => f !== null);
    const missing = g.members.filter((_, i) => !found[i]);
    let line: string;
    if (merged.length < 2) {
      line = "merge colors  group left alone (fewer than two fills found)";
    } else {
      merged.forEach((ink) => claimed.add(ink));
      let into: Ink | null = null;
      let paint: string;
      if (g.target?.startsWith("#")) {
        paint = g.target.toLowerCase();
      } else {
        const k = g.target?.startsWith("@") ? Number(g.target.slice(1)) - 1 : -1;
        into = k >= 0 ? found[k] : merged.reduce((a, b) => (b.share > a.share ? b : a));
        into ??= merged[0];
        paint = into.kind === "gradient" ? into.keys[0] : into.hex;
      }
      const keys = new Set(merged.flatMap((i) => i.keys));
      for (const el of doc.querySelectorAll("[fill], [stroke]")) {
        for (const attr of PAINTS) {
          const v = el.getAttribute(attr)?.trim();
          if (v && keys.has(v)) el.setAttribute(attr, paint);
        }
      }
      const fills = merged.map((i) => i.hex).join(" + ");
      line = `merge colors  ${fills} -> ${into?.kind === "gradient" ? "one gradient" : paint}`;
    }
    if (missing.length) line += `; not found in the trace: ${missing.join(", ")}`;
    lines.push(line);
  }
  return { svg: new XMLSerializer().serializeToString(doc), lines };
}
