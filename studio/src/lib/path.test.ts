import { describe, expect, it } from "vitest";

import { anchorStyle, nodesOf, shapesOf, viewBoxOf } from "./path";

/** The on-curve points of a path, as [x, y] pairs. */
const points = (d: string) => nodesOf(d).map((n) => [n.x, n.y]);

/** The shapes of an SVG document, parsed the way the viewer parses it. */
function shapes(body: string): string[] {
  const doc = new DOMParser().parseFromString(`<svg xmlns="http://www.w3.org/2000/svg">${body}</svg>`, "image/svg+xml");
  return shapesOf(doc.documentElement);
}

describe("nodesOf", () => {
  it("reads absolute and relative lines, and closes back to the subpath's start", () => {
    expect(points("M10 20 L30 40 Z")).toEqual([
      [10, 20],
      [30, 40],
    ]);
    expect(points("m10 10 l5 0 l0 5 z l1 1")).toEqual([
      [10, 10],
      [15, 10],
      [15, 15],
      [11, 11],
    ]);
  });

  it("continues an implicit repeat of M as L", () => {
    expect(points("M0 0 10 0 10 10")).toEqual([
      [0, 0],
      [10, 0],
      [10, 10],
    ]);
    expect(points("m1 1 2 0")).toEqual([
      [1, 1],
      [3, 1],
    ]);
  });

  it("reads numbers the way the grammar allows: packed signs, exponents, leading dots", () => {
    expect(points("M10-5L-3-4")).toEqual([
      [10, -5],
      [-3, -4],
    ]);
    expect(points("M1e1 2E-1 L.5.25")).toEqual([
      [10, 0.2],
      [0.5, 0.25],
    ]);
  });

  it("reads horizontal and vertical runs", () => {
    expect(points("M1 1 H5 V7 h-2 v-3")).toEqual([
      [1, 1],
      [5, 1],
      [5, 7],
      [3, 7],
      [3, 4],
    ]);
  });

  it("attaches a cubic's control points to the node it arrives at", () => {
    expect(nodesOf("M0 0 C1 2 3 4 5 6")[1]).toEqual({ x: 5, y: 6, in: { x: 1, y: 2 }, out: { x: 3, y: 4 } });
    expect(nodesOf("M10 10 c1 2 3 4 5 6")[1]).toEqual({ x: 15, y: 16, in: { x: 11, y: 12 }, out: { x: 13, y: 14 } });
  });

  it("reflects the previous control point for S and T", () => {
    const s = nodesOf("M0 0 C0 10 10 10 10 0 S20 -10 20 0");
    expect(s[2]).toEqual({ x: 20, y: 0, in: { x: 10, y: -10 }, out: { x: 20, y: -10 } });
    const t = nodesOf("M0 0 Q5 10 10 0 T20 0");
    expect(t[2]).toEqual({ x: 20, y: 0, in: { x: 15, y: -10 }, out: { x: 15, y: -10 } });
    // With no curve before it, S's first control is the current point.
    expect(nodesOf("M3 4 S5 6 7 8")[1].in).toEqual({ x: 3, y: 4 });
  });

  it("puts an arc's end point down as a node with no handles", () => {
    expect(nodesOf("M0 0 A5 5 0 0 1 10 0")[1]).toEqual({ x: 10, y: 0 });
  });

  it("stops at a command that is missing its arguments", () => {
    expect(points("M10")).toEqual([]);
    expect(points("M0 0 L5 5 C1 2")).toEqual([
      [0, 0],
      [5, 5],
    ]);
    expect(points("")).toEqual([]);
  });
});

describe("viewBoxOf", () => {
  it("reads the viewBox, space- or comma-separated", () => {
    expect(viewBoxOf('<svg viewBox="0 0 100 50">')).toEqual({ x: 0, y: 0, w: 100, h: 50 });
    expect(viewBoxOf('<svg viewBox="-5,-5,10,10">')).toEqual({ x: -5, y: -5, w: 10, h: 10 });
  });

  it("falls back to width and height when the viewBox is missing or unreadable", () => {
    expect(viewBoxOf('<svg width="64" height="32">')).toEqual({ x: 0, y: 0, w: 64, h: 32 });
    expect(viewBoxOf('<svg viewBox="0 0 wide" width="8" height="4">')).toEqual({ x: 0, y: 0, w: 8, h: 4 });
  });

  it("returns null when the document says nothing about its size", () => {
    expect(viewBoxOf("<svg>")).toBeNull();
    expect(viewBoxOf('<svg width="64">')).toBeNull();
  });
});

describe("shapesOf", () => {
  it("keeps a path's own data, and lists shapes in document order", () => {
    expect(shapes('<path d="M0 0L1 1"/><g><line x1="1" y1="2" x2="3" y2="4"/></g>')).toEqual(["M0 0L1 1", "M1,2L3,4"]);
  });

  it("draws a circle as four arcs through its quadrant points", () => {
    const [d] = shapes('<circle cx="10" cy="10" r="5"/>');
    expect(d).toBe("M15,10A5,5 0 0 1 10,15A5,5 0 0 1 5,10A5,5 0 0 1 10,5A5,5 0 0 1 15,10Z");
    expect(points(d)).toEqual([
      [15, 10],
      [10, 15],
      [5, 10],
      [10, 5],
      [15, 10],
    ]);
  });

  it("carries an ellipse's rotation into the arcs", () => {
    const [d] = shapes('<ellipse cx="0" cy="0" rx="4" ry="2" transform="rotate(90 0 0)"/>');
    expect(d.startsWith("M0,4A4,2 90 0 1 -2,0")).toBe(true);
  });

  it("draws rectangles square or rounded, taking rx alone for both radii", () => {
    expect(shapes('<rect x="0" y="0" width="10" height="5"/>')).toEqual(["M0,0L10,0L10,5L0,5Z"]);
    const [rounded] = shapes('<rect width="10" height="6" rx="2"/>');
    expect(rounded.startsWith("M2,0L8,0A2,2 0 0 1 10,2")).toBe(true);
    // A radius larger than half a side is clamped to it.
    const [pill] = shapes('<rect width="10" height="4" rx="9"/>');
    expect(pill.startsWith("M5,0L5,0A5,2 0 0 1 10,2")).toBe(true);
  });

  it("draws polylines open and polygons closed", () => {
    expect(shapes('<polyline points="0,0 5,5 10,0"/><polygon points="0 0 1 0 1 1"/>')).toEqual(["M0,0L5,5L10,0", "M0,0L1,0L1,1Z"]);
  });

  it("skips shapes with nothing to draw", () => {
    expect(shapes('<circle r="0"/><rect width="0" height="4"/><polygon points="1"/><text>hi</text>')).toEqual([]);
  });
});

describe("anchorStyle", () => {
  it("grows the dots from 1x to 12x zoom and holds them there", () => {
    expect(anchorStyle(1)).toEqual({ r: 1.2, opacity: 0.4 });
    expect(anchorStyle(0.5)).toEqual({ r: 1.2, opacity: 0.4 });
    expect(anchorStyle(12)).toEqual({ r: 2.4, opacity: 1 });
    expect(anchorStyle(40)).toEqual({ r: 2.4, opacity: 1 });
  });
});
