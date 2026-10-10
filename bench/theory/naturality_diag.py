"""How much of a trace's parameter count is "unnatural": descriptions an artist would not write.

Counts, in the SVGs inkvec writes, the patterns a shorter description would replace at no
cost in fit (each a merge the MDL objective itself prefers once the fit is unchanged):

* `collinear`  a vertex between two line segments that turns by less than `TURN_DEG`
               (one line would do: -2 parameters)
* `straight`   a cubic whose control points lie within `STRAIGHT_PX` of its chord
               (a line would do: -4 parameters)
* `cocircular` two consecutive arcs with the same radius (within `RADIUS_REL`) and centre
               (one arc would do: -7 parameters)
* `continue`   two open stroke paths meeting end to end with tangents continuing within
               `TURN_DEG` (one path would do; their shared segment pair is then often
               `collinear` too)

    python3 bench/theory/naturality_diag.py --exe target/release/inkvec --tier 512ss \\
        --families lucide,openmoji
"""

from __future__ import annotations

import argparse
import math
import re
import subprocess
import sys
import tempfile
from collections import Counter
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))

TURN_DEG = 3.0
STRAIGHT_PX = 0.25
RADIUS_REL = 0.02


def subpaths(svg: str):
    """(stroked, list of segments) per subpath; a segment is ('L', p0, p1), ('C', p0, c1, c2,
    p1) or ('A', p0, r, p1, large, sweep)."""
    import svgelements as se
    doc = se.SVG.parse(__import__("io").StringIO(svg))
    out = []
    for el in doc.elements():
        if not isinstance(el, se.Shape) or isinstance(el, (se.Circle, se.Ellipse, se.Rect)):
            continue
        stroked = el.stroke is not None and el.stroke.value is not None and (
            el.fill is None or el.fill.value is None)
        cur = []
        for seg in se.Path(el).segments():
            if isinstance(seg, se.Move):
                if cur:
                    out.append((stroked, cur))
                cur = []
            elif isinstance(seg, se.Line):
                cur.append(("L", complex(*seg.start), complex(*seg.end)))
            elif isinstance(seg, se.CubicBezier):
                cur.append(("C", complex(*seg.start), complex(*seg.control1),
                            complex(*seg.control2), complex(*seg.end)))
            elif isinstance(seg, se.Arc):
                cur.append(("A", complex(*seg.start), (seg.rx + seg.ry) / 2, complex(*seg.end),
                            complex(*seg.center)))
            elif isinstance(seg, se.Close):
                if cur and abs(complex(*seg.start) - complex(*seg.end)) > 1e-6:
                    cur.append(("L", complex(*seg.start), complex(*seg.end)))
        if cur:
            out.append((stroked, cur))
    return out


def tangent(seg, at_end: bool) -> complex:
    k = seg[0]
    if k == "L":
        d = seg[2] - seg[1]
    elif k == "C":
        d = (seg[4] - seg[3]) if at_end else (seg[2] - seg[1])
        if abs(d) < 1e-9:
            d = seg[4] - seg[1]
    else:
        c = seg[4]
        p = seg[3] if at_end else seg[1]
        r = p - c
        d = complex(-r.imag, r.real)
        # orientation: follow the chord direction
        if ((seg[3] - seg[1]) * d.conjugate()).real < 0:
            d = -d
    return d / abs(d) if abs(d) > 1e-12 else 0j


def turn_deg(a: complex, b: complex) -> float:
    if a == 0 or b == 0:
        return 180.0
    return abs(math.degrees(math.atan2((b * a.conjugate()).imag, (b * a.conjugate()).real)))


def count(svg: str, units: float) -> Counter:
    c: Counter = Counter()
    paths = subpaths(svg)
    for stroked, segs in paths:
        for i, s in enumerate(segs):
            if s[0] == "C":
                p0, p1 = s[1], s[4]
                ch = p1 - p0
                if abs(ch) > 1e-9:
                    dist = max(abs(((q - p0) * ch.conjugate()).imag) / abs(ch) for q in (s[2], s[3]))
                    if dist < STRAIGHT_PX * units:
                        c["straight"] += 1
            if i + 1 < len(segs):
                t = segs[i + 1]
                if s[0] == "L" and t[0] == "L" and turn_deg(tangent(s, True), tangent(t, False)) < TURN_DEG:
                    c["collinear"] += 1
                if s[0] == "A" and t[0] == "A" and abs(s[2] - t[2]) < RADIUS_REL * s[2] \
                        and abs(s[4] - t[4]) < RADIUS_REL * s[2]:
                    c["cocircular"] += 1
    ends = [(segs[0][1], tangent(segs[0], False), segs[-1][-2] if segs[-1][0] == "A" else segs[-1][-1],
             tangent(segs[-1], True)) for st, segs in paths if st and segs]
    # open stroked paths meeting end to end with a continuing tangent
    used = set()
    for i, (a0, ta0, a1, ta1) in enumerate(ends):
        for j, (b0, tb0, b1, tb1) in enumerate(ends):
            if i >= j or i in used or j in used:
                continue
            for p, tp, q, tq in ((a1, ta1, b0, tb0), (b1, tb1, a0, ta0), (a1, ta1, b1, -tb1),
                                 (a0, -ta0, b0, tb0)):
                if abs(p - q) < 0.6 * units and turn_deg(tp, tq) < 25.0:
                    c["continue"] += 1
                    used.update((i, j))
                    break
    return c


def one(args):
    exe, it, tier, mode, td, extra = args
    import svgeval
    from PIL import Image
    svgeval.set_tier(tier)
    png, gt = svgeval.item_paths(it)
    out = Path(td) / f"{it['corpus']}__{it['stem']}.svg"
    subprocess.run([exe, str(png), "-o", str(out), "--quiet", "--mode", mode, *extra],
                   check=True, capture_output=True)
    units = 1.0  # trace coordinates are source pixels
    return dict(icon=f"{it['corpus']}/{it['stem']}", family=it["corpus"],
                c=count(out.read_text(encoding="utf-8"), units))


def main() -> None:
    import svgeval
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(ROOT / "target/release/inkvec"))
    ap.add_argument("--tier", default="512ss")
    ap.add_argument("--mode", default="quality")
    ap.add_argument("--extra", default="")
    ap.add_argument("--families", default="")
    a = ap.parse_args()
    items = svgeval.load_sets()["screen"]
    if a.families:
        items = [it for it in items if it["corpus"] in a.families.split(",")]
    with tempfile.TemporaryDirectory() as td, ProcessPoolExecutor(4) as pool:
        rows = list(pool.map(one, [(a.exe, it, a.tier, a.mode, td, a.extra.split()) for it in items]))
    save = {"collinear": 2, "straight": 4, "cocircular": 7, "continue": 0}
    for f in sorted({r["family"] for r in rows}):
        tot = Counter()
        for r in rows:
            if r["family"] == f:
                tot.update(r["c"])
        n = sum(1 for r in rows if r["family"] == f)
        print(f"{f:15s} icons {n:3d}  " + "  ".join(f"{k} {tot[k]:4d} (-{tot[k] * save[k]} params)" for k in save))


if __name__ == "__main__":
    main()
