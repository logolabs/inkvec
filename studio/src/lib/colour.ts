/**
 * Colour groups: the arithmetic behind the palette's "draw these as one".
 *
 * The engine does the merging (`inkvec_cli::parse_color_groups`, `Args.merge_colors`); this
 * module only decides what to *propose*, names each ink the way the engine's grammar names
 * it, and finds an ink again after a re-trace has moved it by a rounding.
 *
 * Distances are CIEDE2000, the same formula the quality report measures with (the Rust one
 * in `quality.rs`; this is its twin, checked against the same Sharma, Wu & Dalal pairs).
 */

import type { ColourGroup, Ink } from "./ipc";

// ------------------------------------------------------------------ dE00 ---

/** `#rgb` or `#rrggbb` to 0..1 sRGB, or null. */
export function parseHex(hex: string): [number, number, number] | null {
  const h = hex.trim().replace(/^#/, "");
  const full = h.length === 3 ? [...h].map((c) => c + c).join("") : h;
  if (!/^[0-9a-fA-F]{6}$/.test(full)) return null;
  return [0, 2, 4].map((i) => parseInt(full.slice(i, i + 2), 16) / 255) as [number, number, number];
}

/** sRGB (0..1) to CIE L*a*b* under D65. */
export function lab([r0, g0, b0]: [number, number, number]): [number, number, number] {
  const lin = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  const [r, g, b] = [lin(r0), lin(g0), lin(b0)];
  const x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047;
  const y = 0.2126729 * r + 0.7151522 * g + 0.072175 * b;
  const z = (0.0193339 * r + 0.119192 * g + 0.9503041 * b) / 1.08883;
  const f = (t: number) => (t > 216 / 24389 ? Math.cbrt(t) : (24389 / 27 * t + 16) / 116);
  const [fx, fy, fz] = [f(x), f(y), f(z)];
  return [116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz)];
}

/** CIEDE2000 (Sharma, Wu & Dalal 2005), kL = kC = kH = 1. */
export function ciede2000(p: [number, number, number], q: [number, number, number]): number {
  const [l1, a1, b1] = p;
  const [l2, a2, b2] = q;
  const rad = Math.PI / 180;
  const c1 = Math.hypot(a1, b1);
  const c2 = Math.hypot(a2, b2);
  const cb7 = ((c1 + c2) / 2) ** 7;
  const g = 0.5 * (1 - Math.sqrt(cb7 / (cb7 + 25 ** 7)));
  const a1p = (1 + g) * a1;
  const a2p = (1 + g) * a2;
  const c1p = Math.hypot(a1p, b1);
  const c2p = Math.hypot(a2p, b2);
  const hue = (ap: number, b: number) => {
    if (ap === 0 && b === 0) return 0;
    const h = Math.atan2(b, ap) / rad;
    return h < 0 ? h + 360 : h;
  };
  const h1p = hue(a1p, b1);
  const h2p = hue(a2p, b2);
  const dlp = l2 - l1;
  const dcp = c2p - c1p;
  let dhp = 0;
  if (c1p * c2p !== 0) {
    dhp = h2p - h1p;
    if (dhp > 180) dhp -= 360;
    else if (dhp < -180) dhp += 360;
  }
  const dH = 2 * Math.sqrt(c1p * c2p) * Math.sin((dhp * rad) / 2);
  const lpBar = (l1 + l2) / 2;
  const cpBar = (c1p + c2p) / 2;
  let hpBar = h1p + h2p;
  if (c1p * c2p !== 0) {
    const d = Math.abs(h1p - h2p);
    hpBar = d <= 180 ? (h1p + h2p) / 2 : h1p + h2p < 360 ? (h1p + h2p + 360) / 2 : (h1p + h2p - 360) / 2;
  }
  const t =
    1 -
    0.17 * Math.cos((hpBar - 30) * rad) +
    0.24 * Math.cos(2 * hpBar * rad) +
    0.32 * Math.cos((3 * hpBar + 6) * rad) -
    0.2 * Math.cos((4 * hpBar - 63) * rad);
  const dTheta = 30 * Math.exp(-(((hpBar - 275) / 25) ** 2));
  const cp7 = cpBar ** 7;
  const rc = 2 * Math.sqrt(cp7 / (cp7 + 25 ** 7));
  const sl = 1 + (0.015 * (lpBar - 50) ** 2) / Math.sqrt(20 + (lpBar - 50) ** 2);
  const sc = 1 + 0.045 * cpBar;
  const sh = 1 + 0.015 * cpBar * t;
  const rt = -Math.sin(2 * dTheta * rad) * rc;
  const tl = dlp / sl;
  const tc = dcp / sc;
  const th = dH / sh;
  return Math.sqrt(tl * tl + tc * tc + th * th + rt * tc * th);
}

// --------------------------------------------------------------- members ---

/**
 * An ink as the engine's grammar names it: `#rrggbb`, or a gradient's stops joined by `>`.
 *
 * The *traced* colour, not a snapped one: the engine merges inks of the image before it
 * traces, and a snap is a rewrite of the finished file that the image never saw.
 */
export function memberOf(ink: Ink): string {
  if (ink.kind === "gradient" && ink.stops.length > 1) return ink.stops.map((s) => s.toLowerCase()).join(">");
  return ink.traced.toLowerCase();
}

/** A member's stop colours: one for a flat member. */
export function stopsOf(member: string): string[] {
  return member.split(">").map((s) => s.trim().toLowerCase());
}

/** Whether a member is a gradient. */
export function isGradient(member: string): boolean {
  return member.includes(">");
}

/** A CSS background that previews a member: a flat colour or its stops as a ramp. */
export function swatchBackground(member: string, radial = false): string {
  const stops = stopsOf(member);
  if (stops.length < 2) return stops[0] ?? "transparent";
  return radial ? `radial-gradient(circle, ${stops.join(", ")})` : `linear-gradient(90deg, ${stops.join(", ")})`;
}

const labCache = new Map<string, [number, number, number]>();
function labOf(hex: string): [number, number, number] | null {
  const hit = labCache.get(hex);
  if (hit) return hit;
  const rgb = parseHex(hex);
  if (!rgb) return null;
  const l = lab(rgb);
  if (labCache.size > 4096) labCache.clear();
  labCache.set(hex, l);
  return l;
}

/**
 * How far apart two members are, in dE00.
 *
 * Two flat colours: their CIEDE2000. A gradient is the set of its stops, and two sets are
 * compared by the modified Hausdorff distance (Dubuisson & Jain, 1994): each stop's distance
 * to the nearest stop of the other, averaged, and the larger of the two directions. It is 0
 * for the same ramp, equals the plain dE00 for two flats, and for a flat against a ramp it is
 * the ramp's mean distance from that colour, so a colour joins a gentle ramp around it but
 * not a ramp that merely passes through it on the way to something else.
 */
export function memberDistance(a: string, b: string): number {
  const la = stopsOf(a).map(labOf);
  const lb = stopsOf(b).map(labOf);
  if (la.some((x) => !x) || lb.some((x) => !x) || !la.length || !lb.length) return Infinity;
  const pa = la as [number, number, number][];
  const pb = lb as [number, number, number][];
  const directed = (from: [number, number, number][], to: [number, number, number][]) =>
    from.reduce((sum, p) => sum + Math.min(...to.map((q) => ciede2000(p, q))), 0) / from.length;
  return Math.max(directed(pa, pb), directed(pb, pa));
}

/**
 * How close an ink of a later trace must be to a member to be taken for it, in dE00. A
 * re-trace refits the same pixels, so an ink that survived lands within a rounding of where
 * it was; 2 is where CIEDE2000 stops being "only perceptible through close observation".
 */
export const SAME_INK_DE00 = 2;

/** The palette ink a member stands for now, if one is near enough. */
export function inkFor(member: string, palette: Ink[]): Ink | null {
  const grad = isGradient(member);
  const exact = palette.find((ink) => memberOf(ink) === member);
  if (exact) return exact;
  let best: Ink | null = null;
  let bestD = SAME_INK_DE00;
  for (const ink of palette) {
    if ((ink.kind === "gradient") !== grad) continue;
    const d = memberDistance(member, memberOf(ink));
    if (d <= bestD) {
      bestD = d;
      best = ink;
    }
  }
  return best;
}

/** Two members are the same fill. */
export function sameMember(a: string, b: string): boolean {
  return a === b || memberDistance(a, b) < 0.5;
}

// ----------------------------------------------------------- suggestions ---

/**
 * The dE00 under which two inks are proposed as one.
 *
 * CIEDE2000's usual reading: under 1 a difference is not perceptible, 1 to 2 is perceptible
 * only through close observation, 2 to 10 at a glance. Traced palettes bear that out. Over
 * the flat inks of forty noto emoji and the bundled samples (442 inks), each ink's nearest
 * neighbour is bimodal: a tenth sit under 1 (#7b7b7b beside #7c7c7c, #ca12cf beside #cb12cf,
 * one colour split by the tracer), and past about 2.2 the neighbours are the artist's own
 * shading steps (skin tones 2.5 to 4 apart, #c2185b beside #d81b60 at 4.9). 2.0 takes the
 * first group and leaves the second alone.
 */
export const SUGGEST_DE00 = 2;

/** The most inks proposals are worked out over; see `suggestGroups`. */
const MAX_CLUSTERED = 240;

/**
 * Proposed groups: inks that are near duplicates of each other, by complete-linkage
 * agglomerative clustering.
 *
 * Complete linkage (a cluster's distance to another is its *farthest* pair) because the
 * palette of a shaded drawing is a chain of steps each a little apart: single linkage would
 * walk down the chain and fold a whole ramp from highlight to shadow into one group, where
 * complete linkage keeps every member of a group within the threshold of every other.
 *
 * With `count`, merging runs on past the threshold until `count` inks are left — the
 * "simplify to N colours" proposal — and only ever joins the closest pair left.
 *
 * Only inks are clustered, not what the user has already grouped or turned down: `skip`
 * members are left out. Each group comes back as members ordered by share, largest first,
 * with target Auto (the engine's "most used"). Groups of one are not groups.
 */
export function suggestGroups(palette: Ink[], opts: { count?: number | null; skip?: string[] } = {}): ColourGroup[] {
  const skip = opts.skip ?? [];
  const shown = palette.filter((ink) => ink.share > 0);
  // The largest inks only, on a photo-like trace of hundreds: clustering is cubic in the
  // count, and a merge among inks each under a tenth of a percent changes nothing visible.
  const items = [...shown]
    .sort((a, b) => b.share - a.share)
    .slice(0, MAX_CLUSTERED)
    .map((ink) => ({ member: memberOf(ink), share: ink.share }))
    .filter((it) => !skip.some((s) => sameMember(s, it.member)));
  const n = items.length;
  if (n < 2) return [];
  // Cluster-to-cluster distances, kept up to date by the Lance-Williams rule for complete
  // linkage: the union's distance to a third cluster is the larger of its parts' distances.
  const d: number[][] = items.map((a, i) => items.map((b, j) => (i === j ? 0 : memberDistance(a.member, b.member))));
  const clusters: (number[] | null)[] = items.map((_, i) => [i]);
  let alive = n;
  // "Simplify to N" counts every ink the palette shows, including those left out here.
  const target = opts.count ? Math.max(1, Math.round(opts.count) - (shown.length - n)) : null;
  for (;;) {
    if (target !== null && alive <= target) break;
    let best = Infinity;
    let a = -1;
    let b = -1;
    for (let i = 0; i < n; i++) {
      if (!clusters[i]) continue;
      for (let j = i + 1; j < n; j++) {
        if (clusters[j] && d[i][j] < best) {
          best = d[i][j];
          a = i;
          b = j;
        }
      }
    }
    if (a < 0) break;
    if (target === null && best > SUGGEST_DE00) break;
    clusters[a] = [...(clusters[a] as number[]), ...(clusters[b] as number[])];
    clusters[b] = null;
    alive--;
    for (let k = 0; k < n; k++) {
      if (k === a || !clusters[k]) continue;
      const m = Math.max(d[a][k], d[b][k]);
      d[a][k] = m;
      d[k][a] = m;
    }
  }
  return (clusters.filter(Boolean) as number[][])
    .filter((c) => c.length > 1)
    .map((c) => c.map((i) => items[i]).sort((x, y) => y.share - x.share))
    .sort((x, y) => y.reduce((s, it) => s + it.share, 0) - x.reduce((s, it) => s + it.share, 0))
    .map((c) => ({ members: c.map((it) => it.member), target: null }));
}

/** A group's identity for "the user turned this down": its members, order-free. */
export function groupSignature(g: ColourGroup): string {
  return [...g.members].sort().join(",");
}

// ---------------------------------------------------------------- report ---

/** What the engine said about one group, read off its `merge colors` report line. */
export interface MergeReport {
  /** The line without its label, and without the list of what was not found. */
  line: string;
  /** Members the engine found nothing of in the image, spelt as members. */
  unmatched: string[];
  /** Fewer than two fills matched, so the group did nothing. */
  leftAlone: boolean;
}

/**
 * The trace's `merge colors` lines, one per group in the order the groups were sent:
 * `merge colors  #a + #b -> #c`, `... -> one gradient`, or `group left alone (...)`, each
 * followed by `; not found in the trace: <members>` when a member matched nothing (earlier
 * engines wrote `; no ink near <colours>`).
 */
export function mergeReports(engineLog: string[]): MergeReport[] {
  return engineLog
    .filter((l) => l.trimStart().startsWith("merge colors"))
    .map((raw) => {
      const text = raw.trim().replace(/^merge colors\s+/, "");
      const cut = /;\s*(?:not found in the trace:|no ink near)\s*(.*)$/.exec(text);
      const unmatched = (cut?.[1] ?? "")
        .split(",")
        .map((m) => m.trim().toLowerCase())
        .filter((m) => /^#[0-9a-f]{6}(>#[0-9a-f]{6})*$/.test(m));
      return {
        line: cut ? text.slice(0, cut.index).trim() : text,
        unmatched,
        leftAlone: text.startsWith("group left alone"),
      };
    });
}

/** Whether the engine reported `member` as found nowhere in the image. */
export function reportedMissing(report: MergeReport | null, member: string): boolean {
  if (!report) return false;
  return report.unmatched.some((u) => u === member || stopsOf(member).includes(u) || sameMember(u, member));
}
