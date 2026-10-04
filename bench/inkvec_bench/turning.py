"""The gate's `turning` axis: how far an SVG's outlines turn, per unit of their length.

`turning` exists to see a sawtooth. A boundary that zig-zags around the true edge can render
almost as well as a smooth one, so dE00 barely moves; the zig-zag shows as extra turning.
(On openmoji/1F3A1 the teeth lowered self_res from 0.0146 to 0.0122 while the old reading
of turning went from 277 to 828 and the parameter count doubled.)

What is measured
----------------
Each subpath of each `<path d>` is read command by command, and so is every `<rect>`,
`<circle>`, `<ellipse>`, `<polygon>` and `<polyline>` as the path it stands for (the
emitter writes fitted primitives that way). Each subpath's control polygon is measured:
- a line contributes its end point;
- a quadratic its control point and end;
- a cubic its two controls and end;
- an elliptical arc the controls and ends of its cubic approximation, in pieces of at most
  90 degrees, the standard conversion. An arc and the cubics it stands for then read the
  same: a circle written as two arcs reads 0.0905, as four cubics 0.0904.

`turning` is the total absolute turning angle at the polygon's interior vertices, plus the
vertex where a closed subpath ends and begins again, over the polygon's total length. A
convex closed ring turns exactly 2 pi, so the value falls as rings grow and rises with every
wiggle.

Not from the literature as a gate axis. The control polygon is used rather than the curve
itself because it is what the file stores and what an editor shows: every handle that
doubles back adds turning even where the curve hides it.

Before scorer version 3 (2026-10-04)
------------------------------------
The axis took every `x,y` pair in `d` as a point. That has three defects:
- It read an arc's radii (`rx,ry`) and flags (`large,sweep`) as points. Our own output
  writes arcs in 215 of 246 screen-set traces, so this affected nearly every icon.
- It skipped relative commands and numbers written without a comma.
- It ran one polyline through every subpath, so the jump from one ring to the next
  counted as turning.

Baselines recorded with the old reading are not comparable; see `SCORER_VERSION` in
`svgeval.py`.
"""
from __future__ import annotations

import math
import re

D_ATTR = re.compile(r'\sd="([^"]+)"')

#: Points closer than this (user units) are one point: a zero-length step has no direction.
SAME_POINT = 1e-9


def _polygons(d: str):
    """Yield `(points, closed)` for each subpath of one path's `d`."""
    from svgelements import Arc, Close, CubicBezier, Line, Move, Path, QuadraticBezier

    pts: list[tuple[float, float]] = []
    closed = False
    for seg in Path(d):
        if isinstance(seg, Move):
            if len(pts) > 1:
                yield pts, closed
            pts, closed = [(seg.end.x, seg.end.y)], False
            continue
        if not pts and seg.start is not None:
            pts = [(seg.start.x, seg.start.y)]
        if isinstance(seg, Close):
            closed = True
            if seg.end is not None:
                pts.append((seg.end.x, seg.end.y))
        elif isinstance(seg, Line):
            pts.append((seg.end.x, seg.end.y))
        elif isinstance(seg, QuadraticBezier):
            pts += [(seg.control.x, seg.control.y), (seg.end.x, seg.end.y)]
        elif isinstance(seg, CubicBezier):
            pts += [(seg.control1.x, seg.control1.y), (seg.control2.x, seg.control2.y),
                    (seg.end.x, seg.end.y)]
        elif isinstance(seg, Arc):
            pieces = max(1, math.ceil(abs(seg.sweep) / (math.pi / 2) - 1e-9))
            for c in seg.as_cubic_curves(pieces):
                pts += [(c.control1.x, c.control1.y), (c.control2.x, c.control2.y),
                        (c.end.x, c.end.y)]
    if len(pts) > 1:
        yield pts, closed


def _turn_and_length(pts, closed):
    """Total absolute turning (radians) and total length of one control polygon."""
    q = [pts[0]]
    for p in pts[1:]:
        if abs(p[0] - q[-1][0]) > SAME_POINT or abs(p[1] - q[-1][1]) > SAME_POINT:
            q.append(p)
    if closed and len(q) > 2 and math.dist(q[0], q[-1]) <= SAME_POINT:
        q.pop()
    n = len(q)
    if n < 2:
        return 0.0, 0.0
    steps = [(q[i + 1][0] - q[i][0], q[i + 1][1] - q[i][1]) for i in range(n - 1)]
    if closed and n > 2:
        steps.append((q[0][0] - q[-1][0], q[0][1] - q[-1][1]))
    length = sum(math.hypot(*s) for s in steps)
    pairs = list(zip(steps, steps[1:]))
    if closed and len(steps) > 2:
        pairs.append((steps[-1], steps[0]))
    turn = sum(abs(math.atan2(ax * by - ay * bx, ax * bx + ay * by))
               for (ax, ay), (bx, by) in pairs)
    return turn, length


#: Outline elements other than <path> the tracer writes (`emit` turns a fitted rectangle,
#: circle, ellipse or polygon into one of these).
SHAPE = re.compile(r"<(?:rect|circle|ellipse|polygon|polyline)\s[^>]*>")


def _shape_d(tag: str) -> str:
    """Path data for one primitive element, as svgelements converts it (a rounded rect's
    corners become arcs, a circle or ellipse four arcs)."""
    import io

    from svgelements import SVG, Path, Shape

    tag = tag if tag.endswith("/>") else tag[:-1] + "/>"
    doc = SVG.parse(io.StringIO(f'<svg xmlns="http://www.w3.org/2000/svg">{tag}</svg>'))
    return " ".join(Path(e).d() for e in doc.elements()
                    if isinstance(e, Shape) and not isinstance(e, SVG))


def turning(svg: str) -> float:
    """Total absolute turning of every outline's control polygon per unit of its length:
    every `<path d>`, and every `<rect>`, `<circle>`, `<ellipse>`, `<polygon>` and
    `<polyline>` read as the path it stands for. Reading only `d` dropped a shape from the
    measure whenever the emitter wrote it as a primitive, which changed the reading without
    changing a single outline (2026-10-04: the 0.2.6 border pad turned 18 rounded squares
    and circles of the 512 px screen set into primitives, +0.96 % on `d` alone, -0.05 % read
    whole)."""
    total_turn = total_len = 0.0
    ds = D_ATTR.findall(svg) + [_shape_d(m.group(0)) for m in SHAPE.finditer(svg)]
    for d in ds:
        for pts, closed in _polygons(d):
            t, l = _turn_and_length(pts, closed)
            total_turn += t
            total_len += l
    return total_turn / total_len if total_len > 1e-9 else 0.0
