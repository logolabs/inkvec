"""The human prior, counted: how the artists of the corpus draw.

Chain R of `docs/theory/chain-representation.md` needs numbers: how often a line is
axis-aligned, a coordinate sits on a design grid, a join is smooth, a radius repeats, a
drawing is mirror-symmetric, a shape repeats, a shape continues under the shapes painted
over it. This script counts them from the artists' own files (`bench/data/corpus_svg`). The
numbers are read as written and kept as exact rationals, so no float tolerance decides
whether a line is horizontal or a coordinate is on the grid; relative commands are resolved
exactly. It then runs two simulations of the chain's decision rules on those coordinates,
with a stated noise model standing in for the boundary chain's precision:

* `grid_sim`: empirical-Bayes inference of the design grid (pitch from the raster size and a
  list of viewBox sizes and subdivisions, with an Occam prior that halves per subdivision),
  then snapping each coordinate whose posterior puts it on the grid. Twice: per coordinate
  with one on-grid weight, and hierarchically (tie classes first, the grid on their means,
  the on-grid weight conditioned on the point's role: construction or derived). Scored by
  how many of the artist's exact numbers come back, and how many snaps are wrong.
* `ties_sim`: coordinate ties (a vertical edge shares its x; two shapes share a baseline) by
  the exact one-dimensional clustering under a Chinese-restaurant prior (an O(n^2) dynamic
  program over the sorted values; contiguity of the optimal clusters is lemma R2.5 of the
  chain), scored by pairwise precision and recall of "these two numbers are equal" and by
  the error against the artist's numbers before and after.

    python3 bench/theory/design_prior.py                    # counts, every gate family
    python3 bench/theory/design_prior.py --set screen       # the 246-icon screen set
    python3 bench/theory/design_prior.py --part geometry    # symmetry, repetition, layers
    python3 bench/theory/design_prior.py --part grid_sim
    python3 bench/theory/design_prior.py --part ties_sim
    python3 bench/theory/design_prior.py --part all --json out/design_prior.json

Coordinates in the simulations are in raster pixels with pixel *edges* at integers (the
design's 0 maps to 0); inkvec's own convention puts pixel centres at integers, a fixed
offset of one half that the grid hypothesis carries.
"""

from __future__ import annotations

import argparse
import io
import json
import math
import re
import sys
import xml.etree.ElementTree as ET
from collections import Counter, defaultdict
from concurrent.futures import ProcessPoolExecutor
from fractions import Fraction
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))

CORPUS = ROOT / "bench" / "data" / "corpus_svg"
GATE_FAMILIES = ["lucide", "material-icons", "simple-icons", "openmoji", "twemoji",
                 "noto-emoji", "synthetic"]

# --------------------------------------------------------------------------- parsing

PAINT_ATTRS = ("fill", "stroke", "stroke-width", "opacity", "fill-opacity",
               "stroke-opacity", "stroke-linecap", "stroke-linejoin", "display")
NONPAINT = {"defs", "clipPath", "mask", "symbol", "linearGradient", "radialGradient",
            "filter", "pattern", "marker", "title", "desc", "metadata", "style"}
NUM_RE = re.compile(r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")
ARGS = {"M": 2, "L": 2, "T": 2, "H": 1, "V": 1, "C": 6, "S": 4, "Q": 4, "A": 7, "Z": 0}
GATE = {"L": 2, "Q": 4, "C": 6, "A": 7}
TURN_BINS = [(0, 0.5), (0.5, 2), (2, 5), (5, 10), (10, 30), (30, 60), (60, 90.01), (90.01, 181)]


def frac(text: str) -> Fraction:
    t = text.strip().removesuffix("px")
    if t.endswith("."):
        t = t[:-1]
    return Fraction(t)


def scan_path(d: str) -> list[tuple[str, list[Fraction]]]:
    """Path data as (command, arguments), arguments exact; arc flags read one character each,
    so the compact form `a2 2 0 012-2` reads as SVG says it does."""
    out: list[tuple[str, list[Fraction]]] = []
    i, n, cmd = 0, len(d), None

    def skip(i: int) -> int:
        while i < n and d[i] in " \t\r\n,":
            i += 1
        return i

    while True:
        i = skip(i)
        if i >= n:
            break
        if d[i].isalpha():
            cmd = d[i]
            i += 1
            if cmd in "Zz":
                out.append((cmd, []))
                continue
        elif cmd is None or cmd in "Zz":
            raise ValueError(f"bad path data at {i}")
        k = ARGS[cmd.upper()]
        args: list[Fraction] = []
        for a in range(k):
            i = skip(i)
            if cmd in "Aa" and a in (3, 4):
                args.append(Fraction(int(d[i])))
                i += 1
            else:
                m = NUM_RE.match(d, i)
                if not m:
                    raise ValueError(f"bad number at {i}")
                args.append(frac(m.group(0)))
                i = m.end()
        out.append((cmd, args))
        if cmd == "M":
            cmd = "L"
        elif cmd == "m":
            cmd = "l"
    return out


def to_subpaths(cmds) -> list[dict]:
    """Absolute, exact segments per subpath. A segment is (kind, letter, p0, ..., p1) with
    kind L, Q, C or A; a closing `Z` with a gap writes ('L', 'Z', cur, start)."""
    subs: list[dict] = []
    cur = start = (Fraction(0), Fraction(0))
    sp = None
    last = None  # (kind, control) for S and T reflection

    def ensure():
        nonlocal sp
        if sp is None:
            sp = {"start": cur, "segs": [], "closed": False}
            subs.append(sp)
        return sp

    for cmd, a in cmds:
        U, rel = cmd.upper(), cmd.islower()
        ox, oy = cur if rel else (Fraction(0), Fraction(0))
        if U == "M":
            cur = start = (ox + a[0], oy + a[1])
            sp = {"start": cur, "segs": [], "closed": False}
            subs.append(sp)
            last = None
            continue
        if U == "Z":
            s = ensure()
            s["closed"] = True
            if cur != start:
                s["segs"].append(("L", "Z", cur, start))
            cur = start
            sp = None
            last = None
            continue
        s = ensure()
        if U == "L":
            p = (ox + a[0], oy + a[1])
            s["segs"].append(("L", cmd, cur, p))
            cur, last = p, None
        elif U == "H":
            p = (ox + a[0] if rel else a[0], cur[1])
            s["segs"].append(("L", cmd, cur, p))
            cur, last = p, None
        elif U == "V":
            p = (cur[0], oy + a[0] if rel else a[0])
            s["segs"].append(("L", cmd, cur, p))
            cur, last = p, None
        elif U in "CS":
            if U == "C":
                c1 = (ox + a[0], oy + a[1])
                c2, p = (ox + a[2], oy + a[3]), (ox + a[4], oy + a[5])
            else:
                c1 = (2 * cur[0] - last[1][0], 2 * cur[1] - last[1][1]) if last and last[0] == "C" else cur
                c2, p = (ox + a[0], oy + a[1]), (ox + a[2], oy + a[3])
            s["segs"].append(("C", cmd, cur, c1, c2, p))
            cur, last = p, ("C", c2)
        elif U in "QT":
            if U == "Q":
                c, p = (ox + a[0], oy + a[1]), (ox + a[2], oy + a[3])
            else:
                c = (2 * cur[0] - last[1][0], 2 * cur[1] - last[1][1]) if last and last[0] == "Q" else cur
                p = (ox + a[0], oy + a[1])
            s["segs"].append(("Q", cmd, cur, c, p))
            cur, last = p, ("Q", c)
        elif U == "A":
            p = (ox + a[5], oy + a[6])
            s["segs"].append(("A", cmd, cur, abs(a[0]), abs(a[1]), a[2], int(a[3]), int(a[4]), p))
            cur, last = p, None
    return subs


def style_of(el) -> dict:
    st = {k: el.get(k).strip() for k in PAINT_ATTRS if el.get(k) is not None}
    for part in (el.get("style") or "").split(";"):
        if ":" in part:
            k, v = part.split(":", 1)
            if k.strip() in PAINT_ATTRS:
                st[k.strip()] = v.strip()
    return st


def num_attr(el, name, default="0") -> Fraction:
    v = el.get(name)
    try:
        return frac(v) if v is not None else frac(default)
    except (ValueError, ZeroDivisionError):
        return frac(default)


class Element:
    __slots__ = ("tag", "el", "st", "tf", "subs", "prim", "typed", "gate")

    def __init__(self, tag, el, st, tf):
        self.tag, self.el, self.st, self.tf = tag, el, st, tf
        self.subs: list[dict] = []
        self.prim: dict = {}
        self.typed = 0  # numbers the artist typed for the geometry
        self.gate = 0   # the gate's parameter count (bench/inkvec_bench/svgmodel.py rules)

    @property
    def fill(self) -> str:
        return self.st.get("fill", "#000").lower()

    @property
    def stroke(self) -> str:
        return self.st.get("stroke", "none").lower()

    @property
    def stroke_width(self) -> Fraction:
        try:
            return frac(self.st.get("stroke-width", "1"))
        except (ValueError, ZeroDivisionError):
            return Fraction(1)

    @property
    def filled(self) -> bool:
        return self.fill not in ("none", "transparent")

    @property
    def stroked(self) -> bool:
        return self.stroke not in ("none", "transparent") and self.stroke_width > 0

    @property
    def opaque(self) -> bool:
        for k in ("opacity", "fill-opacity"):
            try:
                if float(self.st.get(k, "1")) < 0.999:
                    return False
            except ValueError:
                pass
        return True


def parse_doc(text: str) -> tuple[list[Element], Fraction]:
    root = ET.fromstring(text)
    vb = (root.get("viewBox") or "0 0 24 24").replace(",", " ").split()
    V = max(frac(vb[2]), frac(vb[3]))
    out: list[Element] = []

    def walk(node, inherited: dict, tf: bool):
        for ch in node:
            tag = ch.tag.split("}")[-1]
            if tag in NONPAINT:
                continue
            st = dict(inherited)
            st.update(style_of(ch))
            if st.get("display") == "none":
                continue
            t = tf or ch.get("transform") is not None
            if tag in ("g", "svg", "a", "switch"):
                walk(ch, st, t)
            elif tag in ("path", "rect", "circle", "ellipse", "line", "polyline", "polygon", "use"):
                out.append(Element(tag, ch, st, t))

    walk(root, style_of(root), False)
    for e in out:
        el = e.el
        if e.tag == "path":
            cmds = scan_path(el.get("d") or "")
            e.subs = to_subpaths(cmds)
            e.typed = sum(len(a) for _, a in cmds)
            e.gate = sum(GATE[s[0]] for sp in e.subs for s in sp["segs"] if s[1] not in "Zz")
        elif e.tag in ("polyline", "polygon"):
            nums = [frac(x) for x in NUM_RE.findall(el.get("points") or "")]
            pts = list(zip(nums[0::2], nums[1::2]))
            if pts:
                segs = [("L", "L", p, q) for p, q in zip(pts, pts[1:])]
                closed = e.tag == "polygon"
                if closed and pts[-1] != pts[0]:
                    segs.append(("L", "Z", pts[-1], pts[0]))
                e.subs = [{"start": pts[0], "segs": segs, "closed": closed}]
            e.typed = len(nums)
            e.gate = max(4, 2 * (len(pts) - 1)) if e.tag == "polygon" else 2 * len(pts)
        elif e.tag == "line":
            p = (num_attr(el, "x1"), num_attr(el, "y1"))
            q = (num_attr(el, "x2"), num_attr(el, "y2"))
            e.subs = [{"start": p, "segs": [("L", "L", p, q)], "closed": False}]
            e.typed, e.gate = 4, 4
        elif e.tag == "rect":
            rx, ry = el.get("rx"), el.get("ry")
            rxv = num_attr(el, "rx") if rx is not None else (num_attr(el, "ry") if ry is not None else Fraction(0))
            ryv = num_attr(el, "ry") if ry is not None else rxv
            e.prim = dict(x=num_attr(el, "x"), y=num_attr(el, "y"), w=num_attr(el, "width"),
                          h=num_attr(el, "height"), rx=rxv, ry=ryv)
            e.typed = sum(el.get(k) is not None for k in ("x", "y", "width", "height", "rx", "ry"))
            e.gate = 6
        elif e.tag == "circle":
            e.prim = dict(cx=num_attr(el, "cx"), cy=num_attr(el, "cy"), r=num_attr(el, "r"))
            e.typed = sum(el.get(k) is not None for k in ("cx", "cy", "r"))
            e.gate = 3
        elif e.tag == "ellipse":
            e.prim = dict(cx=num_attr(el, "cx"), cy=num_attr(el, "cy"), rx=num_attr(el, "rx"),
                          ry=num_attr(el, "ry"))
            e.typed = sum(el.get(k) is not None for k in ("cx", "cy", "rx", "ry"))
            e.gate = 4
        elif e.tag == "use":
            e.gate = 6
    return out, V


# --------------------------------------------------------------------------- geometry helpers


def vec(p, q):
    return (q[0] - p[0], q[1] - p[1])


def nonzero(v) -> bool:
    return v[0] != 0 or v[1] != 0


def arc_center(p0, rx, ry, phi_deg, fa, fs, p1):
    """SVG arc endpoint form to centre form (SVG 1.1 F.6.5), floats."""
    x1, y1 = float(p0[0]), float(p0[1])
    x2, y2 = float(p1[0]), float(p1[1])
    rx, ry = float(rx), float(ry)
    phi = math.radians(float(phi_deg))
    c, s = math.cos(phi), math.sin(phi)
    dx2, dy2 = (x1 - x2) / 2, (y1 - y2) / 2
    x1p, y1p = c * dx2 + s * dy2, -s * dx2 + c * dy2
    if rx == 0 or ry == 0:
        return None
    lam = x1p ** 2 / rx ** 2 + y1p ** 2 / ry ** 2
    if lam > 1:
        rx, ry = rx * math.sqrt(lam), ry * math.sqrt(lam)
    num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p
    den = rx * rx * y1p * y1p + ry * ry * x1p * x1p
    co = math.sqrt(max(0.0, num / den)) if den > 0 else 0.0
    if fa == fs:
        co = -co
    cxp, cyp = co * rx * y1p / ry, -co * ry * x1p / rx

    def ang(ux, uy, vx, vy):
        return math.atan2(ux * vy - uy * vx, ux * vx + uy * vy)

    th1 = ang(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry)
    dth = ang((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry)
    if not fs and dth > 0:
        dth -= 2 * math.pi
    elif fs and dth < 0:
        dth += 2 * math.pi
    return rx, ry, phi, th1, dth


def arc_tangent(rx, ry, phi, th, sgn):
    c, s = math.cos(phi), math.sin(phi)
    tx = -rx * math.sin(th) * c - ry * math.cos(th) * s
    ty = -rx * math.sin(th) * s + ry * math.cos(th) * c
    return (sgn * tx, sgn * ty)


def tangents(seg):
    """(start tangent, end tangent) of a segment; exact for L, Q, C; float for A."""
    k = seg[0]
    if k == "L":
        v = vec(seg[2], seg[3])
        return v, v
    if k == "C":
        p0, c1, c2, p3 = seg[2:6]
        t0 = next((vec(p0, q) for q in (c1, c2, p3) if nonzero(vec(p0, q))), (0, 0))
        t1 = next((vec(q, p3) for q in (c2, c1, p0) if nonzero(vec(q, p3))), (0, 0))
        return t0, t1
    if k == "Q":
        p0, c, p1 = seg[2:5]
        t0 = vec(p0, c) if nonzero(vec(p0, c)) else vec(p0, p1)
        t1 = vec(c, p1) if nonzero(vec(c, p1)) else vec(p0, p1)
        return t0, t1
    ac = arc_center(seg[2], seg[3], seg[4], seg[5], seg[6], seg[7], seg[8])
    if ac is None:
        v = vec(seg[2], seg[8])
        return v, v
    rx, ry, phi, th1, dth = ac
    sg = 1.0 if dth >= 0 else -1.0
    return arc_tangent(rx, ry, phi, th1, sg), arc_tangent(rx, ry, phi, th1 + dth, sg)


def seg_length_nonzero(seg) -> bool:
    pts = seg[2:6] if seg[0] == "C" else (seg[2:5] if seg[0] == "Q" else (seg[2], seg[-1]))
    return any(p != pts[0] for p in pts[1:])


def turn_deg(t_in, t_out) -> float:
    a = (float(t_in[0]), float(t_in[1]))
    b = (float(t_out[0]), float(t_out[1]))
    cr = a[0] * b[1] - a[1] * b[0]
    dt = a[0] * b[0] + a[1] * b[1]
    return abs(math.degrees(math.atan2(cr, dt)))


def exact_g1(t_in, t_out) -> bool:
    if any(isinstance(x, float) for x in (*t_in, *t_out)):
        return turn_deg(t_in, t_out) < 1e-6
    return t_in[0] * t_out[1] - t_in[1] * t_out[0] == 0 and t_in[0] * t_out[0] + t_in[1] * t_out[1] > 0


def cubic_is_arc(seg) -> bool:
    """A cubic written as a circular arc: equal handles, symmetric, of the length
    `(4/3)·tan(θ/4)·r` for its turn θ (the standard arc approximation), within 10 %."""
    p0, c1, c2, p3 = [(float(p[0]), float(p[1])) for p in seg[2:6]]
    h0, h1 = (c1[0] - p0[0], c1[1] - p0[1]), (p3[0] - c2[0], p3[1] - c2[1])
    l0, l1 = math.hypot(*h0), math.hypot(*h1)
    if l0 < 1e-9 or l1 < 1e-9 or abs(l0 - l1) > 0.1 * max(l0, l1):
        return False
    th = math.radians(turn_deg(h0, h1))
    if th < math.radians(5) or th > math.radians(179):
        return False
    chord = math.hypot(p3[0] - p0[0], p3[1] - p0[1])
    r = chord / (2 * math.sin(th / 2))
    want = 4 / 3 * math.tan(th / 4) * r
    # the chord must make equal angles with both handles (symmetric arc)
    cx, cy = p3[0] - p0[0], p3[1] - p0[1]
    a0 = turn_deg(h0, (cx, cy))
    a1 = turn_deg((cx, cy), h1)
    return abs(a0 - a1) < 2.0 and abs((l0 + l1) / 2 - want) < 0.1 * want


def cubic_is_straight(seg) -> bool:
    p0, c1, c2, p3 = [(float(p[0]), float(p[1])) for p in seg[2:6]]
    dx, dy = p3[0] - p0[0], p3[1] - p0[1]
    L = math.hypot(dx, dy)
    if L < 1e-9:
        return False
    for c in (c1, c2):
        if abs((c[0] - p0[0]) * dy - (c[1] - p0[1]) * dx) / L > 0.01 * L:
            return False
    return True


def denominator_class(v: Fraction) -> tuple[int, int]:
    """(dyadic exponent or 99, decimal digits or 99) of an exact number."""
    d = v.denominator
    a2 = 0
    while d % 2 == 0:
        d //= 2
        a2 += 1
    a5 = 0
    while d % 5 == 0:
        d //= 5
        a5 += 1
    if d != 1:
        return 99, 99
    return (a2 if a5 == 0 else 99), max(a2, a5)


def on_curve_points(sp) -> list:
    """The subpath's on-curve points, each once: the start and every segment end, except a
    final end that returns to the start."""
    pts = [sp["start"]]
    for s in sp["segs"]:
        pts.append(s[-1])
    if len(pts) > 1 and pts[-1] == pts[0]:
        pts.pop()
    return pts


def control_points(sp) -> list:
    out = []
    for s in sp["segs"]:
        if s[0] == "C":
            out += [s[3], s[4]]
        elif s[0] == "Q":
            out.append(s[3])
    return out


# --------------------------------------------------------------------------- counting


def count_icon(path: Path) -> dict:
    """Every per-icon count the prior needs, from one artist file."""
    text = path.read_text(encoding="utf-8")
    try:
        els, V = parse_doc(text)
    except Exception as ex:  # noqa: BLE001
        return {"error": f"{type(ex).__name__}: {ex}"}
    c: Counter = Counter()
    dot = Fraction(V) / 480  # a stroke "dot" (lucide's `h.01`), not a drawn line
    xs, ys, radii, widths = [], [], [], []
    ends, ctrls = [], []
    classed: list[tuple[str, Fraction]] = []

    def is_dot(s) -> bool:
        return s[0] == "L" and abs(s[3][0] - s[2][0]) + abs(s[3][1] - s[2][1]) < dot

    coords: list[tuple[str, str, Fraction]] = []  # (axis, role, value) of every position

    def record(p, cls: str) -> None:
        xs.append(p[0])
        ys.append(p[1])
        ends.extend((p[0], p[1]))
        classed.extend(((cls, p[0]), (cls, p[1])))
        coords.extend((("x", cls, p[0]), ("y", cls, p[1])))

    for e in els:
        c["el:" + e.tag] += 1
        c["gate"] += e.gate
        c["typed"] += e.typed
        if e.stroked:
            c["el_stroked"] += 1
            widths.append(e.stroke_width)
        if e.filled:
            c["el_filled"] += 1
        if e.tf:
            c["el_transformed"] += 1
        if e.tag == "rect":
            c["rect_rounded"] += int(e.prim["rx"] > 0)
            if e.prim["rx"] > 0:
                radii.append(e.prim["rx"])
            for k in ("x", "w"):
                xs.append(e.prim["x"] if k == "x" else e.prim["x"] + e.prim["w"])
            ys += [e.prim["y"], e.prim["y"] + e.prim["h"]]
            ends += [e.prim["x"], e.prim["y"], e.prim["w"], e.prim["h"]]
            classed += [("prim", e.prim[k]) for k in ("x", "y", "w", "h")]
            coords += [("x", "prim", e.prim["x"]), ("x", "prim", e.prim["x"] + e.prim["w"]),
                       ("y", "prim", e.prim["y"]), ("y", "prim", e.prim["y"] + e.prim["h"])]
        elif e.tag in ("circle", "ellipse"):
            radii += [e.prim["r"]] if e.tag == "circle" else [e.prim["rx"], e.prim["ry"]]
            xs.append(e.prim["cx"])
            ys.append(e.prim["cy"])
            ends += [e.prim["cx"], e.prim["cy"]]
            classed += [("prim", e.prim[k]) for k in e.prim]
            coords += [("x", "prim", e.prim["cx"]), ("y", "prim", e.prim["cy"])]
        for sp in e.subs:
            for p in control_points(sp):
                ctrls += [p[0], p[1]]
            segs = [s for s in sp["segs"] if seg_length_nonzero(s) and not is_dot(s)]
            c["line_dot"] += sum(1 for s in sp["segs"] if seg_length_nonzero(s) and is_dot(s))
            if not segs:  # a dot (lucide's `M6 8h.01`) or a lone move: one free point
                record(sp["start"], "end")
                continue
            for s in segs:
                k, letter = s[0], s[1]
                if letter in "Zz":
                    c["seg:Z-line"] += 1
                else:
                    c["seg:" + k] += 1
                    c["cmd:" + letter.upper()] += 1
                if k == "L":
                    dx, dy = vec(s[2], s[3])
                    c["line"] += 1
                    if dx == 0 or dy == 0:
                        c["line_axis"] += 1
                        c["line_axis_hv"] += int(letter.upper() in "HV")
                    elif abs(dx) == abs(dy):
                        c["line_45"] += 1
                    else:
                        ang = math.degrees(math.atan2(abs(float(dy)), abs(float(dx))))
                        if min(ang, 90 - ang) <= 0.5:
                            c["line_near_axis"] += 1
                        elif abs(ang - 45) <= 0.5:
                            c["line_near_45"] += 1
                        else:
                            c["line_other"] += 1
                elif k == "C":
                    c["cubic"] += 1
                    c["cubic_arc"] += int(cubic_is_arc(s))
                    c["cubic_straight"] += int(cubic_is_straight(s))
                elif k == "Q":
                    c["quad"] += 1
                elif k == "A":
                    c["arc"] += 1
                    c["arc_circular"] += int(s[3] == s[4])
                    if s[3] == s[4]:
                        radii.append(s[3])
                    else:
                        radii += [s[3], s[4]]
                    ac = arc_center(s[2], s[3], s[4], s[5], s[6], s[7], s[8])
                    if ac:
                        sw = abs(math.degrees(ac[4]))
                        c["arc_sweep_le90"] += int(sw <= 90.5)
                        c["arc_sweep_le120"] += int(sw <= 120.5)
                        c["arc_sweep_le180"] += int(sw <= 180.5)
            # joins: consecutive segments, and the wrap-around of a closed subpath; each
            # on-curve point is classed by the join it sits at
            pairs = list(zip(segs, segs[1:]))
            if sp["closed"] and len(segs) >= 2:
                pairs.append((segs[-1], segs[0]))
            else:
                record(segs[0][2], "end")
                record(segs[-1][-1], "end")
            for a, b in pairs:
                key = "-".join(sorted((a[0], b[0])))
                t_in, t_out = tangents(a)[1], tangents(b)[0]
                c["join:" + key] += 1
                t = turn_deg(t_in, t_out)
                curved = key != "L-L"
                for lo, hi in TURN_BINS:
                    if lo <= t < hi:
                        c[f"turn{'_curve' if curved else '_line'}:{lo}"] += 1
                if exact_g1(t_in, t_out):
                    c["join_g1:" + key] += 1
                elif t < 2.0:
                    c["join_near:" + key] += 1
                elif t > 178.0:
                    c["join_cusp:" + key] += 1
                record(a[-1], "smooth" if t < 2.0 else ("corner" if key == "L-L" else "corner_curve"))

    # coordinates on a grid, as written
    for name, vals in (("end", ends), ("ctrl", ctrls)):
        for v in vals:
            dy2, dec = denominator_class(v)
            c[f"{name}_n"] += 1
            for kexp in range(5):
                c[f"{name}_dy{kexp}"] += int(dy2 <= kexp)
            for d in range(4):
                c[f"{name}_dec{d}"] += int(dec <= d)
    # the icon's own grid: the coarsest dyadic unit covering 95 % of its on-curve numbers
    grid = None
    construction = [v for cls, v in classed if cls in ("corner", "end", "prim")] or ends
    if construction:
        for kexp in range(5):
            share = sum(denominator_class(v)[0] <= kexp for v in construction) / len(construction)
            if share >= 0.8:
                grid = kexp
                break
    for cls, v in classed:
        c[f"cls_{cls}_n"] += 1
        c[f"cls_{cls}_half"] += int(denominator_class(v)[0] <= 1)
        c[f"cls_{cls}_icongrid"] += int(grid is not None and denominator_class(v)[0] <= grid)
        c[f"cls_{cls}_dec2"] += int(denominator_class(v)[1] <= 2)
    # repeated radii and widths
    rc = Counter(radii)
    c["radius_n"] = len(radii)
    c["radius_repeat"] = sum(n for n in rc.values() if n >= 2)
    c["radius_distinct"] = len(rc)
    c["width_distinct"] = len(set(widths))
    c["stroked_icon"] = int(bool(widths))
    xk, yk = len(set(xs)), len(set(ys))
    return {
        "counts": dict(c),
        "V": float(V),
        "grid": grid,
        "xs": [str(v) for v in xs],
        "ys": [str(v) for v in ys],
        "nx": len(xs), "kx": xk, "ny": len(ys), "ky": yk,
        "radii": [str(v) for v in radii],
        "coords": [(a, r, str(v)) for a, r, v in coords],
    }


def icons_for(set_name: str) -> list[tuple[str, Path]]:
    if set_name == "corpus":
        return [(f, p) for f in GATE_FAMILIES for p in sorted((CORPUS / f).glob("*.svg"))]
    import svgeval
    items = svgeval.load_sets()[set_name]
    return [(it["corpus"], CORPUS / it["corpus"] / f"{it['stem']}.svg") for it in items]


def _count_one(arg):
    fam, p = arg
    r = count_icon(p)
    r["family"], r["icon"] = fam, p.stem
    return r


def crp_alpha(rows: list[tuple[int, int]]) -> float:
    """Maximum-likelihood concentration of a Chinese-restaurant process from (n, K) pairs:
    the Ewens likelihood K·ln α + lnΓ(α) − lnΓ(α + n), summed over icons."""
    rows = [(n, k) for n, k in rows if n > 0]
    if not rows:
        return float("nan")

    def ll(la):
        a = math.exp(la)
        return sum(k * la + math.lgamma(a) - math.lgamma(a + n) for n, k in rows)

    lo, hi = -6.0, 10.0
    g = (math.sqrt(5) - 1) / 2
    for _ in range(80):
        m1, m2 = hi - g * (hi - lo), lo + g * (hi - lo)
        if ll(m1) < ll(m2):
            lo = m1
        else:
            hi = m2
    return math.exp((lo + hi) / 2)


def prior_code_nats(vals: list[Fraction], V: float, grid: int | None, alpha: float,
                    pi_on: float, n_px: int = 512, prec_px: float = 0.1) -> list[float]:
    """Nats per value under a Chinese-restaurant prior whose base measure is the icon's grid
    (weight `pi_on`) mixed with a uniform at inkvec's flat precision; values in order."""
    flat = math.log(n_px / prec_px)
    seen: Counter = Counter()
    out = []
    for i, v in enumerate(vals):
        if seen[v]:
            out.append(-math.log(seen[v] / (i + alpha)))
        else:
            if grid is not None and denominator_class(v)[0] <= grid and pi_on > 0:
                base = -math.log(pi_on) + math.log(V * 2 ** grid)
            else:
                base = -math.log(max(1e-9, 1 - pi_on if grid is not None else 1.0)) + flat
            out.append(-math.log(alpha / (i + alpha)) + base)
        seen[v] += 1
    return out


def summarize_counts(rows: list[dict]) -> dict:
    fams = [f for f in GATE_FAMILIES if any(r["family"] == f for r in rows)]
    res = {}
    for fam in fams + ["ALL"]:
        rs = [r for r in rows if "counts" in r and (fam == "ALL" or r["family"] == fam)]
        C: Counter = Counter()
        for r in rs:
            C.update(r["counts"])
        n_icons = len(rs)

        def ratio(a, b):
            return C[a] / C[b] if C[b] else float("nan")

        joins = {}
        for key in ("L-L", "C-L", "C-C", "A-L", "A-C", "A-A"):
            nj = C["join:" + key]
            if nj:
                joins[key] = dict(n=nj, g1=C["join_g1:" + key] / nj,
                                  near=C["join_near:" + key] / nj,
                                  cusp=C["join_cusp:" + key] / nj)
        nseg = sum(C["seg:" + k] for k in ("L", "Q", "C", "A"))
        grid_icons = Counter(r["grid"] for r in rs)
        xs_rows = [(r["nx"], r["kx"]) for r in rs]
        ys_rows = [(r["ny"], r["ky"]) for r in rs]
        ax, ay = crp_alpha(xs_rows), crp_alpha(ys_rows)
        # the share of an on-grid icon's numbers that are on its grid
        on_share = []
        for r in rs:
            if r["grid"] is not None and r["xs"]:
                vals = [Fraction(v) for v in r["xs"] + r["ys"]]
                on_share.append(np.mean([denominator_class(v)[0] <= r["grid"] for v in vals]))
        pi_on = float(np.mean(on_share)) if on_share else 0.0
        code = []
        for r in rs:
            for key, a in (("xs", ax), ("ys", ay)):
                vals = [Fraction(v) for v in r[key]]
                code += prior_code_nats(vals, r["V"], r["grid"], a, pi_on)
        res[fam] = dict(
            icons=n_icons,
            gate_per_icon=C["gate"] / max(1, n_icons),
            typed_over_gate=ratio("typed", "gate"),
            elements=C["el:path"] + C["el:rect"] + C["el:circle"] + C["el:ellipse"]
            + C["el:line"] + C["el:polyline"] + C["el:polygon"] + C["el:use"],
            el_kinds={k[3:]: C[k] for k in C if k.startswith("el:")},
            stroked_elements=ratio("el_stroked", "el:path") if C["el:path"] else float("nan"),
            stroked_icons=C["stroked_icon"] / max(1, n_icons),
            rect_rounded=ratio("rect_rounded", "el:rect"),
            seg_share={k: C["seg:" + k] / nseg for k in ("L", "Q", "C", "A")} if nseg else {},
            hv_of_axis_lines=ratio("line_axis_hv", "line_axis"),
            cmd_S=C["cmd:S"], cmd_T=C["cmd:T"],
            lines=C["line"],
            line_axis=ratio("line_axis", "line"),
            line_45=ratio("line_45", "line"),
            line_near_axis=ratio("line_near_axis", "line"),
            line_near_45=ratio("line_near_45", "line"),
            line_other=ratio("line_other", "line"),
            line_dots=C["line_dot"],
            cubics=C["cubic"],
            cubic_arc=ratio("cubic_arc", "cubic"),
            cubic_straight=ratio("cubic_straight", "cubic"),
            arcs=C["arc"],
            arc_circular=ratio("arc_circular", "arc"),
            arc_le90=ratio("arc_sweep_le90", "arc"),
            arc_le120=ratio("arc_sweep_le120", "arc"),
            arc_le180=ratio("arc_sweep_le180", "arc"),
            joins=joins,
            end_on_grid={u: ratio(f"end_dy{k}", "end_n") for k, u in
                         enumerate(("1", "1/2", "1/4", "1/8", "1/16"))},
            end_decimals={d: ratio(f"end_dec{d}", "end_n") for d in range(4)},
            ctrl_on_grid={u: ratio(f"ctrl_dy{k}", "ctrl_n") for k, u in
                          enumerate(("1", "1/2", "1/4", "1/8", "1/16"))},
            ctrl_decimals={d: ratio(f"ctrl_dec{d}", "ctrl_n") for d in range(4)},
            icon_grid={("none" if g is None else f"1/{2 ** g}"): n / max(1, n_icons)
                       for g, n in sorted(grid_icons.items(), key=lambda t: (t[0] is None, t[0] or 0))},
            pi_on_grid=pi_on,
            radius_repeat=ratio("radius_repeat", "radius_n"),
            radii=C["radius_n"],
            widths_per_stroked_icon=C["width_distinct"] / max(1, C["stroked_icon"]),
            x_reuse=1 - sum(k for _, k in xs_rows) / max(1, sum(n for n, _ in xs_rows)),
            y_reuse=1 - sum(k for _, k in ys_rows) / max(1, sum(n for n, _ in ys_rows)),
            crp_alpha_x=ax, crp_alpha_y=ay,
            point_classes={cls: dict(n=C[f"cls_{cls}_n"], half=ratio(f"cls_{cls}_half", f"cls_{cls}_n"),
                                     icon_grid=ratio(f"cls_{cls}_icongrid", f"cls_{cls}_n"),
                                     dec2=ratio(f"cls_{cls}_dec2", f"cls_{cls}_n"))
                           for cls in ("corner", "end", "prim", "corner_curve", "smooth")},
            turn_curve={f"{lo}": C[f"turn_curve:{lo}"] / max(1, sum(C[f"turn_curve:{b[0]}"] for b in TURN_BINS))
                        for lo, _ in TURN_BINS},
            turn_line={f"{lo}": C[f"turn_line:{lo}"] / max(1, sum(C[f"turn_line:{b[0]}"] for b in TURN_BINS))
                       for lo, _ in TURN_BINS},
            prior_nats_per_coord=float(np.mean(code)) if code else float("nan"),
            flat_nats_per_coord=math.log(512 / 0.1),
        )
    return res


# --------------------------------------------------------------------------- geometry (shapely)


def shapes_of(text: str):
    """Painted elements in paint order as shapely geometries (fill ∪ stroke), with the gate
    parameter count of each; svgelements resolves transforms and inheritance."""
    import shapely
    import svgelements as se
    from shapely.geometry import LineString, Polygon
    from shapely.ops import unary_union
    doc = se.SVG.parse(io.StringIO(text))
    out = []
    for el in doc.elements():
        if not isinstance(el, se.Shape) or isinstance(el, (se.SVG, se.Group)):
            continue
        try:
            segs = list(se.Path(el).segments())
        except Exception:  # noqa: BLE001
            continue
        rings, cur, params = [], [], 0
        opened = []
        for s in segs:
            nm = type(s).__name__
            if nm == "Move":
                if len(cur) >= 2:
                    opened.append(cur)
                cur = [(s.end.x, s.end.y)]
                continue
            if nm == "Close":
                if len(cur) >= 3:
                    rings.append(cur)
                cur = []
                continue
            params += {"Line": 2, "QuadraticBezier": 4, "CubicBezier": 6, "Arc": 7}.get(nm, 0)
            n = 1 if nm == "Line" else 12
            for i in range(1, n + 1):
                p = s.point(i / n)
                cur.append((p.x, p.y))
        if len(cur) >= 2:
            opened.append(cur)
        kind = type(el).__name__
        prim = {"Circle": 3, "Ellipse": 4, "Rect": 6, "SimpleLine": 4}.get(kind)
        if prim is not None:
            params = prim
        fill = el.fill if el.fill is not None and el.fill.value is not None else None
        geo = []
        if fill is not None and fill.alpha != 0:
            polys = []
            for r in rings + [o for o in opened if len(o) >= 3]:
                pg = Polygon(r).buffer(0)
                if not pg.is_empty:
                    polys.append(pg)
            if polys:
                g = polys[0]
                for pg in polys[1:]:
                    g = g.symmetric_difference(pg)
                geo.append(g)
        sw = float(el.stroke_width or 0) if el.stroke is not None and el.stroke.value is not None else 0.0
        if sw > 0 and el.stroke.alpha != 0:
            cap = {"round": 1, "square": 3}.get(str(el.values.get("stroke-linecap", "butt")), 2)
            join = {"round": 1, "bevel": 3}.get(str(el.values.get("stroke-linejoin", "miter")), 2)
            for r in rings:
                geo.append(LineString(r + [r[0]]).buffer(sw / 2, cap_style=cap, join_style=join))
            for o in opened:
                geo.append(LineString(o).buffer(sw / 2, cap_style=cap, join_style=join))
        if not geo:
            continue
        try:
            g = shapely.set_precision(unary_union(geo).buffer(0), 1e-5)
        except Exception:  # noqa: BLE001
            continue
        if g.is_empty or g.area <= 0:
            continue
        op = 1.0
        for k in ("opacity", "fill-opacity"):
            try:
                op *= float(el.values.get(k, 1))
            except (TypeError, ValueError):
                pass
        opaque = op > 0.999 and (fill is None or fill.alpha == 255)
        paint = (str(fill) if fill is not None and fill.alpha != 0 else None,
                 str(el.stroke) if sw > 0 else None)
        out.append(dict(geo=g, params=params, opaque=opaque, kind=kind, paint=paint))
    return out


def iou(a, b) -> float:
    try:
        i = a.intersection(b).area
    except Exception:  # noqa: BLE001
        return 0.0
    u = a.area + b.area - i
    return i / u if u > 0 else 0.0


def geometry_icon(arg) -> dict:
    fam, path = arg
    from shapely import affinity
    from shapely.ops import unary_union
    text = path.read_text(encoding="utf-8")
    try:
        sh = shapes_of(text)
    except Exception as ex:  # noqa: BLE001
        return {"family": fam, "icon": path.stem, "error": str(ex)}
    res = {"family": fam, "icon": path.stem, "n": len(sh)}
    total = sum(s["params"] for s in sh) or 1
    # layers: how much of each element the elements painted after it, in another paint, cover
    later = []
    occ = []
    for s in reversed(sh):
        g = s["geo"]
        f, borrowed = 0.0, 0.0
        cands = [h["geo"] for h in later if h["opaque"] and h["paint"] != s["paint"]
                 and h["geo"].intersects(g)]
        if cands:
            try:
                union = unary_union(cands)
                f = g.intersection(union).area / g.area
                if 0.02 < f < 0.98:
                    vis = g.difference(union)
                    blen = vis.boundary.length
                    shared = vis.boundary.intersection(
                        union.boundary.buffer(1e-3 * math.sqrt(g.area))).length
                    borrowed = shared / blen if blen > 0 else 0.0
            except Exception:  # noqa: BLE001
                pass
        occ.append((f, s["params"], borrowed))
        later.append(s)
    occ.reverse()
    res["partial"] = sum(1 for f, _, _ in occ if 0.02 < f < 0.98)
    res["hidden"] = sum(1 for f, _, _ in occ if f >= 0.98)
    res["partial_params"] = sum(p for f, p, _ in occ if 0.02 < f < 0.98) / total
    res["hidden_params"] = sum(p for f, p, _ in occ if f >= 0.98) / total
    res["borrowed"] = [b for f, _, b in occ if 0.02 < f < 0.98]
    # symmetry, repetition: elements against their own mirror and against each other
    self_sym = mirror_pair = repeat = 0
    save_use = 0
    cents = [s["geo"].centroid for s in sh]
    used = set()
    for i, s in enumerate(sh):
        g = s["geo"]
        c = cents[i]
        if iou(g, affinity.scale(g, -1, 1, origin=c)) > 0.97 or iou(g, affinity.scale(g, 1, -1, origin=c)) > 0.97:
            self_sym += 1
        for j in range(i + 1, len(sh)):
            h = sh[j]["geo"]
            if abs(g.area - h.area) > 0.03 * max(g.area, h.area):
                continue
            cj = cents[j]
            if math.hypot(c.x - cj.x, c.y - cj.y) < 1e-6 * math.sqrt(g.area):
                continue
            if iou(affinity.translate(g, cj.x - c.x, cj.y - c.y), h) > 0.97:
                if j not in used:
                    repeat += 1
                    used.add(j)
                    save_use += max(0, sh[j]["params"] - 6)
                continue
            if abs(c.y - cj.y) < 0.02 * math.sqrt(g.area) and \
                    iou(affinity.scale(g, -1, 1, origin=((c.x + cj.x) / 2, c.y)), h) > 0.97:
                mirror_pair += 1
    res.update(self_sym=self_sym, mirror_pair=mirror_pair, repeat=repeat,
               use_saving=save_use / total)
    # whole-drawing symmetry, on a render
    try:
        from inkvec_bench import render
        img = render.render(text, 128, 128).astype(np.float64)
        pm = np.concatenate([img[..., :3] * img[..., 3:], img[..., 3:]], axis=-1)
        den = np.abs(pm).sum() or 1.0
        res["sym_v"] = float(np.abs(pm - pm[:, ::-1]).sum() / den)
        res["sym_h"] = float(np.abs(pm - pm[::-1, :]).sum() / den)
        res["sym_r"] = float(np.abs(pm - pm[::-1, ::-1]).sum() / den)
    except Exception:  # noqa: BLE001
        pass
    return res


def summarize_geometry(rows: list[dict]) -> dict:
    out = {}
    fams = [f for f in GATE_FAMILIES if any(r["family"] == f for r in rows)]
    for fam in fams + ["ALL"]:
        rs = [r for r in rows if "n" in r and (fam == "ALL" or r["family"] == fam)]
        if not rs:
            continue
        nel = sum(r["n"] for r in rs) or 1
        b = [x for r in rs for x in r["borrowed"]]
        d = dict(
            icons=len(rs),
            el_partially_covered=sum(r["partial"] for r in rs) / nel,
            el_hidden=sum(r["hidden"] for r in rs) / nel,
            icons_with_partial=np.mean([r["partial"] > 0 for r in rs]),
            params_in_partial=float(np.mean([r["partial_params"] for r in rs])),
            params_in_hidden=float(np.mean([r["hidden_params"] for r in rs])),
            borrowed_boundary_median=float(np.median(b)) if b else float("nan"),
            el_self_symmetric=sum(r["self_sym"] for r in rs) / nel,
            el_mirror_pairs=sum(r["mirror_pair"] for r in rs) / nel,
            el_repeats=sum(r["repeat"] for r in rs) / nel,
            icons_with_repeat=np.mean([r["repeat"] > 0 for r in rs]),
            use_saving=float(np.mean([r["use_saving"] for r in rs])),
        )
        for k in ("sym_v", "sym_h", "sym_r"):
            v = [r[k] for r in rs if k in r]
            if v:
                d[k + "_lt_0.01"] = float(np.mean([x < 0.01 for x in v]))
                d[k + "_lt_0.03"] = float(np.mean([x < 0.03 for x in v]))
        d["any_mirror_lt_0.01"] = float(np.mean([min(r.get("sym_v", 1), r.get("sym_h", 1)) < 0.01 for r in rs]))
        out[fam] = d
    return out


# --------------------------------------------------------------------------- simulations

VIEWBOXES = [16, 20, 24, 32, 36, 48, 64, 72, 96, 100, 128]
SUBDIV = [0, 1, 2, 3]  # unit 1, 1/2, 1/4, 1/8


def wrapped_density(r: np.ndarray, g: float, sigma: float) -> np.ndarray:
    """Density at offset `r` (|r| <= g/2) of a Gaussian wrapped on a circle of length g."""
    k = np.arange(-2, 3)[:, None]
    return np.exp(-0.5 * ((r[None, :] + k * g) / sigma) ** 2).sum(0) / (math.sqrt(2 * math.pi) * sigma)


def infer_grid(x: np.ndarray, n_px: int, sigma: float):
    """Empirical-Bayes design grid. Hypotheses: pitch g = n_px/V·2^-k for V in VIEWBOXES and k
    in SUBDIV, prior uniform over V and halving per subdivision, and 'no grid' at prior ½. Per
    pitch the mixture weight π of on-grid coordinates is fitted (Laplace, ½·ln n for it). The
    likelihood ratio of one coordinate against a uniform background is π·g·φ(r) + 1 − π, with
    φ the wrapped Gaussian at its offset r from the nearest grid point."""
    hyps: dict[float, float] = {}
    for V in VIEWBOXES:
        for k in SUBDIV:
            g = round(n_px / V / 2 ** k, 12)
            hyps[g] = hyps.get(g, 0.0) + (1 / len(VIEWBOXES)) * 2.0 ** -(k + 1) * 0.5 / (1 - 2 ** -len(SUBDIV))
    best = (math.log(0.5), None, 0.0)
    n = len(x)
    pis = np.linspace(0.02, 0.995, 60)
    for g, pr in hyps.items():
        if g < 3 * sigma:
            continue  # the pitch is below the measurement's resolution: no evidence either way
        r = x - g * np.round(x / g)
        dens = g * wrapped_density(r, g, sigma)
        ll = np.log(pis[:, None] * dens[None, :] + (1 - pis[:, None])).sum(1)
        j = int(np.argmax(ll))
        score = math.log(pr) + ll[j] - 0.5 * math.log(max(n, 1))
        if score > best[0]:
            best = (score, g, pis[j])
    return best


def grid_sim_icon(arg) -> list[dict]:
    fam, icon, xs, ys, V, n_pxs, sigmas, seed = arg
    rng = np.random.default_rng(seed)
    vals = [Fraction(v) for v in xs + ys]
    if len(vals) < 4:
        return []
    out = []
    for n_px in n_pxs:
        s = Fraction(n_px) / Fraction(V).limit_denominator(10 ** 6)
        true_px = np.array([float(v * s) for v in vals])
        for sigma in sigmas:
            obs = true_px + rng.normal(0, sigma, len(true_px))
            _, g, pi = infer_grid(obs, n_px, sigma)
            snapped = np.zeros(len(obs), bool)
            final = obs.copy()
            if g is not None:
                r = obs - g * np.round(obs / g)
                d = g * wrapped_density(r, g, sigma)
                resp = pi * d / (pi * d + 1 - pi)
                snapped = resp > 0.5
                final[snapped] = g * np.round(obs[snapped] / g)
            exact = np.abs(final - true_px) < 1e-7
            out.append(dict(family=fam, icon=icon, n_px=n_px, sigma=sigma, n=len(obs),
                            grid=g, exact=int((snapped & exact).sum()),
                            wrong=int((snapped & ~exact).sum()),
                            err_before=float(np.abs(obs - true_px).mean()),
                            err_after=float(np.abs(final - true_px).mean())))
    return out


CONSTRUCTION = {"corner", "end", "prim"}


def grid_hypotheses(n_px: int) -> dict[float, float]:
    """Pitch (px) -> prior mass: uniform over VIEWBOXES, halving per subdivision, total 1/2."""
    hyps: dict[float, float] = {}
    for Vh in VIEWBOXES:
        for k in SUBDIV:
            g = round(n_px / Vh / 2 ** k, 12)
            hyps[g] = hyps.get(g, 0.0) + (1 / len(VIEWBOXES)) * 2.0 ** -(k + 1) * 0.5 / (1 - 2 ** -len(SUBDIV))
    return hyps


def grid_sim_hier_icon(arg) -> list[dict]:
    """The hierarchical model: ties first (exact 1-D clustering per axis under the CRP), then
    the grid on the cluster means (each with noise sigma/sqrt(size)), with the on-grid weight
    fitted separately for construction points (line-line corners, open ends, primitive
    centres and sides) and for derived points (curve corners, tangent points)."""
    fam, icon, coords, V, alpha, n_pxs, sigmas, seed = arg
    rng = np.random.default_rng(seed)
    if len(coords) < 4:
        return []
    out = []
    pis = np.linspace(0.02, 0.995, 60)
    kk = np.arange(-2, 3)[:, None]
    for n_px in n_pxs:
        s = Fraction(n_px) / Fraction(V).limit_denominator(10 ** 6)
        for sigma in sigmas:
            clusters = []  # (mean, sd, construction?, member indices)
            true_all, obs_all = [], []
            for axis in ("x", "y"):
                idx = [i for i, c in enumerate(coords) if c[0] == axis]
                if not idx:
                    continue
                true = np.array([float(Fraction(coords[i][2]) * s) for i in idx])
                obs = true + rng.normal(0, sigma, len(true))
                lab = ties_dp(obs, sigma, alpha, float(n_px)) if len(idx) <= 1500 else np.arange(len(idx))
                base = len(true_all)
                true_all += true.tolist()
                obs_all += obs.tolist()
                for c in np.unique(lab):
                    mem = np.where(lab == c)[0]
                    con = any(coords[idx[m]][1] in CONSTRUCTION for m in mem)
                    clusters.append((float(obs[mem].mean()), sigma / math.sqrt(len(mem)), con,
                                     [base + int(m) for m in mem]))
            true_all, obs_all = np.array(true_all), np.array(obs_all)
            mu = np.array([c[0] for c in clusters])
            sd = np.array([c[1] for c in clusters])
            con = np.array([c[2] for c in clusters])
            best = (math.log(0.5), None, 0.0, 0.0)
            for g, pr in grid_hypotheses(n_px).items():
                if g < 3 * sigma:
                    continue
                r = mu - g * np.round(mu / g)
                dens = g * np.exp(-0.5 * ((r[None, :] + kk * g) / sd[None, :]) ** 2).sum(0) / (math.sqrt(2 * math.pi) * sd)
                score = math.log(pr)
                pick = []
                for mask in (con, ~con):
                    if mask.sum() == 0:
                        pick.append(0.0)
                        continue
                    ll = np.log(pis[:, None] * dens[mask][None, :] + (1 - pis[:, None])).sum(1)
                    j = int(np.argmax(ll))
                    score += ll[j] - 0.5 * math.log(mask.sum())
                    pick.append(pis[j])
                if score > best[0]:
                    best = (score, g, pick[0], pick[1])
            _, g, pc, pd = best
            final = np.zeros(len(true_all))
            snapped = np.zeros(len(true_all), bool)
            for (m, sdc, cflag, mem) in clusters:
                val = m
                if g is not None:
                    pi = pc if cflag else pd
                    r = m - g * round(m / g)
                    d = g * sum(math.exp(-0.5 * ((r + k * g) / sdc) ** 2) for k in range(-2, 3)) / (math.sqrt(2 * math.pi) * sdc)
                    if pi * d / (pi * d + 1 - pi) > 0.5:
                        val = g * round(m / g)
                        snapped[mem] = True
                final[mem] = val
            exact = np.abs(final - true_all) < 1e-7
            out.append(dict(family=fam, icon=icon, n_px=n_px, sigma=sigma, n=len(true_all),
                            grid=g, exact=int((snapped & exact).sum()),
                            wrong=int((snapped & ~exact).sum()),
                            err_before=float(np.abs(obs_all - true_all).mean()),
                            err_after=float(np.abs(final - true_all).mean())))
    return out


def ties_dp(x: np.ndarray, sigma: float, alpha: float, W: float) -> np.ndarray:
    """MAP partition of the values into equal-value clusters under a Chinese-restaurant prior
    (concentration alpha), a uniform base measure on [0, W] and Gaussian noise of known sigma.
    Equal noise makes the optimal clusters contiguous in sorted order, so this O(n²) dynamic
    program is exact. Returns a cluster label per input value (input order)."""
    order = np.argsort(x)
    v = x[order]
    n = len(v)
    s1 = np.concatenate([[0.0], np.cumsum(v)])
    s2 = np.concatenate([[0.0], np.cumsum(v * v)])
    best = np.full(n + 1, np.inf)
    best[0] = 0.0
    arg = np.zeros(n + 1, int)
    lg = np.array([math.lgamma(m) if m > 0 else 0.0 for m in range(n + 1)])
    l2ps = math.log(2 * math.pi * sigma * sigma)
    for b in range(1, n + 1):
        a = np.arange(0, b)
        m = b - a
        sse = (s2[b] - s2[a]) - (s1[b] - s1[a]) ** 2 / m
        cost = (-math.log(alpha) - lg[m] + math.log(W) + 0.5 * (m - 1) * l2ps
                + np.maximum(sse, 0) / (2 * sigma * sigma) + 0.5 * np.log(m))
        tot = best[a] + cost
        j = int(np.argmin(tot))
        best[b], arg[b] = tot[j], a[j]
    labels = np.zeros(n, int)
    b, lab = n, 0
    while b > 0:
        a = arg[b]
        labels[order[a:b]] = lab
        lab += 1
        b = a
    return labels


def ties_sim_icon_exact(arg) -> list[dict]:
    """Pairwise precision/recall of recovered equalities, and error before/after."""
    fam, icon, xs, ys, V, alpha, n_pxs, sigmas, seed = arg
    rng = np.random.default_rng(seed)
    out = []
    for n_px in n_pxs:
        s = n_px / V
        for sigma in sigmas:
            row = dict(family=fam, icon=icon, n_px=n_px, sigma=sigma, tp=0, fp=0, fn=0,
                       n=0, err_before=0.0, err_after=0.0)
            for coords in (xs, ys):
                if len(coords) < 2 or len(coords) > 1500:
                    continue
                vals = [Fraction(v) for v in coords]
                true = np.array([float(v) * s for v in vals])
                obs = true + rng.normal(0, sigma, len(true))
                lab = ties_dp(obs, sigma, alpha, float(n_px))
                est = obs.copy()
                for c in np.unique(lab):
                    est[lab == c] = obs[lab == c].mean()
                t_lab = {}
                tl = np.array([t_lab.setdefault(str(v), len(t_lab)) for v in vals])
                # pairs predicted equal, truly equal, both
                joint = Counter(zip(lab.tolist(), tl.tolist()))
                pred = Counter(lab.tolist())
                tru = Counter(tl.tolist())
                both = sum(k * (k - 1) // 2 for k in joint.values())
                p_pairs = sum(k * (k - 1) // 2 for k in pred.values())
                t_pairs = sum(k * (k - 1) // 2 for k in tru.values())
                row["tp"] += both
                row["fp"] += p_pairs - both
                row["fn"] += t_pairs - both
                row["n"] += len(true)
                row["err_before"] += float(np.abs(obs - true).sum())
                row["err_after"] += float(np.abs(est - true).sum())
            out.append(row)
    return out


# --------------------------------------------------------------------------- main


def report_grid(sims, title, set_name) -> dict:
    agg = {}
    print(f"\n== grid inference and snapping, {title} ({set_name})")
    for fam in [f for f in GATE_FAMILIES if any(s["family"] == f for s in sims)] + ["ALL"]:
        for n_px in (128, 512):
            for sigma in (0.005, 0.02, 0.05, 0.1):
                ss = [s for s in sims if (fam == "ALL" or s["family"] == fam)
                      and s["n_px"] == n_px and s["sigma"] == sigma]
                n = sum(s["n"] for s in ss) or 1
                d = dict(exact=sum(s["exact"] for s in ss) / n, wrong=sum(s["wrong"] for s in ss) / n,
                         grid_found=float(np.mean([s["grid"] is not None for s in ss])),
                         err_ratio=sum(s["err_after"] for s in ss) / max(1e-12, sum(s["err_before"] for s in ss)))
                agg[f"{fam}/{n_px}/{sigma}"] = d
                print(f"[{fam}] {n_px}px σ={sigma}: grid found {fmt(d['grid_found'], 2)}, "
                      f"exact {fmt(d['exact'])}, wrong {fmt(d['wrong'], 4)}, "
                      f"error after/before {fmt(d['err_ratio'])}")
    return agg


def fmt(x, nd=3):
    if isinstance(x, float):
        return "nan" if math.isnan(x) else f"{x:.{nd}f}"
    return str(x)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--set", default="corpus", help="corpus (every gate-family file), screen, all, ...")
    ap.add_argument("--part", default="counts", choices=["counts", "geometry", "grid_sim", "ties_sim", "all"])
    ap.add_argument("--workers", type=int, default=3)
    ap.add_argument("--json", default="")
    a = ap.parse_args()
    icons = icons_for(a.set)
    report: dict = {"set": a.set, "icons": len(icons)}
    parts = ["counts", "geometry", "grid_sim", "ties_sim"] if a.part == "all" else [a.part]

    with ProcessPoolExecutor(a.workers) as pool:
        rows = [r for r in pool.map(_count_one, icons, chunksize=8) if "counts" in r]
        cs = summarize_counts(rows)
        report["counts"] = cs
        if "counts" in parts:
            print(f"== counts over {len(rows)} artist files ({a.set})")
            for fam, d in cs.items():
                print(f"\n[{fam}] icons {d['icons']}, gate params/icon {fmt(d['gate_per_icon'], 1)}, "
                      f"typed/gate {fmt(d['typed_over_gate'], 2)}")
                print("  elements", d["el_kinds"], "rounded rects", fmt(d["rect_rounded"], 2),
                      "stroked icons", fmt(d["stroked_icons"], 2))
                print("  segments", {k: fmt(v, 3) for k, v in d["seg_share"].items()},
                      "S", d["cmd_S"], "T", d["cmd_T"])
                print(f"  lines {d['lines']}: axis {fmt(d['line_axis'])} (H/V {fmt(d['hv_of_axis_lines'], 2)}), "
                      f"45° {fmt(d['line_45'])}, near-axis {fmt(d['line_near_axis'])}, near-45 "
                      f"{fmt(d['line_near_45'])}, other {fmt(d['line_other'])}; dots {d['line_dots']}")
                print(f"  cubics {d['cubics']}: arc-like {fmt(d['cubic_arc'])}, straight {fmt(d['cubic_straight'])}; "
                      f"arcs {d['arcs']}: circular {fmt(d['arc_circular'])}, ≤90° {fmt(d['arc_le90'])}, "
                      f"≤120° {fmt(d['arc_le120'])}, ≤180° {fmt(d['arc_le180'])}")
                print("  joins", {k: {kk: fmt(vv) for kk, vv in v.items()} for k, v in d["joins"].items()})
                print("  on-curve numbers on grid", {k: fmt(v) for k, v in d["end_on_grid"].items()},
                      "decimals≤", {k: fmt(v) for k, v in d["end_decimals"].items()})
                print("  control numbers on grid", {k: fmt(v) for k, v in d["ctrl_on_grid"].items()},
                      "decimals≤", {k: fmt(v) for k, v in d["ctrl_decimals"].items()})
                print("  point classes", {k: {kk: fmt(vv) for kk, vv in v.items()} for k, v in d["point_classes"].items()})
                print("  turn at curve joins (deg bins)", {k: fmt(v) for k, v in d["turn_curve"].items()})
                print("  turn at line-line joins", {k: fmt(v) for k, v in d["turn_line"].items()})
                print("  icon grid", {k: fmt(v) for k, v in d["icon_grid"].items()},
                      "π_on", fmt(d["pi_on_grid"]))
                print(f"  radii {d['radii']}: repeated {fmt(d['radius_repeat'])}; widths per stroked icon "
                      f"{fmt(d['widths_per_stroked_icon'], 2)}")
                print(f"  reuse x {fmt(d['x_reuse'])} y {fmt(d['y_reuse'])}; CRP α x {fmt(d['crp_alpha_x'], 2)} "
                      f"y {fmt(d['crp_alpha_y'], 2)}; nats/coord prior {fmt(d['prior_nats_per_coord'], 2)} "
                      f"vs flat {fmt(d['flat_nats_per_coord'], 2)}")

        if "geometry" in parts:
            g_rows = list(pool.map(geometry_icon, icons, chunksize=4))
            gs = summarize_geometry([r for r in g_rows if "n" in r])
            report["geometry"] = gs
            print(f"\n== geometry ({a.set})")
            for fam, d in gs.items():
                print(f"[{fam}] " + ", ".join(f"{k} {fmt(v)}" for k, v in d.items()))

        if "grid_sim" in parts:
            args = [(r["family"], r["icon"], r["xs"], r["ys"], r["V"], [128, 512], [0.005, 0.02, 0.05, 0.1],
                     i) for i, r in enumerate(rows)]
            sims = [x for lst in pool.map(grid_sim_icon, args, chunksize=4) for x in lst]
            alphas = {f: (d["crp_alpha_x"] + d["crp_alpha_y"]) / 2 for f, d in cs.items()}
            hargs = [(r["family"], r["icon"], r["coords"], r["V"], alphas[r["family"]], [128, 512],
                      [0.005, 0.02, 0.05, 0.1], i) for i, r in enumerate(rows)]
            hsims = [x for lst in pool.map(grid_sim_hier_icon, hargs, chunksize=4) for x in lst]
            report["grid_sim_hier"] = report_grid(hsims, "hierarchical: ties, then grid by role", a.set)
            agg = {}
            print(f"\n== grid inference and snapping, per coordinate, one weight ({a.set})")
            for fam in [f for f in GATE_FAMILIES if any(s["family"] == f for s in sims)] + ["ALL"]:
                for n_px in (128, 512):
                    for sigma in (0.005, 0.02, 0.05, 0.1):
                        ss = [s for s in sims if (fam == "ALL" or s["family"] == fam)
                              and s["n_px"] == n_px and s["sigma"] == sigma]
                        n = sum(s["n"] for s in ss) or 1
                        d = dict(exact=sum(s["exact"] for s in ss) / n, wrong=sum(s["wrong"] for s in ss) / n,
                                 grid_found=float(np.mean([s["grid"] is not None for s in ss])),
                                 err_ratio=sum(s["err_after"] for s in ss) / max(1e-12, sum(s["err_before"] for s in ss)))
                        agg[f"{fam}/{n_px}/{sigma}"] = d
                        print(f"[{fam}] {n_px}px σ={sigma}: grid found {fmt(d['grid_found'], 2)}, "
                              f"exact {fmt(d['exact'])}, wrong {fmt(d['wrong'], 4)}, "
                              f"error after/before {fmt(d['err_ratio'])}")
            report["grid_sim"] = agg

        if "ties_sim" in parts:
            alphas = {f: (d["crp_alpha_x"] + d["crp_alpha_y"]) / 2 for f, d in cs.items()}
            args = [(r["family"], r["icon"], r["xs"], r["ys"], r["V"], alphas[r["family"]], [128, 512],
                     [0.005, 0.02, 0.05, 0.1], 7 + i) for i, r in enumerate(rows)]
            sims = [x for lst in pool.map(ties_sim_icon_exact, args, chunksize=4) for x in lst]
            agg = {}
            print(f"\n== coordinate ties by exact 1-D clustering under a CRP prior ({a.set})")
            for fam in [f for f in GATE_FAMILIES if any(s["family"] == f for s in sims)] + ["ALL"]:
                for n_px in (128, 512):
                    for sigma in (0.005, 0.02, 0.05, 0.1):
                        ss = [s for s in sims if (fam == "ALL" or s["family"] == fam)
                              and s["n_px"] == n_px and s["sigma"] == sigma]
                        tp, fp, fn = (sum(s[k] for s in ss) for k in ("tp", "fp", "fn"))
                        d = dict(precision=tp / max(1, tp + fp), recall=tp / max(1, tp + fn),
                                 err_ratio=sum(s["err_after"] for s in ss) / max(1e-12, sum(s["err_before"] for s in ss)))
                        agg[f"{fam}/{n_px}/{sigma}"] = d
                        print(f"[{fam}] {n_px}px σ={sigma}: pairs precision {fmt(d['precision'])}, "
                              f"recall {fmt(d['recall'])}, error after/before {fmt(d['err_ratio'])}")
            report["ties_sim"] = agg

    if a.json:
        Path(a.json).parent.mkdir(parents=True, exist_ok=True)
        Path(a.json).write_text(json.dumps(report, indent=1, default=str), encoding="utf-8")


if __name__ == "__main__":
    main()
