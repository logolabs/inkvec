"""The renderer floor of the corpus intake, measured against the artists' own geometry.

`docs/theory/chain-boundary.md` scores a description by window areas with calibrated
variances. Its calibration (amendment A1 of `chain-representation.md`) asks: when the
artist's own SVG is scored against the intake the gate feeds the tracer (resvg at 8x,
box-filtered, 8-bit; `bench/build_corpus_v2.py`), what is left over, and which part of it
is structure a forward model must reproduce rather than noise it may average?

Each flat-paint icon of the screen set (no gradients, clips, masks, opacity, `<use>`) is
drawn exactly, with no anti-aliasing approximation: its elements become shapely regions
(fill rules by winding number, strokes by buffering the centreline with the SVG's caps,
joins and miter limit), and each region's exact area per pixel comes from
`exact_raster.py`. Three models:

* `true`  the true geometry (arcs, circles and ellipses as ellipses), the visible partition
          (each element minus everything painted above it);
* `usvg`  usvg's geometry (every arc, circle, ellipse and rounded corner converted to cubics
          by kurbo at 0.1 user units, as `usvg::parser::shapes` does), the visible partition;
* `as8`   usvg's geometry composited element by element with exact coverage at 8x, then
          box-filtered: resvg's own procedure, except that resvg's coverage of each 8x pixel
          is anti-aliased by tiny-skia (4x4 supersampling, curves flattened) where this is exact.

The intake's residual against each model is projected, at every pixel two inks share, onto
the axis between them (a coverage), and summed over windows: maximal runs of such pixels
along a column (where the boundary is within 45 degrees of horizontal) or a row
(elsewhere), each pixel in exactly one window. Reported per family and tier:

* `rms_q`   window error of the 8-bit quantisation alone (`as8` rounded as the intake is);
* `rms_*`   window error of the intake against each model;
* `floor`   `sqrt(rms_as8^2 - rms_q^2)`: the renderer floor, what tiny-skia adds;
* `bias`    mean signed window error against `as8` (coverage of the ink on the left of the
            boundary's direction), and `chain`: the share of the floor's variance that is a
            constant per connected run of windows (one edge), A3's correlation;
* `selfcal` the fourth-difference estimate of the window variance from the intake alone,
            over the truth-based one;
* `curve`, `line`, `stroke`, `fill-ln`: `rms_as8` restricted to windows on curves, on
            straight segments, on stroke outlines, and on straight segments of fills;
* `seam`    window error between `as8` and `usvg` (what per-element compositing at 8x adds
            along abutting edges), and at junction pixels the size of the compositing term.

With `--fresh`, the icon is also rendered now by the harness's own code
(`build_corpus_v2.render_supersampled`, resvg-py), and:

* `fresh`   window error of that render against `as8`; `stale` the largest difference, in
            8-bit levels, between it and the committed intake (0: the intake is reproducible);
* `lattice` window error of that render against `as8` drawn with tiny-skia's sample lattice
            (4 x 4 points per 8x pixel, `lattice_coverage`), and `lat-ln`, `lat-cv`, `lat-st`
            the same on straight fill segments, curves and stroke outlines: what is left is
            tiny-skia's curve flattening and its stroker.

    python3 bench/theory/renderer_floor.py [--per-family 6] [--tiers 128ss,512ss] [--fresh]

Needs svgelements, shapely, scipy and resvg-py (about 25 minutes with `--fresh` for six
icons per family at both tiers).
"""

from __future__ import annotations

import argparse
import io
import math
import re
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np
from PIL import Image

import exact_raster as er

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))

TOL_PX = 2e-4          # flattening tolerance of the exact models, px
ARC_TOL = 0.1          # usvg's arc-to-cubic tolerance, user units
SS = 8                 # the intake's supersampling
EPS_COV = 1e-6         # an ink takes part in a pixel above this coverage


# ------------------------------------------------------------------- kurbo's arc, in Python

def svg_arc(p0, p1, rx, ry, xrot, large, sweep):
    """kurbo::Arc::from_svg_arc: (center, rx, ry, start, sweep, xrot) or None (a line)."""
    if abs(p0[0] - p1[0]) < 1e-12 and abs(p0[1] - p1[1]) < 1e-12:
        return None
    rx, ry = abs(rx), abs(ry)
    if rx < 1e-5 or ry < 1e-5:
        return None
    sp, cp = math.sin(xrot), math.cos(xrot)
    hdx, hdy = (p0[0] - p1[0]) / 2, (p0[1] - p1[1]) / 2
    hsx, hsy = (p0[0] + p1[0]) / 2, (p0[1] + p1[1]) / 2
    px, py = cp * hdx + sp * hdy, -sp * hdx + cp * hdy
    rf = px * px / (rx * rx) + py * py / (ry * ry)
    if rf > 1:
        rx, ry = rx * math.sqrt(rf), ry * math.sqrt(rf)
    rxry, rxpy, rypx = rx * ry, rx * py, ry * px
    s2 = rxpy * rxpy + rypx * rypx
    sign = -1.0 if large == sweep else 1.0
    coe = sign * math.sqrt(abs((rxry * rxry - s2) / s2))
    tcx, tcy = coe * rxpy / ry, -coe * rypx / rx
    c = (cp * tcx - sp * tcy + hsx, sp * tcx + cp * tcy + hsy)
    sa = math.atan2((py - tcy) / ry, (px - tcx) / rx)
    ea = math.atan2((-py - tcy) / ry, (-px - tcx) / rx)
    sw = math.fmod(ea - sa, 2 * math.pi)
    if sweep and sw < 0:
        sw += 2 * math.pi
    elif not sweep and sw > 0:
        sw -= 2 * math.pi
    return c, rx, ry, sa, sw, xrot


def _ell(rx, ry, xrot, a):
    u, v = rx * math.cos(a), ry * math.sin(a)
    return (u * math.cos(xrot) - v * math.sin(xrot), u * math.sin(xrot) + v * math.cos(xrot))


def arc_cubics(arc, start):
    """kurbo's append_iter(ARC_TOL): the cubics usvg writes for an arc, from `start`."""
    c, rx, ry, a0, sw, xr = arc
    n_err = max((1.1163 * max(rx, ry) / ARC_TOL) ** (1 / 6), 3.999999)
    n = int(math.ceil(n_err * abs(sw) / (2 * math.pi)))
    step = sw / n
    arm = (4 / 3) * math.tan(abs(0.25 * step)) * math.copysign(1, sw)
    out, prev = [], start
    p0 = _ell(rx, ry, xr, a0)
    for _ in range(n):
        a1 = a0 + step
        d0 = _ell(rx, ry, xr, a0 + math.pi / 2)
        p3 = _ell(rx, ry, xr, a1)
        d3 = _ell(rx, ry, xr, a1 + math.pi / 2)
        q1 = (c[0] + p0[0] + arm * d0[0], c[1] + p0[1] + arm * d0[1])
        q2 = (c[0] + p3[0] - arm * d3[0], c[1] + p3[1] - arm * d3[1])
        q3 = (c[0] + p3[0], c[1] + p3[1])
        out.append(("C", prev, q1, q2, q3))
        prev, p0, a0 = q3, p3, a1
    return out


def arc_true(arc, start, end):
    return [("A", start, arc, end)]


# ---------------------------------------------------------------- elements to subpaths

def _shape_segments(e, se, mode):
    """Subpaths of one svgelements shape in its local units, as usvg builds them:
    [(segments, closed)], segments ('L', p, q), ('C', p, c1, c2, q), ('A', p, arc, q)."""
    def arc_to(prev, to, rx, ry, xrot_deg, large, sweep):
        a = svg_arc(prev, to, rx, ry, math.radians(xrot_deg), large, sweep)
        if a is None:
            return [("L", prev, to)]
        return arc_cubics(a, prev) if mode == "usvg" else arc_true(a, prev, to)

    if isinstance(e, (se.Circle, se.Ellipse)):
        cx, cy, rx, ry = float(e.cx), float(e.cy), float(e.rx), float(e.ry)
        if rx <= 0 or ry <= 0:
            return []
        pts = [(cx + rx, cy), (cx, cy + ry), (cx - rx, cy), (cx, cy - ry), (cx + rx, cy)]
        segs = []
        for k in range(4):
            segs += arc_to(pts[k], pts[k + 1], rx, ry, 0.0, False, True)
        return [(segs, True)]
    if isinstance(e, se.Rect):
        x, y, w, h = float(e.x), float(e.y), float(e.width), float(e.height)
        rx, ry = float(e.rx or 0), float(e.ry or 0)
        rx, ry = min(rx, w / 2), min(ry, h / 2)
        if rx <= 0 or ry <= 0:
            p = [(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)]
            return [([("L", p[k], p[k + 1]) for k in range(4)], True)]
        segs, cur = [], (x + rx, y)

        def line(q):
            nonlocal cur
            segs.append(("L", cur, q))
            cur = q

        def arc(q):
            nonlocal cur
            segs.extend(arc_to(cur, q, rx, ry, 0.0, False, True))
            cur = q
        line((x + w - rx, y)); arc((x + w, y + ry)); line((x + w, y + h - ry))
        arc((x + w - rx, y + h)); line((x + rx, y + h)); arc((x, y + h - ry))
        line((x, y + ry)); arc((x + rx, y))
        return [(segs, True)]
    if isinstance(e, (se.SimpleLine,)):
        return [([("L", (float(e.x1), float(e.y1)), (float(e.x2), float(e.y2)))], False)]
    if isinstance(e, (se.Polyline, se.Polygon)):
        pts = [(float(p.x), float(p.y)) for p in e.points]
        segs = [("L", pts[k], pts[k + 1]) for k in range(len(pts) - 1)]
        return [(segs, isinstance(e, se.Polygon))]
    if isinstance(e, se.Path):
        out, segs, closed = [], [], False
        start = None
        for s in e:
            if isinstance(s, se.Move):
                if segs:
                    out.append((segs, closed))
                segs, closed, start = [], False, (s.end.x, s.end.y)
            elif isinstance(s, se.Close):
                if s.start is not None and s.end is not None and (s.start != s.end):
                    segs.append(("L", (s.start.x, s.start.y), (s.end.x, s.end.y)))
                closed = True
                out.append((segs, closed))
                segs, closed = [], False
            elif isinstance(s, se.Line):
                segs.append(("L", (s.start.x, s.start.y), (s.end.x, s.end.y)))
            elif isinstance(s, se.CubicBezier):
                segs.append(("C", (s.start.x, s.start.y), (s.control1.x, s.control1.y),
                             (s.control2.x, s.control2.y), (s.end.x, s.end.y)))
            elif isinstance(s, se.QuadraticBezier):
                p0, q, p1 = (s.start.x, s.start.y), (s.control.x, s.control.y), (s.end.x, s.end.y)
                c1 = (p0[0] + 2 / 3 * (q[0] - p0[0]), p0[1] + 2 / 3 * (q[1] - p0[1]))
                c2 = (p1[0] + 2 / 3 * (q[0] - p1[0]), p1[1] + 2 / 3 * (q[1] - p1[1]))
                segs.append(("C", p0, c1, c2, p1))
            elif isinstance(s, se.Arc):
                sw = float(s.sweep)
                segs += arc_to((s.start.x, s.start.y), (s.end.x, s.end.y), float(s.rx), float(s.ry),
                               math.degrees(float(s.get_rotation())), abs(sw) > math.pi, sw > 0)
        if segs:
            out.append((segs, closed))
        return out
    return []


def _flatten(segs, tf, tol):
    """Polyline (px) of a subpath under the affine `tf` (a, b, c, d, e, f), and per vertex
    whether it came from a curve."""
    a, b, c, d, e, f = tf
    scale = math.sqrt(abs(a * d - b * c)) or 1.0
    t_loc = tol / scale
    pts, curve = [], []

    def put(p, cv):
        pts.append((a * p[0] + c * p[1] + e, b * p[0] + d * p[1] + f))
        curve.append(cv)
    for s in segs:
        if not pts:
            put(s[1], False)
        if s[0] == "L":
            put(s[2], False)
        elif s[0] == "C":
            p0, p1, p2, p3 = (np.array(v) for v in s[1:])
            m = max(np.linalg.norm(p0 - 2 * p1 + p2), np.linalg.norm(p1 - 2 * p2 + p3))
            n = max(1, int(math.ceil(math.sqrt(6 * m / (8 * t_loc)))))
            for k in range(1, n + 1):
                t = k / n
                q = (1 - t) ** 3 * p0 + 3 * (1 - t) ** 2 * t * p1 + 3 * (1 - t) * t * t * p2 + t ** 3 * p3
                put(q, True)
        elif s[0] == "A":
            (cx, cy), rx, ry, a0, sw, xr = s[2]
            r = max(rx, ry)
            dth = 2 * math.acos(max(-1.0, 1 - t_loc / r)) if r > t_loc else math.pi / 2
            n = max(2, int(math.ceil(abs(sw) / dth)))
            for k in range(1, n + 1):
                u, v = _ell(rx, ry, xr, a0 + sw * k / n)
                put((cx + u, cy + v), True)
            # the arc ends exactly where the path says
    return pts, curve


def _winding(pt, ring):
    x, y = pt
    r = np.asarray(ring)
    x0, y0, x1, y1 = r[:, 0], r[:, 1], np.roll(r[:, 0], -1), np.roll(r[:, 1], -1)
    up = (y0 <= y) & (y1 > y)
    dn = (y0 > y) & (y1 <= y)
    cross = (x1 - x0) * (y - y0) - (x - x0) * (y1 - y0)
    return int(np.sum(up & (cross > 0)) - np.sum(dn & (cross < 0)))


def _fill_region(rings, evenodd):
    import shapely
    from shapely.geometry import LineString, Polygon
    from shapely.ops import polygonize, unary_union
    rings = [r for r in rings if len(r) >= 3]
    if not rings:
        return None
    if len(rings) == 1:
        p = Polygon(rings[0])
        return p if p.is_valid else shapely.make_valid(p)
    lines = unary_union([LineString(list(r) + [r[0]]) for r in rings])
    keep = []
    for face in polygonize(lines):
        rp = face.representative_point()
        w = sum(_winding((rp.x, rp.y), r) for r in rings)
        if (w % 2 != 0) if evenodd else (w != 0):
            keep.append(face)
    return unary_union(keep) if keep else None


def _stroke_region(paths, width, cap, join, miter):
    from shapely.geometry import LineString, LinearRing
    from shapely.ops import unary_union
    cap_style = {"butt": "flat", "round": "round", "square": "square"}.get(cap, "flat")
    join_style = {"miter": "mitre", "round": "round", "bevel": "bevel"}.get(join, "mitre")
    parts = []
    for pts, closed in paths:
        q = [p for k, p in enumerate(pts) if k == 0 or p != pts[k - 1]]
        if len(q) == 1:
            if cap_style == "flat":
                continue
            q = [q[0], (q[0][0] + 1e-9, q[0][1])]
        g = LinearRing(q) if closed and len(q) >= 3 else LineString(q)
        parts.append(g.buffer(width / 2, quad_segs=48, cap_style=cap_style,
                              join_style=join_style, mitre_limit=miter))
    return unary_union(parts) if parts else None


def layers_of(svg_text: str, size: int, mode: str):
    """[(region, premultiplied rgba, kind, curve_points)] in paint order, in px of a `size`
    raster in the gate's frame, or None when the file uses what this does not draw."""
    import svgelements as se
    from inkvec_bench import render
    if re.search(r"Gradient|<mask|clip-path|clipPath|<filter|<pattern|<image|<text|stroke-dash|<use|opacity",
                 svg_text):
        return None
    s, _ = render.normalize_svg(svg_text)
    s = render.fit_viewbox(s, size, size)
    m = re.search(r"<svg\b[^>]*>", s)
    tag = re.sub(r'\s(width|height)="[^"]*"', "", m.group(0))
    tag = tag[:-1] + f' width="{size}" height="{size}">'
    s = s[:m.start()] + tag + s[m.end():]
    doc = se.SVG.parse(io.StringIO(s), reify=False)
    out = []
    for e in doc.elements():
        if not isinstance(e, se.Shape) or isinstance(e, se.Text):
            continue
        if e.values.get("visibility") == "hidden" or e.values.get("display") == "none":
            continue
        tf = e.transform
        tf = (tf.a, tf.b, tf.c, tf.d, tf.e, tf.f)
        sub = _shape_segments(e, se, mode)
        if not sub:
            continue
        flat = [(_flatten(segs, tf, TOL_PX), closed) for segs, closed in sub]
        curve_pts = [p for (pts, cv), _ in flat for p, c in zip(pts, cv) if c]
        fill = e.fill
        if fill is not None and fill.value is not None and fill.alpha > 0:
            rings = [pts for (pts, _), _ in flat]
            reg = _fill_region(rings, e.values.get("fill-rule") == "evenodd")
            if reg is not None and not reg.is_empty:
                out.append((reg, (fill.red / 255, fill.green / 255, fill.blue / 255), "fill", curve_pts))
        st = e.stroke
        if st is not None and st.value is not None and st.alpha > 0:
            scale = math.sqrt(abs(tf[0] * tf[3] - tf[1] * tf[2]))
            width = float(e.stroke_width) * scale
            if width > 0:
                reg = _stroke_region([(pts, closed) for (pts, _), closed in flat], width,
                                     e.values.get("stroke-linecap", "butt"),
                                     e.values.get("stroke-linejoin", "miter"),
                                     float(e.values.get("stroke-miterlimit", 4)))
                if reg is not None and not reg.is_empty:
                    out.append((reg, (st.red / 255, st.green / 255, st.blue / 255), "stroke", curve_pts))
    return out


# ------------------------------------------------------------------------- exact rasters

def coverage(geom, w: int, h: int, scale: float = 1.0, y0: int = 0, y1: int | None = None):
    """Exact area coverage of `geom` (shapely, px) on a `w x h` raster at `scale`, rows
    `y0..y1` of the scaled raster: (rows, w*scale) float64."""
    import shapely
    from shapely import affinity
    from shapely.geometry import box
    from shapely.geometry.polygon import orient
    W, H = int(round(w * scale)), int(round(h * scale))
    y1 = H if y1 is None else y1
    out = np.zeros((y1 - y0, W))
    g = affinity.scale(geom, scale, scale, origin=(0, 0)) if scale != 1 else geom
    g = g.intersection(box(0, y0, W, y1))
    if g.is_empty:
        return out
    polys = [p for p in getattr(g, "geoms", [g]) if p.geom_type == "Polygon" and p.area > 0]
    for p in polys:
        p = orient(p, 1.0)
        minx, miny, maxx, maxy = p.bounds
        bx0, by0 = max(0, int(math.floor(minx))), max(y0, int(math.floor(miny)))
        bx1, by1 = min(W, int(math.ceil(maxx))), min(y1, int(math.ceil(maxy)))
        if bx1 <= bx0 or by1 <= by0:
            continue
        rings = [np.asarray(p.exterior.coords)[:-1] - (bx0, by0)]
        rings += [np.asarray(r.coords)[:-1] - (bx0, by0) for r in p.interiors]
        rings = [np.clip(r, 0, None) for r in rings]
        c = er.rasterize(rings, bx1 - bx0, by1 - by0)
        out[by0 - y0:by1 - y0, bx0:bx1] += c
    return np.clip(out, 0, 1)


def visible_regions(layers):
    from shapely.ops import unary_union
    covered, vis = None, []
    for reg, col, kind, cpts in reversed(layers):
        v = reg if covered is None else reg.difference(covered)
        covered = reg if covered is None else unary_union([covered, reg])
        vis.append((v, col, kind, cpts))
    return list(reversed(vis))


def render_partition(vis, size):
    """Premultiplied RGBA of the visible partition, exact area, and per-layer coverages."""
    img = np.zeros((size, size, 4))
    covs = []
    for v, col, _, _ in vis:
        c = coverage(v, size, size) if not v.is_empty else np.zeros((size, size))
        covs.append(c)
        img += c[..., None] * np.array([*col, 1.0])
    return img, covs


def render_layered(layers, size, scale, strip=16):
    """Premultiplied RGBA of per-element "over" compositing with exact coverage at `scale`,
    box-filtered to `size`: resvg's procedure with exact coverage."""
    img = np.zeros((size, size, 4))
    for r0 in range(0, size, strip):
        r1 = min(size, r0 + strip)
        acc = np.zeros(((r1 - r0) * scale, size * scale, 4))
        for reg, col, _, _ in layers:
            c = coverage(reg, size, size, scale, r0 * scale, r1 * scale)
            src = np.array([*col, 1.0])
            acc = c[..., None] * src + (1 - c[..., None]) * acc
        img[r0:r1] = acc.reshape(r1 - r0, scale, size, scale, 4).mean(axis=(1, 3))
    return img


def _rings(geom):
    out = []
    for g in getattr(geom, "geoms", [geom]):
        if g.geom_type == "Polygon":
            out.append(np.asarray(g.exterior.coords))
            out += [np.asarray(r.coords) for r in g.interiors]
    return out


def lattice_coverage(geom, size, ss=SS, sub=4):
    """tiny-skia's anti-aliasing of `geom` at `ss`: each device pixel's coverage is the share
    of its `sub x sub` sample points, at the centres of the supersampled grid, that lie
    inside (a span covers supersampled pixels `round(x_left) .. round(x_right)` on each
    supersampled scanline, sampled at its centre: `scan/path_aa.rs`, SUPERSAMPLE_SHIFT 2)."""
    D, S = size * ss, size * ss * sub
    rings = _rings(geom)
    if not rings:
        return np.zeros((D, D))
    e = np.concatenate([np.stack([r[:-1], r[1:]], 1) for r in rings]) * (ss * sub)
    x0, y0, x1, y1 = e[:, 0, 0], e[:, 0, 1], e[:, 1, 0], e[:, 1, 1]
    ymin, ymax = np.minimum(y0, y1), np.maximum(y0, y1)
    cnt = np.zeros((D, S))
    for k in range(int(max(0, math.floor(ymin.min()))), int(min(S, math.ceil(ymax.max())))):
        yc = k + 0.5
        m = (ymin <= yc) & (ymax > yc)
        if not m.any():
            continue
        t = (yc - y0[m]) / (y1[m] - y0[m])
        xs = np.sort(x0[m] + t * (x1[m] - x0[m]))
        lo = np.clip(np.ceil(xs[0::2] - 0.5).astype(int), 0, S)
        hi = np.clip(np.ceil(xs[1::2] - 0.5).astype(int), 0, S)
        row = np.zeros(S + 1)
        np.add.at(row, lo, 1)
        np.add.at(row, hi, -1)
        cnt[k // sub] += np.cumsum(row)[:S]
    return cnt.reshape(D, D, sub).sum(-1) / (sub * sub)


def render_lattice(layers, size):
    """resvg's procedure with tiny-skia's sample lattice (curves flattened at `TOL_PX`, not
    by tiny-skia's own forward differencing; strokes by the exact offset, not its stroker)."""
    acc = np.zeros((size * SS, size * SS, 4))
    for reg, col, _, _ in layers:
        c = lattice_coverage(reg, size)
        acc = c[..., None] * np.array([*col, 1.0]) + (1 - c[..., None]) * acc
    return acc.reshape(size, SS, size, SS, 4).mean(axis=(1, 3))


def quantise_like_intake(pm):
    """Premultiplied RGBA -> straight 8-bit (as the corpus builder) -> premultiplied."""
    a = pm[..., 3:4]
    rgb = np.where(a > 1e-6, pm[..., :3] / np.maximum(a, 1e-6), 0.0)
    q = np.round(np.clip(np.concatenate([rgb, a], -1), 0, 1) * 255) / 255
    return np.concatenate([q[..., :3] * q[..., 3:4], q[..., 3:4]], -1)


def load_intake(path):
    s = np.asarray(Image.open(path).convert("RGBA"), dtype=np.float64) / 255
    return np.concatenate([s[..., :3] * s[..., 3:4], s[..., 3:4]], -1)


# -------------------------------------------------------------------------------- windows

def ink_maps(vis, covs, size):
    cols = {}
    for (v, col, _, _), c in zip(vis, covs):
        cols.setdefault(col, np.zeros((size, size)))
        cols[col] += c
    inks = [(0.0, 0.0, 0.0, 0.0)] + [(*k, 1.0) for k in cols]
    cov = [np.clip(1 - sum(cols.values()), 0, 1)] + list(cols.values())
    return np.array(inks), np.array(cov)


def windows(cov_ink, inks):
    """Two-ink pixels grouped into column and row windows; each such pixel in one window."""
    k, h, w = cov_ink.shape
    present = cov_ink > EPS_COV
    two = present.sum(0) == 2
    idx = np.argsort(~present, axis=0, kind="stable")[:2]  # the two present inks, ascending
    a_ink = np.minimum(idx[0], idx[1])
    b_ink = np.maximum(idx[0], idx[1])
    grads = np.array([np.gradient(cov_ink[i]) for i in range(k)])  # (k, 2, h, w)
    g = np.take_along_axis(grads, b_ink[None, None], 0)[0]
    gy, gx = g[0], g[1]
    colcls = np.abs(gy) >= np.abs(gx)
    wins = []
    for cls in (True, False):
        mask = two & (colcls == cls)
        lines = range(w) if cls else range(h)
        for li in lines:
            col = mask[:, li] if cls else mask[li, :]
            pa = a_ink[:, li] if cls else a_ink[li, :]
            pb = b_ink[:, li] if cls else b_ink[li, :]
            j = 0
            n = len(col)
            while j < n:
                if not col[j]:
                    j += 1
                    continue
                k0 = j
                while j + 1 < n and col[j + 1] and pa[j + 1] == pa[k0] and pb[j + 1] == pb[k0]:
                    j += 1
                pix = [(k, li) if cls else (li, k) for k in range(k0, j + 1)]
                wins.append({"col": cls, "line": li, "lo": k0, "pix": pix,
                             "pair": (int(pa[k0]), int(pb[k0]))})
                j += 1
    return wins, two


def project(res, wins, inks):
    """Per window, the residual's coverage of the pair's higher ink, summed."""
    out = np.zeros(len(wins))
    for t, wdw in enumerate(wins):
        a, b = wdw["pair"]
        d = inks[b] - inks[a]
        dd = float(d @ d)
        ys = np.array([p[0] for p in wdw["pix"]])
        xs = np.array([p[1] for p in wdw["pix"]])
        out[t] = float((res[ys, xs] @ d).sum()) / dd
    return out


def chains(wins):
    """Connected runs of windows (consecutive lines, same pair and axis, overlapping)."""
    key = {}
    for t, wdw in enumerate(wins):
        key.setdefault((wdw["col"], wdw["line"], wdw["pair"]), []).append(t)
    parent = list(range(len(wins)))

    def find(i):
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i
    nxt = {}
    for t, wdw in enumerate(wins):
        for u in key.get((wdw["col"], wdw["line"] + 1, wdw["pair"]), []):
            v = wins[u]
            if v["lo"] <= wdw["lo"] + len(wdw["pix"]) + 1 and wdw["lo"] <= v["lo"] + len(v["pix"]) + 1:
                parent[find(u)] = find(t)
                nxt.setdefault(t, u)
    return [find(t) for t in range(len(wins))], nxt


def fourth_difference(wins, nxt, obs, inks, cov_ink):
    """D over five consecutive windows of one run, from the intake alone: each window's mean
    boundary position, `lo + (the weight of the ink on the window's low side)`. The model is
    used only to say which ink that is."""
    size = obs.shape[0]

    def pos(t):
        wdw = wins[t]
        a, b = wdw["pair"]
        d = inks[b] - inks[a]
        ys = np.array([p[0] for p in wdw["pix"]])
        xs = np.array([p[1] for p in wdw["pix"]])
        wb = float((((obs[ys, xs] - inks[a]) @ d) / float(d @ d)).sum())
        y, x = wdw["pix"][0]
        y, x = (y - 1, x) if wdw["col"] else (y, x - 1)
        if y < 0 or x < 0 or y >= size or x >= size:
            return None
        low = int(np.argmax(cov_ink[:, y, x]))
        if low == b:
            return wdw["lo"] + wb
        if low == a:
            return wdw["lo"] + len(wdw["pix"]) - wb
        return None
    ds = []
    for t in range(len(wins)):
        seq = [t]
        while len(seq) < 5 and seq[-1] in nxt:
            seq.append(nxt[seq[-1]])
        if len(seq) < 5:
            continue
        hs = [pos(u) for u in seq]
        if any(v is None for v in hs):
            continue
        ds.append(hs[0] - 4 * hs[1] + 6 * hs[2] - 4 * hs[3] + hs[4])
    return np.array(ds)


# ------------------------------------------------------------------------------- the run

def measure(item, tier, fresh=False):
    from inkvec_bench import render  # noqa: F401
    size = int(re.match(r"\d+", tier).group(0))
    svg = (ROOT / "bench/data/corpus_svg" / item["corpus"] / f"{item['stem']}.svg").read_text()
    png = ROOT / "bench/data/corpus_raster" / item["corpus"] / tier / f"{item['stem']}.png"
    if not png.exists():
        return None
    lay_u = layers_of(svg, size, "usvg")
    lay_t = layers_of(svg, size, "true")
    if not lay_u:
        return None
    obs = load_intake(png)
    vis_u = visible_regions(lay_u)
    x_u, covs_u = render_partition(vis_u, size)
    x_t, _ = render_partition(visible_regions(lay_t), size)
    x_8 = render_layered(lay_u, size, SS)
    inks, cov_ink = ink_maps(vis_u, covs_u, size)
    wins, two = windows(cov_ink, inks)
    if not wins:
        return None
    r = {}
    r["q"] = project(quantise_like_intake(x_8) - x_8, wins, inks)
    r["true"] = project(obs - x_t, wins, inks)
    r["usvg"] = project(obs - x_u, wins, inks)
    r["as8"] = project(obs - x_8, wins, inks)
    r["seam"] = project(x_8 - x_u, wins, inks)
    if fresh:
        sys.path.insert(0, str(ROOT / "bench"))
        import build_corpus_v2 as B
        f = np.asarray(Image.open(io.BytesIO(B.render_supersampled(svg, size, SS))).convert("RGBA"),
                       dtype=np.float64) / 255
        fpm = np.concatenate([f[..., :3] * f[..., 3:4], f[..., 3:4]], -1)
        r["fresh"] = project(fpm - x_8, wins, inks)
        r["stale"] = float(np.abs(fpm - obs).max() * 255)
        x_l = render_lattice(lay_u, size)
        r["lattice"] = project(fpm - x_l, wins, inks)
    # tags
    curve = np.zeros((size, size), bool)
    stroke = np.zeros((size, size), bool)
    for reg, col, kind, cpts in lay_u:
        if cpts:
            p = np.asarray(cpts)
            ix = np.clip(np.floor(p[:, 0]).astype(int), 0, size - 1)
            iy = np.clip(np.floor(p[:, 1]).astype(int), 0, size - 1)
            curve[iy, ix] = True
        if kind == "stroke":
            for g in getattr(reg, "geoms", [reg]):
                for ring in [g.exterior, *g.interiors] if g.geom_type == "Polygon" else []:
                    q = np.asarray(ring.coords)
                    seg = np.linspace(0, 1, 4)[:, None, None]
                    pts = (q[:-1][None] * (1 - seg) + q[1:][None] * seg).reshape(-1, 2)
                    ix = np.clip(np.floor(pts[:, 0]).astype(int), 0, size - 1)
                    iy = np.clip(np.floor(pts[:, 1]).astype(int), 0, size - 1)
                    stroke[iy, ix] = True
    from scipy import ndimage
    curve = ndimage.binary_dilation(curve)
    r["curve"] = np.array([any(curve[y, x] for y, x in w["pix"]) for w in wins])
    r["stroke"] = np.array([any(stroke[y, x] for y, x in w["pix"]) for w in wins])
    r["n"] = np.array([len(w["pix"]) for w in wins])
    r["col"] = np.array([w["col"] for w in wins])
    ch, nxt = chains(wins)
    r["chain"] = np.array(ch)
    r["d4"] = fourth_difference(wins, nxt, obs, inks, cov_ink)
    # junction pixels: three or more inks; the compositing term there and overall
    multi = (cov_ink > EPS_COV).sum(0) >= 3
    dl = np.abs(x_8 - x_u).max(-1)
    r["junction_px"] = int(multi.sum())
    r["comp_junction"] = dl[multi]
    r["comp_two"] = dl[two]
    r["boundary_px"] = int(two.sum())
    return r


def chain_share(e, ch):
    """Share of the variance of `e` that is a constant per chain (one edge)."""
    groups = defaultdict(list)
    for v, c in zip(e, ch):
        groups[c].append(v)
    within = sum(((np.array(g) - np.mean(g)) ** 2).sum() for g in groups.values() if len(g) > 1)
    n_within = sum(len(g) - 1 for g in groups.values() if len(g) > 1)
    tot = float(((e - e.mean()) ** 2).sum()) / max(1, len(e) - 1)
    w = within / max(1, n_within)
    return max(0.0, 1 - w / tot) if tot > 0 else 0.0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--per-family", type=int, default=6)
    ap.add_argument("--tiers", default="128ss,512ss")
    ap.add_argument("--fresh", action="store_true", help="also score a fresh resvg render")
    ap.add_argument("--seed", type=int, default=1)
    a = ap.parse_args()
    import svgeval
    items = svgeval.load_sets()["screen"]
    rng = np.random.default_rng(a.seed)
    by_fam = defaultdict(list)
    for it in items:
        by_fam[it["corpus"]].append(it)
    pick = []
    for fam, its in sorted(by_fam.items()):
        its = [it for it in its if layers_of_ok(it)]
        rng.shuffle(its)
        pick += its[:a.per_family]
    for tier in a.tiers.split(","):
        acc = defaultdict(lambda: defaultdict(list))
        for it in pick:
            try:
                r = measure(it, tier, a.fresh)
            except Exception as ex:  # a file this cannot draw is skipped, and said so
                print(f"  skip {it['corpus']}/{it['stem']}: {type(ex).__name__}: {ex}", file=sys.stderr)
                continue
            if r is None:
                continue
            for k in ("q", "true", "usvg", "as8", "seam", "curve", "stroke", "n", "col", "d4",
                      "comp_junction", "comp_two"):
                acc[it["corpus"]][k].append(r[k])
            if "fresh" in r:
                acc[it["corpus"]]["fresh"].append(r["fresh"])
                acc[it["corpus"]]["lattice"].append(r["lattice"])
                acc[it["corpus"]]["stale"].append(np.array([r["stale"]]))
            acc[it["corpus"]]["chain"].append(r["chain"] + 10 ** 6 * len(acc[it["corpus"]]["chain"]))
            acc[it["corpus"]]["jpx"].append(np.array([r["junction_px"]]))
            acc[it["corpus"]]["bpx"].append(np.array([r["boundary_px"]]))
        report(tier, acc)


def layers_of_ok(it):
    s = (ROOT / "bench/data/corpus_svg" / it["corpus"] / f"{it['stem']}.svg").read_text()
    return not re.search(r"Gradient|<mask|clip-path|clipPath|<filter|<pattern|<image|<text|stroke-dash|<use|opacity", s)


def report(tier, acc):
    rms = lambda v: float(np.sqrt(np.mean(np.square(v)))) if len(v) else float("nan")
    print(f"== {tier}: window errors in coverage (px of mean boundary position per window)")
    print(f"{'family':15s} {'icons':>5s} {'windows':>7s} {'rms_q':>7s} {'true':>7s} {'usvg':>7s} {'as8':>7s} "
          f"{'floor':>7s} {'bias':>8s} {'chain':>6s} {'selfcal':>7s} {'curve':>7s} {'line':>7s} "
          f"{'stroke':>7s} {'fill-ln':>7s} {'seam':>7s} {'fresh':>7s} {'stale':>6s} {'lattice':>7s} "
          f"{'lat-ln':>7s} {'lat-cv':>7s} {'lat-st':>7s}")
    for fam, d in sorted(acc.items()):
        cat = {k: np.concatenate(v) for k, v in d.items()}
        q, t, u, s8, sm = cat["q"], cat["true"], cat["usvg"], cat["as8"], cat["seam"]
        floor = math.sqrt(max(0.0, rms(s8) ** 2 - rms(q) ** 2))
        d4 = cat["d4"]
        selfcal = ((np.median(np.abs(d4)) / 0.6745) ** 2 / 70) / (rms(s8) ** 2) if len(d4) > 10 else float("nan")
        cv, st = cat["curve"].astype(bool), cat["stroke"].astype(bool)
        fresh = rms(cat["fresh"]) if "fresh" in cat else float("nan")
        stale = float(cat["stale"].max()) if "stale" in cat else float("nan")
        fl = (~cv) & (~st)
        if "lattice" in cat:
            la = cat["lattice"]
            lat, lat_ln, lat_cv, lat_st = rms(la), rms(la[fl]), rms(la[cv]), rms(la[st])
        else:
            lat = lat_ln = lat_cv = lat_st = float("nan")
        print(f"{fam:15s} {len(d['q']):5d} {len(q):7d} {rms(q):7.4f} {rms(t):7.4f} {rms(u):7.4f} {rms(s8):7.4f} "
              f"{floor:7.4f} {s8.mean():+8.5f} {chain_share(s8, cat['chain']):6.2f} {selfcal:7.2f} "
              f"{rms(s8[cv]):7.4f} {rms(s8[~cv]):7.4f} {rms(s8[st]):7.4f} {rms(s8[fl]):7.4f} "
              f"{rms(sm):7.4f} {fresh:7.4f} {stale:6.0f} {lat:7.4f} {lat_ln:7.4f} {lat_cv:7.4f} {lat_st:7.4f}")
    print("compositing term (max over channels of |as8 - usvg|, colour units):")
    for fam, d in sorted(acc.items()):
        cj = np.concatenate(d["comp_junction"]) if d["comp_junction"] else np.zeros(0)
        ct = np.concatenate(d["comp_two"]) if d["comp_two"] else np.zeros(0)
        jpx = int(np.concatenate(d["jpx"]).sum())
        bpx = int(np.concatenate(d["bpx"]).sum())
        q99 = lambda v: float(np.quantile(v, 0.99)) if len(v) else float("nan")
        print(f"  {fam:15s} junction px {jpx:6d}: mean {cj.mean() if len(cj) else float('nan'):.4f} "
              f"p99 {q99(cj):.4f} max {cj.max() if len(cj) else float('nan'):.4f}   "
              f"two-ink px {bpx:6d}: mean {ct.mean() if len(ct) else float('nan'):.5f} p99 {q99(ct):.4f} "
              f"share > 1/255: {float((ct > 1 / 255).mean()) if len(ct) else float('nan'):.3f}")


if __name__ == "__main__":
    main()
