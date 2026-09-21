/**
 * Just enough SVG path reading to draw the overlays.
 *
 * The anchors overlay puts a dot on every on-curve point and the handles overlay adds
 * each cubic's control points with tangent lines, so both need the path's own structure —
 * which the DOM will not give you. `getPointAtLength` samples a curve; it cannot tell you
 * where the curve's authors put its nodes, and sampling would draw dots in the wrong
 * places on exactly the drawings where the overlay matters.
 *
 * Only what the tracer emits is handled, plus the rest of the grammar for an SVG the
 * Minify tab was handed by someone else.
 */

/** An on-curve point, and the control points of the segment that arrives at it. */
export interface Node {
  x: number;
  y: number;
  /** The control point leaving the previous node, if that segment was a curve. */
  in?: { x: number; y: number };
  /** The control point arriving at this node, if that segment was a curve. */
  out?: { x: number; y: number };
}

const ARGS: Record<string, number> = {
  M: 2, L: 2, T: 2, H: 1, V: 1, C: 6, S: 4, Q: 4, A: 7, Z: 0,
};

/** Split a `d` attribute into `[command, ...numbers]` runs. */
function* commands(d: string): Generator<[string, number[]]> {
  const tokens = d.match(/[a-zA-Z]|-?\d*\.?\d+(?:[eE][-+]?\d+)?/g) ?? [];
  let i = 0;
  let cmd = "";
  while (i < tokens.length) {
    const t = tokens[i];
    if (/[a-zA-Z]/.test(t)) {
      cmd = t;
      i++;
    } else if (!cmd) {
      i++;
      continue;
    }
    const n = ARGS[cmd.toUpperCase()] ?? 0;
    if (n === 0) {
      yield [cmd, []];
      // An implicit repeat of Z is meaningless; move on.
      if (i < tokens.length && !/[a-zA-Z]/.test(tokens[i])) i++;
      continue;
    }
    const args = tokens.slice(i, i + n).map(Number);
    if (args.length < n || args.some(Number.isNaN)) return;
    yield [cmd, args];
    i += n;
    // An implicit repeat of M continues as L, which is what the spec says.
    if (cmd === "M") cmd = "L";
    if (cmd === "m") cmd = "l";
  }
}

/**
 * The nodes of one `d` attribute, in order.
 *
 * Control points are attached to the node they belong to rather than returned separately,
 * so the handles overlay can draw each tangent line without re-walking the path.
 */
export function nodesOf(d: string): Node[] {
  const out: Node[] = [];
  let x = 0;
  let y = 0;
  let startX = 0;
  let startY = 0;
  // The reflected control point S and T need.
  let lastControl: { x: number; y: number } | null = null;

  for (const [raw, a] of commands(d)) {
    const rel = raw >= "a" && raw <= "z";
    const cmd = raw.toUpperCase();
    const px = x;
    const py = y;
    const abs = (i: number) => (rel ? px + a[i] : a[i]);
    const aby = (i: number) => (rel ? py + a[i] : a[i]);

    switch (cmd) {
      case "M":
        x = abs(0);
        y = aby(1);
        startX = x;
        startY = y;
        out.push({ x, y });
        lastControl = null;
        break;
      case "L":
        x = abs(0);
        y = aby(1);
        out.push({ x, y });
        lastControl = null;
        break;
      case "H":
        x = rel ? px + a[0] : a[0];
        out.push({ x, y });
        lastControl = null;
        break;
      case "V":
        y = rel ? py + a[0] : a[0];
        out.push({ x, y });
        lastControl = null;
        break;
      case "C": {
        const c1 = { x: abs(0), y: aby(1) };
        const c2 = { x: abs(2), y: aby(3) };
        x = abs(4);
        y = aby(5);
        out.push({ x, y, in: c1, out: c2 });
        lastControl = c2;
        break;
      }
      case "S": {
        const c1: { x: number; y: number } = lastControl
          ? { x: 2 * px - lastControl.x, y: 2 * py - lastControl.y }
          : { x: px, y: py };
        const c2 = { x: abs(0), y: aby(1) };
        x = abs(2);
        y = aby(3);
        out.push({ x, y, in: c1, out: c2 });
        lastControl = c2;
        break;
      }
      case "Q": {
        // A quadratic's single control is shown as both handles: it is one control
        // point, and pretending otherwise would draw a node that is not there.
        const c = { x: abs(0), y: aby(1) };
        x = abs(2);
        y = aby(3);
        out.push({ x, y, in: c, out: c });
        lastControl = c;
        break;
      }
      case "T": {
        const c: { x: number; y: number } = lastControl
          ? { x: 2 * px - lastControl.x, y: 2 * py - lastControl.y }
          : { x: px, y: py };
        x = abs(0);
        y = aby(1);
        out.push({ x, y, in: c, out: c });
        lastControl = c;
        break;
      }
      case "A":
        x = abs(5);
        y = aby(6);
        out.push({ x, y });
        lastControl = null;
        break;
      case "Z":
        x = startX;
        y = startY;
        lastControl = null;
        break;
    }
  }
  return out;
}

/** Every `d` attribute in a document, in order. */
export function pathData(svg: string): string[] {
  const out: string[] = [];
  const re = /\sd="([^"]*)"/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(svg)) !== null) out.push(m[1]);
  return out;
}

/** Elements the overlay knows how to draw, in document order. */
const SHAPES = "path, circle, ellipse, rect, line, polyline, polygon";

/**
 * Every drawable shape in a parsed document, as path data.
 *
 * The tracer does not emit only `<path>`. A round face comes out as `<circle>`, an
 * oval as `<ellipse>` (sometimes with a `rotate()` on it), and a rectangle as `<rect>`
 * — that is the whole point of primitive recovery, and it is usually the largest shape
 * in the drawing. Reading `d` attributes alone left those shapes with no wireframe and
 * no nodes, so on a logo built from a disc the overlay drew everything except the disc.
 *
 * Primitives are converted rather than sampled: a circle becomes four arcs through its
 * quadrant points, which is where an editor puts its nodes, and a rotated ellipse keeps
 * the rotation as the arcs' own x-axis rotation so nothing has to carry a transform.
 */
export function shapesOf(root: Element): string[] {
  const out: string[] = [];
  for (const el of root.querySelectorAll(SHAPES)) {
    const d = shapeData(el);
    if (d) out.push(d);
  }
  return out;
}

function num(el: Element, name: string, fallback = 0): number {
  // An absent attribute is the fallback, not zero: `Number(null)` is 0, which quietly
  // turned `<rect rx="8">` (no `ry`) into a square-cornered rectangle.
  const raw = el.getAttribute(name);
  if (raw === null || raw.trim() === "") return fallback;
  const v = Number(raw);
  return Number.isFinite(v) ? v : fallback;
}

/** The degrees in a bare `transform="rotate(a cx cy)"`, which is all the tracer emits. */
function rotation(el: Element): number {
  const m = /rotate\(\s*(-?[\d.]+)/.exec(el.getAttribute("transform") ?? "");
  return m ? Number(m[1]) : 0;
}

function shapeData(el: Element): string | null {
  const f = (n: number) => (Math.round(n * 1000) / 1000).toString();
  switch (el.nodeName.toLowerCase()) {
    case "path":
      return el.getAttribute("d");

    case "circle":
    case "ellipse": {
      const cx = num(el, "cx");
      const cy = num(el, "cy");
      const isCircle = el.nodeName.toLowerCase() === "circle";
      const rx = isCircle ? num(el, "r") : num(el, "rx");
      const ry = isCircle ? num(el, "r") : num(el, "ry");
      if (!(rx > 0) || !(ry > 0)) return null;
      const deg = rotation(el);
      const rad = (deg * Math.PI) / 180;
      const cos = Math.cos(rad);
      const sin = Math.sin(rad);
      // The four quadrant points, carried through the ellipse's own rotation.
      const at = (ax: number, ay: number) =>
        `${f(cx + ax * cos - ay * sin)},${f(cy + ax * sin + ay * cos)}`;
      const arc = `A${f(rx)},${f(ry)} ${f(deg)} 0 1 `;
      return (
        `M${at(rx, 0)}${arc}${at(0, ry)}${arc}${at(-rx, 0)}` +
        `${arc}${at(0, -ry)}${arc}${at(rx, 0)}Z`
      );
    }

    case "rect": {
      const x = num(el, "x");
      const y = num(el, "y");
      const w = num(el, "width");
      const h = num(el, "height");
      if (!(w > 0) || !(h > 0)) return null;
      // `rx` alone means both, which is what the tracer writes for a rounded rect.
      const rx = Math.min(num(el, "rx", num(el, "ry")), w / 2);
      const ry = Math.min(num(el, "ry", num(el, "rx")), h / 2);
      if (!(rx > 0) || !(ry > 0)) {
        return `M${f(x)},${f(y)}L${f(x + w)},${f(y)}L${f(x + w)},${f(y + h)}L${f(x)},${f(y + h)}Z`;
      }
      const a = `A${f(rx)},${f(ry)} 0 0 1 `;
      return (
        `M${f(x + rx)},${f(y)}L${f(x + w - rx)},${f(y)}${a}${f(x + w)},${f(y + ry)}` +
        `L${f(x + w)},${f(y + h - ry)}${a}${f(x + w - rx)},${f(y + h)}` +
        `L${f(x + rx)},${f(y + h)}${a}${f(x)},${f(y + h - ry)}` +
        `L${f(x)},${f(y + ry)}${a}${f(x + rx)},${f(y)}Z`
      );
    }

    case "line":
      return `M${f(num(el, "x1"))},${f(num(el, "y1"))}L${f(num(el, "x2"))},${f(num(el, "y2"))}`;

    case "polyline":
    case "polygon": {
      const pts = (el.getAttribute("points") ?? "")
        .trim()
        .split(/[\s,]+/)
        .map(Number);
      if (pts.length < 4 || pts.some((n) => !Number.isFinite(n))) return null;
      const parts: string[] = [`M${f(pts[0])},${f(pts[1])}`];
      for (let i = 2; i + 1 < pts.length; i += 2) parts.push(`L${f(pts[i])},${f(pts[i + 1])}`);
      if (el.nodeName.toLowerCase() === "polygon") parts.push("Z");
      return parts.join("");
    }

    default:
      return null;
  }
}

/** The `viewBox` of a document, or its width/height, or null. */
export function viewBoxOf(svg: string): { x: number; y: number; w: number; h: number } | null {
  const vb = /viewBox="([^"]*)"/.exec(svg);
  if (vb) {
    const n = vb[1].split(/[\s,]+/).map(Number);
    if (n.length === 4 && n.every((v) => Number.isFinite(v))) {
      return { x: n[0], y: n[1], w: n[2], h: n[3] };
    }
  }
  const w = /\swidth="([\d.]+)"/.exec(svg);
  const hh = /\sheight="([\d.]+)"/.exec(svg);
  if (w && hh) return { x: 0, y: 0, w: Number(w[1]), h: Number(hh[1]) };
  return null;
}

/**
 * How big an anchor dot should be, and how visible, at this zoom.
 *
 * A complex logo carries a few thousand nodes. Fixing the dot size would turn them into a
 * red smear at 1x; the dots thin out with zoom instead of being capped, so the overlay is
 * decorative at a glance and precise where it is being used to check an edge.
 */
export function anchorStyle(zoom: number): { r: number; opacity: number } {
  const t = Math.min(1, Math.max(0, (zoom - 1) / 11));
  return { r: 1.2 + t * 1.2, opacity: 0.4 + t * 0.6 };
}
