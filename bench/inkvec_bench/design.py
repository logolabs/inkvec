"""How a file is drawn: a battery of statistics of an SVG's construction, each read on the
trace and on the artist's own file, and the distance between the two.

The gate's axes ask whether a trace looks like the artist's drawing (dE00, geom) and whether
it is about as long and turns about as much (ratio_gap, turning_gap). None asks whether it is
*built* like it: lines along the axes, nodes at the extrema with level handles, smooth joins
where the artist drew a curve and clean corners where they drew a corner, numbers on the
design grid, values shared, circles written as circles, shapes stacked rather than cut out.
These are what a designer sees on opening the file, and what `--editability` aims at. Each
statistic below is a number per file; its **divergence** is how far the trace's number is
from the artist's, per icon, so that zero is "drawn as the artist drew it" whatever the
family's style. They are reported, not gated.

Reading a file
--------------
One pass over the XML (`parse`): `<path>` (every command, relative ones resolved, `S`/`T`
reflected, arcs in endpoint form), and `<rect>`, `<circle>`, `<ellipse>`, `<line>`,
`<polyline>`, `<polygon>` and `<use>` as the outlines they stand for (a rectangle four lines,
rounded corners quarter arcs, a circle or an ellipse four quarter arcs starting at its
rightmost point, as an editor shows them). Transforms are applied; paint (fill, stroke,
stroke-width, fill-rule, colour) is inherited down the tree, from attributes and `style`.
Coordinates are mapped to the canvas the gate renders (`render.fit_viewbox`: the viewBox
centred in a square of its longer side), in units of that side, so a 24-unit icon and its
512 px trace read on one scale. A **node** is an on-curve point (each segment's start, and
an open subpath's last end); a **join** is a node between two segments of one subpath,
including where a closed subpath closes; its **turn** is the angle between the incoming and
the outgoing tangent. A segment shorter than `DOT` (1/1000 of the canvas: lucide's `h.01`
round-cap dots) is a dot, not a segment.

The statistics (`STATS`; divergence in brackets)
------------------------------------------------
Pieces
* `kinds`      segment-kind mix: shares of lines, cubics, quadratics, arcs [total variation
               distance between the two mixes, 0-1]
* `primitives` share of drawn elements written as `rect`, `circle`, `ellipse` or `line`
               [|difference|]
* `stroked`    share of drawn elements with a visible stroke [|difference|]
* `stroke_width_ties` of the stroked elements (two at least), the share whose stroke width
               another repeats exactly: one pen for the drawing [|difference|]
* `elements`   drawn elements [|ln ratio|]
* `subpaths`   subpaths [|ln ratio|]
* `nodes_per_subpath` mean nodes per subpath [|ln ratio|]
* `closed`     share of subpaths that are closed [|difference|]
Directions
* `line_axis`  share of lines within `AXIS_DEG` of horizontal or vertical [|difference|]
* `line_diag`  share of lines within `AXIS_DEG` of a 45 degree diagonal [|difference|]
* `handle_axis` share of cubic handles (node to its control point) within `AXIS_DEG` of an
               axis [|difference|]
Joins
* `smooth`     of joins with a curve on at least one side, the share turning at most
               `SMOOTH_DEG` (G1) [|difference|]
* `extremum`   of those smooth joins, the share whose tangent is within `EXTREMUM_DEG` of an
               axis: a node at an x or y extremum [|difference|]
* `kinked`     of joins between two curves, the share turning more than `SMOOTH_DEG` and at
               most `CORNER_DEG`: neither smooth nor a corner [|difference|]
* `collinear`  of joins between two lines, the share turning at most `AXIS_DEG`: a node in
               the middle of a straight run [|difference|]
* `corner_share` share of all joins turning more than `CORNER_DEG` [|difference|]
* `right_angle` of those corners, the share within `RIGHT_DEG` of 90 degrees [|difference|]
* `corner_angles` the corners' turn angles as a distribution [Wasserstein-1, degrees / 180]
Curves
* `sweep`      each curve's own turning (an arc's angle; a cubic's or quadratic's
               control-polygon turning) as a distribution [Wasserstein-1, degrees / 180]
* `wide_curves` share of curves turning more than 100 degrees (an extremum skipped)
               [|difference|]
* `handle_length` each cubic handle's length over its segment's chord, as a distribution
               (a quarter circle's is 0.55) [Wasserstein-1]
* `symmetric_handles` of smooth joins between two cubics, the share whose two handles are
               within 5 % of one length [|difference|]
* `segment_lengths` segment lengths, log10 of the canvas fraction, as a distribution
               [Wasserstein-1, decades]
* `length_gini` the Gini coefficient of segment lengths [|difference|]
Numbers
* `grid_half`  share of node coordinates (x and y) on the artist's half-unit grid, in the
               artist's own units (the trace's mapped there), within `GRID_TOL_PX` raster px
               for a trace and exactly for the artist [|difference|]
* `grid_int`   the same on the whole-unit grid [|difference|]
* `grid_level` the coarsest of the grids 1, 1/2, 1/4, 1/8, 1/16 unit that holds 75 % of the
               node coordinates, as 0-4, 5 for none [|difference|]
* `decimals`   mean decimals a node coordinate needs in the artist's units (0-3, 4 for more)
               at the same tolerance [|difference|]
* `ties`       share of node coordinates (x among x, y among y) another node repeats
               exactly as written (1e-6 of the canvas) [|difference|]
* `mirror`     share of nodes whose mirror image across the drawing's vertical (or, if
               more, horizontal) centre line is within `MIRROR_TOL` of another node
               [|difference|]
Paint and layers
* `colours`    distinct paint colours (flat fills and strokes, gradient stops) [|ln ratio|]
* `palette`    the two palettes' symmetric mean nearest-colour distance, CIELAB delta E 1976
               [the distance itself; the artist's own file reads 0]
* `nested`     share of closed subpaths that lie inside another subpath of the same element
               (holes and islands of a compound path) [|difference|]
* `stacked`    share of filled elements painted over an earlier filled element (an interior
               point of theirs lies inside it) [|difference|]
* `evenodd`    share of filled elements with `fill-rule="evenodd"` [|difference|]
Artefact signal (reported, left out of `bench/human_stats.py`'s composite)
* `halfpx`     share of node coordinates on the raster's half-pixel grid (pixel edges and
               centres) within `GRID_TOL_PX`: numbers snapped to the pixels the file was
               traced from, which no artist's grid explains [|difference|]

A share whose denominator is empty in either file (no lines, no corners) has no divergence for
that icon. `KEPT` is the non-redundant subset the battery reports: `bench/human_stats.py
--correlate` computes the Spearman correlation of the per-icon divergences and drops a
statistic correlated beyond |rho| 0.7 with one kept before it (see `KEPT` for the result).

Not from the literature as a battery. The node conventions (nodes at extrema, level handles,
smooth or corner but not in between) are the type-design and icon-design canon (e.g. the
Apple and Material icon grids; Adobe's "Illustrator: nodes at extrema" practice); the
divergences are standard: total variation for a categorical mix, Wasserstein-1 for a
distribution of reals (Villani 2009, Optimal Transport, ch. 6).
"""
from __future__ import annotations

import math
import re
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from functools import lru_cache
from itertools import pairwise

import numpy as np

#: Bumped whenever a statistic's definition changes, so cached artist profiles
#: (`svgeval.artist_design`) are never read for a different battery.
VERSION = 1

#: Tolerances, in degrees.
AXIS_DEG = 0.5
SMOOTH_DEG = 3.0
CORNER_DEG = 30.0
EXTREMUM_DEG = 1.0
RIGHT_DEG = 1.0
WIDE_DEG = 100.0
#: A segment shorter than this (canvas fraction) is a dot, not a segment.
DOT = 1e-3
#: A trace's node coordinate is on the artist's grid within this many raster px (the tracer
#: writes two decimals).
GRID_TOL_PX = 0.02
#: An artist's own coordinate is on its grid within this many of its units (exact numbers).
EXACT_TOL = 1e-6
#: Two node coordinates are one value within this canvas fraction (as written).
TIE_TOL = 1e-6
#: A node's mirror image matches another node within this canvas fraction (2 px at 512 px).
MIRROR_TOL = 1 / 256
#: Grids tried for `grid_level`, as subdivisions of the artist's unit.
GRID_LADDER = (1, 2, 4, 8, 16)
GRID_LEVEL_SHARE = 0.75

NONPAINT = {"defs", "clipPath", "mask", "symbol", "linearGradient", "radialGradient",
            "filter", "pattern", "marker", "title", "desc", "metadata", "style", "script",
            "text", "image", "foreignObject"}
INHERITED = ("fill", "stroke", "stroke-width", "fill-rule", "fill-opacity", "stroke-opacity",
             "color", "visibility")
PRIMITIVE_TAGS = ("rect", "circle", "ellipse", "line")
NUM = re.compile(r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")
TOKEN = re.compile(r"[MmLlHhVvCcSsQqTtAaZz]|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")
ARGS = {"M": 2, "L": 2, "T": 2, "H": 1, "V": 1, "C": 6, "S": 4, "Q": 4, "A": 7, "Z": 0}
TRANSFORM = re.compile(r"(matrix|translate|scale|rotate|skewX|skewY)\s*\(([^)]*)\)")


# --------------------------------------------------------------------------- geometry
@dataclass
class Seg:
    """One drawn segment, in canvas units. `kind` is L, C, Q or A; `pts` are its points in
    order (start, controls, end); an arc carries its centre form in `arc`."""

    kind: str
    pts: list
    arc: tuple | None = None          # (cx, cy, rx, ry, phi, theta1, dtheta)

    @property
    def p0(self):
        return self.pts[0]

    @property
    def p1(self):
        return self.pts[-1]


@dataclass
class Sub:
    segs: list[Seg]
    closed: bool


@dataclass
class Elem:
    tag: str
    subs: list[Sub]
    filled: bool
    stroked: bool
    evenodd: bool
    colours: list = field(default_factory=list)
    stroke_width: float = 0.0     # canvas units, when stroked


def _affine_mul(m, n):
    a, b, c, d, e, f = m
    a2, b2, c2, d2, e2, f2 = n
    return (a * a2 + c * b2, b * a2 + d * b2, a * c2 + c * d2, b * c2 + d * d2,
            a * e2 + c * f2 + e, b * e2 + d * f2 + f)


IDENTITY = (1.0, 0.0, 0.0, 1.0, 0.0, 0.0)


def _parse_transform(text: str | None):
    m = IDENTITY
    if not text:
        return m
    for name, args in TRANSFORM.findall(text):
        v = [float(x) for x in NUM.findall(args)]
        if name == "matrix" and len(v) == 6:
            t = tuple(v)
        elif name == "translate" and v:
            t = (1.0, 0.0, 0.0, 1.0, v[0], v[1] if len(v) > 1 else 0.0)
        elif name == "scale" and v:
            t = (v[0], 0.0, 0.0, v[1] if len(v) > 1 else v[0], 0.0, 0.0)
        elif name == "rotate" and v:
            r = math.radians(v[0])
            cs, sn = math.cos(r), math.sin(r)
            t = (cs, sn, -sn, cs, 0.0, 0.0)
            if len(v) == 3:
                t = _affine_mul(_affine_mul((1.0, 0.0, 0.0, 1.0, v[1], v[2]), t),
                                (1.0, 0.0, 0.0, 1.0, -v[1], -v[2]))
        elif name == "skewX" and v:
            t = (1.0, 0.0, math.tan(math.radians(v[0])), 1.0, 0.0, 0.0)
        elif name == "skewY" and v:
            t = (1.0, math.tan(math.radians(v[0])), 0.0, 1.0, 0.0, 0.0)
        else:
            continue
        m = _affine_mul(m, t)
    return m


def _arc_centre(p0, rx, ry, phi, fa, fs, p1):
    """SVG arc endpoint form to centre form (SVG 1.1 F.6.5): (cx, cy, rx, ry, phi, th1, dth),
    or None for a degenerate arc (drawn as a line)."""
    x1, y1 = p0
    x2, y2 = p1
    rx, ry = abs(rx), abs(ry)
    if rx < 1e-12 or ry < 1e-12 or (abs(x1 - x2) < 1e-12 and abs(y1 - y2) < 1e-12):
        return None
    c, s = math.cos(phi), math.sin(phi)
    dx2, dy2 = (x1 - x2) / 2, (y1 - y2) / 2
    x1p, y1p = c * dx2 + s * dy2, -s * dx2 + c * dy2
    lam = x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry)
    if lam > 1:
        k = math.sqrt(lam)
        rx, ry = rx * k, ry * k
    num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p
    den = rx * rx * y1p * y1p + ry * ry * x1p * x1p
    co = math.sqrt(max(0.0, num / den)) if den > 0 else 0.0
    if fa == fs:
        co = -co
    cxp, cyp = co * rx * y1p / ry, -co * ry * x1p / rx
    cx = c * cxp - s * cyp + (x1 + x2) / 2
    cy = s * cxp + c * cyp + (y1 + y2) / 2

    def ang(ux, uy, vx, vy):
        return math.atan2(ux * vy - uy * vx, ux * vx + uy * vy)

    th1 = ang(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry)
    dth = ang((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry)
    if not fs and dth > 0:
        dth -= 2 * math.pi
    elif fs and dth < 0:
        dth += 2 * math.pi
    return (cx, cy, rx, ry, phi, th1, dth)


def _arc_point(a, th):
    cx, cy, rx, ry, phi, _, _ = a
    c, s = math.cos(phi), math.sin(phi)
    x, y = rx * math.cos(th), ry * math.sin(th)
    return (cx + c * x - s * y, cy + s * x + c * y)


def _arc_tangent(a, th):
    _, _, rx, ry, phi, _, dth = a
    c, s = math.cos(phi), math.sin(phi)
    dx, dy = -rx * math.sin(th), ry * math.cos(th)
    sg = 1.0 if dth >= 0 else -1.0
    return (sg * (c * dx - s * dy), sg * (s * dx + c * dy))


def _scan(d: str):
    """Path data as (command, [numbers]); arc flags read one character each, so the compact
    `a2 2 0 012-2` reads as SVG says it does."""
    toks = TOKEN.findall(d)
    out, i, n, cmd = [], 0, len(toks), None
    while i < n:
        t = toks[i]
        if t.isalpha():
            cmd = t
            i += 1
            if cmd in "Zz":
                out.append((cmd, []))
                continue
        elif cmd is None or cmd in "Zz":
            return out
        k = ARGS[cmd.upper()]
        args: list[float] = []
        while len(args) < k:
            if i >= n or toks[i].isalpha():
                return out
            t = toks[i]
            if cmd in "Aa" and len(args) in (3, 4) and len(t) > 1 and t[0] in "01":
                args.append(float(t[0]))
                toks[i] = t[1:]
                continue
            args.append(float(t))
            i += 1
        out.append((cmd, args))
        if cmd == "M":
            cmd = "L"
        elif cmd == "m":
            cmd = "l"
    return out


class _Builder:
    """Collects one element's subpaths in canvas units through a fixed transform."""

    def __init__(self, ctm):
        self.ctm = ctm
        a, b, c, d, _, _ = ctm
        self.scale = math.sqrt(abs(a * d - b * c))
        self.rot = math.atan2(b, a)
        self.flip = a * d - b * c < 0
        self.subs: list[Sub] = []
        self.cur: list[Seg] | None = None

    def tp(self, p):
        a, b, c, d, e, f = self.ctm
        return (a * p[0] + c * p[1] + e, b * p[0] + d * p[1] + f)

    def start(self):
        self.end_sub(False)
        self.cur = []

    def end_sub(self, closed: bool):
        if self.cur:
            self.subs.append(Sub(self.cur, closed))
        self.cur = None

    def add(self, kind, pts, arc=None):
        if self.cur is None:
            self.cur = []
        self.cur.append(Seg(kind, [self.tp(p) for p in pts], arc))

    def add_arc(self, p0, rx, ry, phi_deg, fa, fs, p1):
        q0, q1 = self.tp(p0), self.tp(p1)
        a = _arc_centre(q0, rx * self.scale, ry * self.scale,
                        math.radians(phi_deg) + self.rot, fa, (not fs) if self.flip else fs, q1)
        if self.cur is None:
            self.cur = []
        self.cur.append(Seg("L", [q0, q1]) if a is None else Seg("A", [q0, q1], a))


def _path(b: _Builder, d: str):
    cur = start = (0.0, 0.0)
    last = None   # (kind, control) for S/T reflection
    for cmd, a in _scan(d):
        U, rel = cmd.upper(), cmd.islower()
        ox, oy = cur if rel else (0.0, 0.0)
        if U == "M":
            cur = start = (ox + a[0], oy + a[1])
            b.start()
            last = None
            continue
        if U == "Z":
            if b.cur is not None:
                if cur != start:
                    b.add("L", [cur, start])
                b.end_sub(True)
            cur, last = start, None
            continue
        if b.cur is None:
            b.cur = []
            start = cur
        if U == "L":
            p = (ox + a[0], oy + a[1])
            b.add("L", [cur, p])
            cur, last = p, None
        elif U == "H":
            p = (ox + a[0] if rel else a[0], cur[1])
            b.add("L", [cur, p])
            cur, last = p, None
        elif U == "V":
            p = (cur[0], oy + a[0] if rel else a[0])
            b.add("L", [cur, p])
            cur, last = p, None
        elif U in "CS":
            if U == "C":
                c1, c2, p = (ox + a[0], oy + a[1]), (ox + a[2], oy + a[3]), (ox + a[4], oy + a[5])
            else:
                c1 = (2 * cur[0] - last[1][0], 2 * cur[1] - last[1][1]) if last and last[0] == "C" else cur
                c2, p = (ox + a[0], oy + a[1]), (ox + a[2], oy + a[3])
            b.add("C", [cur, c1, c2, p])
            cur, last = p, ("C", c2)
        elif U in "QT":
            if U == "Q":
                c, p = (ox + a[0], oy + a[1]), (ox + a[2], oy + a[3])
            else:
                c = (2 * cur[0] - last[1][0], 2 * cur[1] - last[1][1]) if last and last[0] == "Q" else cur
                p = (ox + a[0], oy + a[1])
            b.add("Q", [cur, c, p])
            cur, last = p, ("Q", c)
        elif U == "A":
            p = (ox + a[5], oy + a[6])
            b.add_arc(cur, a[0], a[1], a[2], int(a[3]), int(a[4]), p)
            cur, last = p, None
    b.end_sub(False)


def _num(el, name, default=0.0) -> float:
    v = el.get(name)
    if v is None:
        return default
    m = NUM.match(v.strip())
    return float(m.group(0)) if m else default


def _ellipse(b: _Builder, cx, cy, rx, ry):
    if rx <= 0 or ry <= 0:
        return
    pts = [(cx + rx, cy), (cx, cy + ry), (cx - rx, cy), (cx, cy - ry), (cx + rx, cy)]
    b.start()
    for p, q in pairwise(pts):
        b.add_arc(p, rx, ry, 0.0, 0, 1, q)
    b.end_sub(True)


def _rect(b: _Builder, x, y, w, h, rx, ry):
    if w <= 0 or h <= 0:
        return
    rx, ry = min(rx, w / 2), min(ry, h / 2)
    b.start()
    if rx <= 0 or ry <= 0:
        pts = [(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)]
        for p, q in pairwise(pts):
            b.add("L", [p, q])
    else:
        corners = [((x + w - rx, y), (x + w, y + ry)), ((x + w, y + h - ry), (x + w - rx, y + h)),
                   ((x + rx, y + h), (x, y + h - ry)), ((x, y + ry), (x + rx, y))]
        prev = (x + rx, y)
        for p, q in corners:
            if abs(p[0] - prev[0]) + abs(p[1] - prev[1]) > 1e-12:
                b.add("L", [prev, p])
            b.add_arc(p, rx, ry, 0.0, 0, 1, q)
            prev = q
    b.end_sub(True)


# --------------------------------------------------------------------------- paint
@lru_cache(maxsize=4096)
def _rgb(text: str):
    """An sRGB triple in [0, 1] for a CSS colour, or None for none / a reference."""
    t = text.strip().lower()
    if not t or t in ("none", "transparent") or t.startswith("url("):
        return None
    if t.startswith("#"):
        h = t[1:]
        if len(h) in (3, 4):
            h = "".join(ch * 2 for ch in h[:3])
        if len(h) >= 6:
            try:
                return tuple(int(h[i:i + 2], 16) / 255 for i in (0, 2, 4))
            except ValueError:
                return None
        return None
    if t.startswith("rgb"):
        v = NUM.findall(t)
        if len(v) >= 3:
            scale = 100.0 if "%" in t else 255.0
            return tuple(min(1.0, max(0.0, float(x) / scale)) for x in v[:3])
        return None
    try:
        from svgelements import Color
        c = Color(t)
        if c.value is None:
            return None
        return (c.red / 255, c.green / 255, c.blue / 255)
    except Exception:  # noqa: BLE001 - an unknown name is no colour
        return None


def _style(el) -> dict:
    st = {}
    for k in INHERITED + ("opacity", "display"):
        v = el.get(k)
        if v is not None:
            st[k] = v.strip()
    for part in (el.get("style") or "").split(";"):
        if ":" in part:
            k, v = part.split(":", 1)
            st[k.strip()] = v.strip()
    return st


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


@dataclass
class Doc:
    """A parsed file: its drawn elements in paint order, in canvas units."""

    elements: list[Elem]
    frame: tuple[float, float, float]     # origin x, origin y, side, in the file's units


def frame_of(root) -> tuple[float, float, float]:
    """The square the gate renders, in the file's own units: the viewBox centred in a square
    of its longer side (`render.fit_viewbox`), else the root's width and height."""
    vb = root.get("viewBox")
    x = y = 0.0
    w = h = None
    if vb:
        v = [float(t) for t in NUM.findall(vb)]
        if len(v) == 4 and v[2] > 0 and v[3] > 0:
            x, y, w, h = v
    if w is None:
        w, h = _num(root, "width", 0.0), _num(root, "height", 0.0)
        if w <= 0 or h <= 0:
            w = h = 24.0
    s = max(w, h)
    return (x - (s - w) / 2, y - (s - h) / 2, s)


def parse(svg: str) -> Doc:
    """Every drawn element of `svg` (module documentation), in paint order."""
    root = ET.fromstring(svg)
    fx, fy, fs = frame_of(root)
    base = (1 / fs, 0.0, 0.0, 1 / fs, -fx / fs, -fy / fs)    # file units -> canvas units
    ids = {el.get("id"): el for el in root.iter() if el.get("id")}
    stops: dict[str, list] = {}

    def gradient_stops(gid: str, depth: int = 0) -> list:
        if gid in stops:
            return stops[gid]
        g = ids.get(gid)
        out = []
        if g is not None and depth < 8:
            for s in g:
                if _local(s.tag) == "stop":
                    st = _style(s)
                    c = _rgb(st.get("stop-color", s.get("stop-color", "black")))
                    if c is not None:
                        out.append(c)
            if not out:
                href = g.get("{http://www.w3.org/1999/xlink}href") or g.get("href") or ""
                if href.startswith("#"):
                    out = gradient_stops(href[1:], depth + 1)
        stops[gid] = out
        return out

    def paint(value: str | None, st: dict) -> tuple[bool, list]:
        if value is None:
            return False, []
        v = value.strip()
        if v.startswith("url("):
            m = re.match(r"url\(\s*#([^)\s]+)\s*\)", v)
            return True, gradient_stops(m.group(1)) if m else []
        if v.lower() == "currentcolor":
            v = st.get("color", "black")
        c = _rgb(v)
        return c is not None, [c] if c is not None else []

    out: list[Elem] = []

    def walk(node, inherited: dict, ctm, depth: int):
        for ch in node:
            tag = _local(ch.tag) if isinstance(ch.tag, str) else ""
            if not tag or tag in NONPAINT:
                continue
            own = _style(ch)
            if own.get("display") == "none":
                continue
            st = {k: v for k, v in inherited.items() if k in INHERITED}
            st.update(own)
            m = _affine_mul(ctm, _parse_transform(ch.get("transform")))
            if tag in ("g", "svg", "a", "switch"):
                walk(ch, st, m, depth)
            elif tag == "use" and depth < 4:
                href = ch.get("{http://www.w3.org/1999/xlink}href") or ch.get("href") or ""
                ref = ids.get(href[1:]) if href.startswith("#") else None
                if ref is not None:
                    mu = _affine_mul(m, (1.0, 0.0, 0.0, 1.0, _num(ch, "x"), _num(ch, "y")))
                    holder = ET.Element("g")
                    holder.append(ref)
                    walk(holder, st, mu, depth + 1)
            elif tag in ("path", "rect", "circle", "ellipse", "line", "polyline", "polygon"):
                element(tag, ch, st, m)

    def element(tag, el, st, ctm):
        b = _Builder(ctm)
        if tag == "path":
            _path(b, el.get("d") or "")
        elif tag == "rect":
            rxa, rya = el.get("rx"), el.get("ry")
            rx = _num(el, "rx") if rxa is not None else (_num(el, "ry") if rya is not None else 0.0)
            ry = _num(el, "ry") if rya is not None else rx
            _rect(b, _num(el, "x"), _num(el, "y"), _num(el, "width"), _num(el, "height"), rx, ry)
        elif tag == "circle":
            r = _num(el, "r")
            _ellipse(b, _num(el, "cx"), _num(el, "cy"), r, r)
        elif tag == "ellipse":
            _ellipse(b, _num(el, "cx"), _num(el, "cy"), _num(el, "rx"), _num(el, "ry"))
        elif tag == "line":
            b.start()
            b.add("L", [(_num(el, "x1"), _num(el, "y1")), (_num(el, "x2"), _num(el, "y2"))])
            b.end_sub(False)
        else:
            v = [float(t) for t in NUM.findall(el.get("points") or "")]
            pts = list(zip(v[0::2], v[1::2]))
            if len(pts) >= 2:
                b.start()
                for p, q in pairwise(pts):
                    b.add("L", [p, q])
                if tag == "polygon" and pts[-1] != pts[0]:
                    b.add("L", [pts[-1], pts[0]])
                b.end_sub(tag == "polygon")
        if not b.subs:
            return
        filled, fc = paint(st.get("fill", "black"), st)
        stroke_w = st.get("stroke-width", "1")
        m = NUM.match(stroke_w or "")
        sw = float(m.group(0)) if m else 1.0
        stroked, sc = paint(st.get("stroke"), st)
        stroked = stroked and sw > 0
        if not filled and not stroked:
            return
        out.append(Elem(tag, b.subs, filled, stroked, st.get("fill-rule", "") == "evenodd",
                        (fc if filled else []) + (sc if stroked else []),
                        sw * b.scale if stroked else 0.0))

    walk(root, _style(root), base, 0)
    return Doc(out, (fx, fy, fs))


# --------------------------------------------------------------------------- per-segment
def _unit(v):
    n = math.hypot(v[0], v[1])
    return (v[0] / n, v[1] / n) if n > 1e-15 else None


def _sub(p, q):
    return (q[0] - p[0], q[1] - p[1])


def _tangents(s: Seg):
    """Unit tangents at the segment's start and end (None when it has no direction)."""
    if s.kind == "A":
        a = s.arc
        return _unit(_arc_tangent(a, a[5])), _unit(_arc_tangent(a, a[5] + a[6]))
    pts = s.pts
    p0, p1 = pts[0], pts[-1]
    t0 = next((u for u in (_unit(_sub(p0, q)) for q in pts[1:]) if u), None)
    t1 = next((u for u in (_unit(_sub(q, p1)) for q in reversed(pts[:-1])) if u), None)
    return t0, t1


def _turn(u, v) -> float:
    """Unsigned angle between two unit directions, degrees."""
    return abs(math.degrees(math.atan2(u[0] * v[1] - u[1] * v[0], u[0] * v[0] + u[1] * v[1])))


def _axis_dev(u) -> float:
    a = math.degrees(math.atan2(u[1], u[0])) % 90.0
    return min(a, 90.0 - a)


def _length(s: Seg) -> float:
    pts = s.pts
    if s.kind == "L":
        return math.dist(pts[0], pts[1])
    if s.kind == "A":
        a = s.arc
        n = max(2, int(abs(a[6]) / (math.pi / 8)) + 1)
        q = [_arc_point(a, a[5] + a[6] * i / n) for i in range(n + 1)]
        return sum(math.dist(u, v) for u, v in pairwise(q))
    chord = math.dist(pts[0], pts[-1])
    poly = sum(math.dist(u, v) for u, v in pairwise(pts))
    return (chord + poly) / 2


def _sweep(s: Seg) -> float:
    """A curve's own turning, degrees: an arc's angle, a cubic's or quadratic's
    control-polygon turning."""
    if s.kind == "A":
        return abs(math.degrees(s.arc[6]))
    q = [s.pts[0]]
    for p in s.pts[1:]:
        if math.dist(p, q[-1]) > 1e-15:
            q.append(p)
    dirs = [_unit(_sub(u, v)) for u, v in pairwise(q)]
    return sum(_turn(u, v) for u, v in pairwise(dirs))


def _flatten(sub: Sub) -> np.ndarray:
    pts = [sub.segs[0].p0]
    for s in sub.segs:
        if s.kind == "L":
            pts.append(s.p1)
        elif s.kind == "A":
            a = s.arc
            pts += [_arc_point(a, a[5] + a[6] * i / 6) for i in range(1, 7)]
        else:
            p = np.asarray(s.pts)
            t = np.linspace(1 / 6, 1, 6)[:, None]
            if s.kind == "C":
                b = ((1 - t) ** 3) * p[0] + 3 * ((1 - t) ** 2) * t * p[1] + 3 * (1 - t) * t * t * p[2] + t ** 3 * p[3]
            else:
                b = ((1 - t) ** 2) * p[0] + 2 * (1 - t) * t * p[1] + t * t * p[2]
            pts += [tuple(r) for r in b]
    return np.asarray(pts, dtype=np.float64)


def _inside(pts: np.ndarray, ring: np.ndarray) -> np.ndarray:
    """Even-odd point-in-polygon of each row of `pts` against the closed `ring`."""
    x, y = pts[:, 0:1], pts[:, 1:2]
    x0, y0 = ring[:, 0][None, :], ring[:, 1][None, :]
    x1, y1 = np.roll(ring[:, 0], -1)[None, :], np.roll(ring[:, 1], -1)[None, :]
    cond = (y0 > y) != (y1 > y)
    with np.errstate(divide="ignore", invalid="ignore"):
        xc = x0 + (y - y0) * (x1 - x0) / (y1 - y0)
    return (np.count_nonzero(cond & (x < xc), axis=1) % 2) == 1


def _ties(values: list[float]) -> float:
    """Share of `values` another one repeats (within `TIE_TOL`); NaN for fewer than two."""
    if len(values) < 2:
        return float("nan")
    v = np.sort(np.asarray(values, dtype=np.float64))
    close = np.diff(v) <= TIE_TOL
    tied = np.zeros(v.size, dtype=bool)
    tied[1:] |= close
    tied[:-1] |= close
    return float(tied.mean())


def _gini(x: np.ndarray) -> float:
    if x.size == 0 or x.sum() <= 0:
        return float("nan")
    s = np.sort(x)
    n = s.size
    return float((2 * np.arange(1, n + 1) - n - 1) @ s / (n * s.sum()))


def _lab(rgbs: list) -> np.ndarray:
    if not rgbs:
        return np.zeros((0, 3))
    from skimage.color import rgb2lab
    return rgb2lab(np.asarray(rgbs, dtype=np.float64).reshape(-1, 1, 3)).reshape(-1, 3)


# --------------------------------------------------------------------------- profile
def _share(num: int, den: int) -> float:
    return num / den if den else float("nan")


def profile(svg: str, grid: tuple[float, float, float] | None = None,
            tol: float | None = None, raster_px: int | None = None) -> dict:
    """Every statistic of `svg`, as a dict: scalars, and for the distribution statistics
    the sample (a list). `grid` is the artist's frame (origin x, origin y, side, in the
    artist's units) that the grid statistics are read in; None reads the file in its own
    units, exactly (the artist's file). `tol` is the grid tolerance in artist units.
    `raster_px` is the side of the raster the canvas is drawn on, for `halfpx` (NaN
    without it)."""
    doc = parse(svg)
    grid = doc.frame if grid is None else grid
    tol = EXACT_TOL if tol is None else tol
    els = doc.elements
    page = bool(els) and _is_page(els[0])
    if page:
        els = els[1:]
    n_seg = {"L": 0, "C": 0, "Q": 0, "A": 0}
    lines = l_axis = l_diag = 0
    handles = h_axis = 0
    curved_joins = smooth = smooth_ext = 0
    cc_joins = cc_kink = 0
    ll_joins = ll_coll = 0
    joins = corners = right = 0
    corner_angles: list[float] = []
    sweeps: list[float] = []
    wide = 0
    handle_len: list[float] = []
    sym_joins = sym_ok = 0
    lengths: list[float] = []
    nodes: list[tuple[float, float]] = []
    n_sub = closed = 0
    for e in els:
        for sub in e.subs:
            segs = [(s, _length(s)) for s in sub.segs]
            segs = [(s, L) for s, L in segs if L >= DOT]
            if not segs:
                continue
            n_sub += 1
            closed += sub.closed
            info = []
            for s, L in segs:
                n_seg[s.kind] += 1
                lengths.append(L)
                t0, t1 = _tangents(s)
                info.append((s, t0, t1))
                nodes.append(s.p0)
                if s.kind == "L":
                    lines += 1
                    if t0 is not None:
                        dev = _axis_dev(t0)
                        l_axis += dev <= AXIS_DEG
                        l_diag += abs(dev - 45.0) <= AXIS_DEG
                else:
                    sw = _sweep(s)
                    sweeps.append(sw)
                    wide += sw > WIDE_DEG
                if s.kind == "C":
                    chord = math.dist(s.p0, s.p1)
                    for p, q in ((s.pts[0], s.pts[1]), (s.pts[3], s.pts[2])):
                        v = _sub(p, q)
                        hl = math.hypot(*v)
                        if hl > 1e-12:
                            handles += 1
                            h_axis += _axis_dev((v[0] / hl, v[1] / hl)) <= AXIS_DEG
                            if chord > 1e-12:
                                handle_len.append(hl / chord)
            if not sub.closed:
                nodes.append(segs[-1][0].p1)
            pairs = list(pairwise(info))
            if sub.closed and len(info) > 1:
                pairs.append((info[-1], info[0]))
            for (sa, _, ta), (sb, tb, _) in pairs:
                if ta is None or tb is None:
                    continue
                joins += 1
                t = _turn(ta, tb)
                ca, cb = sa.kind != "L", sb.kind != "L"
                if t > CORNER_DEG:
                    corners += 1
                    corner_angles.append(t)
                    right += abs(t - 90.0) <= RIGHT_DEG
                if ca or cb:
                    curved_joins += 1
                    if t <= SMOOTH_DEG:
                        smooth += 1
                        smooth_ext += _axis_dev(tb) <= EXTREMUM_DEG
                if ca and cb:
                    cc_joins += 1
                    cc_kink += SMOOTH_DEG < t <= CORNER_DEG
                    if sa.kind == "C" and sb.kind == "C" and t <= SMOOTH_DEG:
                        h_in = math.dist(sa.pts[2], sa.pts[3])
                        h_out = math.dist(sb.pts[0], sb.pts[1])
                        if h_in > 1e-12 and h_out > 1e-12:
                            sym_joins += 1
                            sym_ok += abs(math.log(h_in / h_out)) <= math.log(1.05)
                if not ca and not cb:
                    ll_joins += 1
                    ll_coll += t <= AXIS_DEG
    n_all = sum(n_seg.values())
    p: dict = {
        "kinds": [n_seg[k] / n_all for k in "LCQA"] if n_all else None,
        "primitives": _share(sum(e.tag in PRIMITIVE_TAGS for e in els), len(els)),
        "stroked": _share(sum(e.stroked for e in els), len(els)),
        "stroke_width_ties": _ties([e.stroke_width for e in els if e.stroked]),
        "elements": len(els),
        "subpaths": n_sub,
        "nodes_per_subpath": len(nodes) / n_sub if n_sub else float("nan"),
        "closed": _share(closed, n_sub),
        "line_axis": _share(l_axis, lines),
        "line_diag": _share(l_diag, lines),
        "handle_axis": _share(h_axis, handles),
        "smooth": _share(smooth, curved_joins),
        "extremum": _share(smooth_ext, smooth),
        "kinked": _share(cc_kink, cc_joins),
        "collinear": _share(ll_coll, ll_joins),
        "corner_share": _share(corners, joins),
        "right_angle": _share(right, corners),
        "corner_angles": corner_angles,
        "sweep": sweeps,
        "wide_curves": _share(wide, len(sweeps)),
        "handle_length": handle_len,
        "symmetric_handles": _share(sym_ok, sym_joins),
        "segment_lengths": [math.log10(x) for x in lengths if x > 0],
        "length_gini": _gini(np.asarray(lengths)),
    }
    p.update(_numbers(nodes, grid, tol))
    p["halfpx"] = _halfpx(nodes, raster_px)
    p.update(_paint_and_layers(els))
    p["page"] = page
    return p


#: A first element filled in white (every channel at least this) and spanning this share of
#: the canvas both ways is the page an opaque input was traced on, not part of the drawing.
PAGE_WHITE = 0.94
PAGE_SPAN = 0.98


def _is_page(e: Elem) -> bool:
    if not e.filled or e.stroked or not e.colours:
        return False
    if any(min(c) < PAGE_WHITE for c in e.colours):
        return False
    pts = np.asarray([p for s in e.subs for seg in s.segs for p in seg.pts])
    if pts.size == 0:
        return False
    span = pts.max(0) - pts.min(0)
    return bool(np.all(span >= PAGE_SPAN))


def _halfpx(nodes, raster_px: int | None) -> float:
    """Share of node coordinates on the raster's half-pixel grid (pixel edges and centres),
    within `GRID_TOL_PX`, the canvas drawn on a `raster_px` raster."""
    if not nodes or not raster_px:
        return float("nan")
    v = np.asarray(nodes, dtype=np.float64).reshape(-1) * raster_px * 2
    return float((np.abs(v - np.round(v)) / 2 <= GRID_TOL_PX).mean())


def _numbers(nodes, grid, tol) -> dict:
    """The grid, decimals, ties and mirror statistics of the nodes (canvas units); `grid` is
    the frame the grid is read in, `tol` its tolerance in that frame's units."""
    if not nodes:
        return {"grid_half": float("nan"), "grid_int": float("nan"), "grid_level": float("nan"),
                "decimals": float("nan"), "ties": float("nan"), "mirror": float("nan")}
    xy = np.asarray(nodes, dtype=np.float64)
    gx, gy, gs = grid
    units = np.concatenate([gx + xy[:, 0] * gs, gy + xy[:, 1] * gs])

    def on(k: float) -> np.ndarray:
        v = units * k
        return np.abs(v - np.round(v)) <= tol * k

    level = len(GRID_LADDER)
    for i, k in enumerate(GRID_LADDER):
        if on(k).mean() >= GRID_LEVEL_SHARE:
            level = i
            break
    dec = np.full(units.shape, 4.0)
    for d in (3, 2, 1, 0):
        dec[on(10.0 ** d)] = d
    ties = 0
    for col in (xy[:, 0], xy[:, 1]):
        s = np.sort(col)
        close = np.diff(s) <= TIE_TOL
        tied = np.zeros(s.size, dtype=bool)
        tied[1:] |= close
        tied[:-1] |= close
        ties += int(tied.sum())
    from scipy.spatial import cKDTree
    tree = cKDTree(xy)
    lo, hi = xy.min(0), xy.max(0)
    best = 0.0
    for axis in (0, 1):
        m = xy.copy()
        m[:, axis] = lo[axis] + hi[axis] - m[:, axis]
        d, _ = tree.query(m, k=1)
        best = max(best, float((d <= MIRROR_TOL).mean()))
    return {"grid_half": float(on(2).mean()), "grid_int": float(on(1).mean()),
            "grid_level": float(level), "decimals": float(dec.mean()),
            "ties": ties / (2 * len(xy)), "mirror": best}


def _paint_and_layers(els: list[Elem]) -> dict:
    cols = {tuple(round(c * 255) for c in rgb) for e in els for rgb in e.colours}
    filled = [e for e in els if e.filled]
    rings_of: list[list[tuple[np.ndarray, bool]]] = []
    for e in filled:
        rings = []
        for s in e.subs:
            r = _flatten(s)
            if len(r) >= 3:
                rings.append((r, s.closed))
        rings_of.append(rings)
    # nested: a closed subpath inside another subpath of the same element (its first point
    # inside the other's outline, and its box inside the other's box)
    n_closed = n_nested = 0
    for rings in rings_of:
        n_closed += sum(c for _, c in rings)
        if len(rings) < 2:
            continue
        firsts = np.asarray([r[0] for r, _ in rings])
        lo = np.asarray([r.min(0) for r, _ in rings])
        hi = np.asarray([r.max(0) for r, _ in rings])
        nested = np.zeros(len(rings), dtype=bool)
        for i, (ri, _) in enumerate(rings):
            inside = _inside(firsts, ri) & np.all(lo >= lo[i], axis=1) & np.all(hi <= hi[i], axis=1)
            inside[i] = False
            nested |= inside
        n_nested += int(sum(n and c for n, (_, c) in zip(nested, rings)))
    # stacked: an interior point of the element (its largest outline's vertex mean, when that
    # is inside the element) lies inside an earlier filled element (even-odd)
    probes = np.full((len(rings_of), 2), np.nan)
    for j, rings in enumerate(rings_of):
        if rings:
            big = max((r for r, _ in rings), key=lambda r: float(np.ptp(r[:, 0]) * np.ptp(r[:, 1])))
            c = big.mean(0, keepdims=True)
            if sum(bool(_inside(c, r)[0]) for r, _ in rings) % 2 == 1:
                probes[j] = c[0]
    has = np.isfinite(probes[:, 0])
    over = np.zeros(len(rings_of), dtype=bool)
    for i, rings in enumerate(rings_of[:-1]):
        later = np.flatnonzero(has[i + 1:]) + i + 1
        if not rings or later.size == 0:
            continue
        parity = np.zeros(later.size, dtype=bool)
        for r, _ in rings:
            parity ^= _inside(probes[later], r)
        over[later[parity]] = True
    stacked, counted = int(over[has].sum()), int(has.sum())
    return {"colours": len(cols), "palette": [list(c) for c in sorted(cols)],
            "nested": _share(n_nested, n_closed),
            "stacked": _share(stacked, counted),
            "evenodd": _share(sum(e.evenodd for e in filled), len(filled))}


# --------------------------------------------------------------------------- divergence
#: How each statistic's divergence is taken (module documentation).
SHARE = "share"
COUNT = "count"
DIST = "dist"
MIX = "mix"
PALETTE = "palette"
STATS: dict[str, tuple[str, float]] = {
    # name: (kind, scale). The order is the pruning's priority (`KEPT`): where two carry one
    # signal the earlier stays, so the node conventions `--editability` works on come first,
    # then the numbers, the pieces and the paint.
    "kinds": (MIX, 1.0),
    "smooth": (SHARE, 1.0),
    "extremum": (SHARE, 1.0),
    "handle_axis": (SHARE, 1.0),
    "kinked": (SHARE, 1.0),
    "corner_share": (SHARE, 1.0),
    "right_angle": (SHARE, 1.0),
    "corner_angles": (DIST, 180.0),
    "line_axis": (SHARE, 1.0),
    "line_diag": (SHARE, 1.0),
    "collinear": (SHARE, 1.0),
    "sweep": (DIST, 180.0),
    "wide_curves": (SHARE, 1.0),
    "handle_length": (DIST, 1.0),
    "symmetric_handles": (SHARE, 1.0),
    "grid_half": (SHARE, 1.0),
    "grid_int": (SHARE, 1.0),
    "grid_level": (SHARE, 5.0),
    "decimals": (SHARE, 4.0),
    "ties": (SHARE, 1.0),
    "mirror": (SHARE, 1.0),
    "primitives": (SHARE, 1.0),
    "stroked": (SHARE, 1.0),
    "stroke_width_ties": (SHARE, 1.0),
    "elements": (COUNT, 1.0),
    "subpaths": (COUNT, 1.0),
    "nodes_per_subpath": (COUNT, 1.0),
    "closed": (SHARE, 1.0),
    "segment_lengths": (DIST, 1.0),
    "length_gini": (SHARE, 1.0),
    "colours": (COUNT, 1.0),
    "palette": (PALETTE, 1.0),
    "nested": (SHARE, 1.0),
    "stacked": (SHARE, 1.0),
    "evenodd": (SHARE, 1.0),
    "halfpx": (SHARE, 1.0),
}
#: Statistics that flag a tracer's artefact rather than a way of drawing: reported with the
#: others, left out of the human-closeness composite (`bench/human_stats.py`).
ARTEFACTS = ("halfpx",)


def _w1(a, b) -> float:
    """Wasserstein-1 distance between two empirical distributions on the line."""
    a, b = np.sort(np.asarray(a, dtype=np.float64)), np.sort(np.asarray(b, dtype=np.float64))
    if a.size == 0 or b.size == 0:
        return float("nan")
    allv = np.concatenate([a, b])
    allv.sort(kind="mergesort")
    d = np.diff(allv)
    fa = np.searchsorted(a, allv[:-1], side="right") / a.size
    fb = np.searchsorted(b, allv[:-1], side="right") / b.size
    return float(np.sum(np.abs(fa - fb) * d))


def _palette_distance(a: list, b: list) -> float:
    if not a or not b:
        return float("nan")
    la, lb = _lab([[c / 255 for c in x] for x in a]), _lab([[c / 255 for c in x] for x in b])
    d = np.sqrt(((la[:, None, :] - lb[None, :, :]) ** 2).sum(-1))
    return float((d.min(1).mean() + d.min(0).mean()) / 2)


def _nan(x) -> bool:
    return x is None or (isinstance(x, float) and math.isnan(x))


def divergence(trace: dict, artist: dict) -> dict[str, float]:
    """Per statistic, how far `trace`'s value is from `artist`'s (module documentation);
    NaN where either has no value."""
    out = {}
    for name, (kind, scale) in STATS.items():
        t, a = trace.get(name), artist.get(name)
        if kind == PALETTE:
            out[name] = _palette_distance(t or [], a or [])
        elif kind == MIX:
            out[name] = float("nan") if t is None or a is None else \
                0.5 * sum(abs(x - y) for x, y in zip(t, a))
        elif kind == DIST:
            out[name] = _w1(t or [], a or []) / scale
        elif _nan(t) or _nan(a):
            out[name] = float("nan")
        elif kind == COUNT:
            out[name] = abs(math.log((t + 0.5) / (a + 0.5)) if min(t, a) <= 0 else math.log(t / a))
        else:
            out[name] = abs(t - a) / scale
    return out


def summary_value(name: str, value) -> float:
    """One number for a statistic's value in a file, for tables: the value itself, a
    distribution's median, the mix's share of lines, a palette's size."""
    kind = STATS[name][0]
    if value is None:
        return float("nan")
    if kind == DIST:
        return float(np.median(value)) if len(value) else float("nan")
    if kind == MIX:
        return float(value[1])          # the share of cubics
    if kind == PALETTE:
        return float(len(value))
    return float(value)


def artist_grid(artist_svg: str) -> tuple[float, float, float]:
    """The artist's frame, read once: where its grid is."""
    return frame_of(ET.fromstring(artist_svg))


def trace_tolerance(artist_svg: str, raster_px: int) -> float:
    """`GRID_TOL_PX` raster px in the artist's units."""
    return GRID_TOL_PX * artist_grid(artist_svg)[2] / max(1, raster_px)


def compare(trace_svg: str, artist_svg: str, raster_px: int, artist: dict | None = None) -> dict:
    """{"trace": profile, "artist": profile, "div": divergences} of one icon; pass the
    artist's profile when it is already known."""
    if artist is None:
        artist = profile(artist_svg, raster_px=raster_px)
    t = profile(trace_svg, artist_grid(artist_svg), trace_tolerance(artist_svg, raster_px),
                raster_px)
    return {"trace": t, "artist": artist, "div": divergence(t, artist)}


#: Candidates the battery does not report, and why. Pruned on the Spearman correlation of the
#: per-icon divergences over the 246 screen icons, quality-512ssop and quality-web pooled
#: (`bench/human_stats.py --correlate`, 2026-10-10): a candidate correlated beyond |rho| 0.7
#: with one kept before it in `STATS` order carries no signal of its own.
DROPPED = {
    "wide_curves": "rho +0.72 with sweep (a curve turning over 100 degrees is the sweep's tail)",
    "grid_int": "rho +0.98 with grid_half (the whole-unit grid is inside the half-unit one)",
    "grid_level": "rho +0.72 with grid_half",
    "decimals": "rho +0.74 with grid_half",
    "length_gini": "rho +0.79 with segment_lengths (the spread is part of the distribution)",
    "palette": "rho +0.84 with colours",
    "evenodd": "nonzero on 6 of 616 icon-conditions: neither the tracer nor the artists use it",
}
#: The statistics the battery reports: `STATS` less `DROPPED`. Every gate axis stays below
#: |rho| 0.57 with each of them (largest: colours with de00, 0.56; smooth with geom, 0.54).
KEPT: tuple[str, ...] = tuple(s for s in STATS if s not in DROPPED)
