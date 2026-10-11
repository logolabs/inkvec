"""Exact box-filter (area-coverage) rasteriser for polygons, in float64.

A port of the signed-area accumulation rasteriser of R. Levien's font-rs
(https://github.com/raphlinus/font-rs, `accumulate`/`draw_line`), the same family as
FreeType's smooth rasteriser and inkvec's own `boundary_opt/band.rs`: each line segment
deposits, into every cell it crosses, the signed area between itself and the cell's right
side, plus a carried cover; a prefix sum along each row yields the exact area of the polygon
inside every pixel. Pixel (i, j) is the square [i, i+1] x [j, j+1] (x right, y down).

Used by `strip_eval.py` to render shapes whose geometry is known exactly, so estimator
errors are the estimators' own and not the renderer's.
"""

from __future__ import annotations

import math

import numpy as np


def _draw_line(acc: np.ndarray, x0: float, y0: float, x1: float, y1: float) -> None:
    h, wp = acc.shape
    if y0 == y1:
        return
    if y0 < y1:
        d_sign = 1.0
    else:
        d_sign = -1.0
        x0, y0, x1, y1 = x1, y1, x0, y0
    dxdy = (x1 - x0) / (y1 - y0)
    x = x0
    ystart = max(int(math.floor(y0)), 0)
    if y0 < 0.0:
        x -= y0 * dxdy
    yend = min(h, int(math.ceil(y1)))
    for y in range(ystart, yend):
        dy = min(y + 1.0, y1) - max(float(y), y0)
        xnext = x + dxdy * dy
        d = dy * d_sign
        xa, xb = (x, xnext) if x < xnext else (xnext, x)
        xa_floor = math.floor(xa)
        xai = int(xa_floor)
        xb_ceil = math.ceil(xb)
        xbi = int(xb_ceil)
        row = acc[y]
        if xbi <= xai + 1:
            xmf = 0.5 * (x + xnext) - xa_floor
            row[xai] += d - d * xmf
            row[xai + 1] += d * xmf
        else:
            s = 1.0 / (xb - xa)
            xaf = xa - xa_floor
            a0 = 0.5 * s * (1.0 - xaf) * (1.0 - xaf)
            xbf = xb - xb_ceil + 1.0
            am = 0.5 * s * xbf * xbf
            row[xai] += d * a0
            if xbi == xai + 2:
                row[xai + 1] += d * (1.0 - a0 - am)
            else:
                a1 = s * (1.5 - xaf)
                row[xai + 1] += d * (a1 - a0)
                for xi in range(xai + 2, xbi - 1):
                    row[xi] += d * s
                a2 = a1 + (xbi - xai - 3) * s
                row[xbi - 1] += d * (1.0 - a2 - am)
            row[xbi] += d * am
        x = xnext


def rasterize(polygons: list[np.ndarray], width: int, height: int) -> np.ndarray:
    """Exact area coverage in [0, 1] of the union of closed polygons (non-zero winding, the
    polygons must not overlap), as a (height, width) float64 array. Each polygon is an (n, 2)
    array of (x, y) vertices; it is closed implicitly. Geometry must lie inside
    [0, width] x [0, height]."""
    acc = np.zeros((height, width + 2), dtype=np.float64)
    for poly in polygons:
        n = len(poly)
        for k in range(n):
            xa, ya = poly[k]
            xb, yb = poly[(k + 1) % n]
            _draw_line(acc, float(xa), float(ya), float(xb), float(yb))
    cov = np.abs(np.cumsum(acc, axis=1))[:, :width]
    return np.clip(cov, 0.0, 1.0)


def circle(cx: float, cy: float, r: float, n: int = 8192) -> np.ndarray:
    t = np.linspace(0.0, 2.0 * math.pi, n, endpoint=False)
    # Vertices on a circle of radius r / cos(pi/n) would bound the same area as the disc;
    # the area-matched radius keeps the polygon's coverage within 1e-7 of the disc's.
    rr = r * math.sqrt((2.0 * math.pi / n) / math.sin(2.0 * math.pi / n))
    return np.stack([cx + rr * np.cos(t), cy + rr * np.sin(t)], axis=1)


def ellipse(cx: float, cy: float, a: float, b: float, phi: float, n: int = 8192) -> np.ndarray:
    t = np.linspace(0.0, 2.0 * math.pi, n, endpoint=False)
    k = math.sqrt((2.0 * math.pi / n) / math.sin(2.0 * math.pi / n))
    x = a * k * np.cos(t)
    y = b * k * np.sin(t)
    c, s = math.cos(phi), math.sin(phi)
    return np.stack([cx + c * x - s * y, cy + s * x + c * y], axis=1)


def half_plane(width: int, height: int, px: float, py: float, theta: float) -> np.ndarray:
    """The part of the frame on the left of the directed line through (px, py) at angle
    theta, as a polygon (clipped to the frame)."""
    d = np.array([math.cos(theta), math.sin(theta)])
    nrm = np.array([-d[1], d[0]])
    frame = [np.array(v, dtype=float) for v in
             [(0, 0), (width, 0), (width, height), (0, height)]]
    out = []
    p = np.array([px, py])
    m = len(frame)
    for k in range(m):
        a, b = frame[k], frame[(k + 1) % m]
        sa, sb = np.dot(a - p, nrm), np.dot(b - p, nrm)
        if sa >= 0:
            out.append(a)
        if (sa >= 0) != (sb >= 0):
            t = sa / (sa - sb)
            out.append(a + t * (b - a))
    return np.array(out)


def ribbon(width: int, height: int, px: float, py: float, theta: float, w: float) -> np.ndarray:
    """A straight stroke of width w centred on the line through (px, py) at angle theta,
    clipped to the frame."""
    d = np.array([math.cos(theta), math.sin(theta)])
    nrm = np.array([-d[1], d[0]])
    poly = [np.array(v, dtype=float) for v in
            [(0, 0), (width, 0), (width, height), (0, height)]]
    p = np.array([px, py])
    for sign, off in ((1.0, -0.5 * w), (-1.0, -0.5 * w)):
        q = p + sign * (-off) * nrm
        out = []
        m = len(poly)
        for k in range(m):
            a, b = poly[k], poly[(k + 1) % m]
            sa = sign * np.dot(a - q, nrm)
            sb = sign * np.dot(b - q, nrm)
            if sa <= 0:
                out.append(a)
            if (sa <= 0) != (sb <= 0):
                t = sa / (sa - sb)
                out.append(a + t * (b - a))
        poly = out
    return np.array(poly)


def _self_check() -> None:
    # An axis-aligned rectangle: every pixel's area is a product of overlaps.
    x0, x1, y0, y1 = 2.3, 7.1, 3.7, 9.25
    cov = rasterize([np.array([(x0, y0), (x1, y0), (x1, y1), (x0, y1)])], 12, 12)
    for j in range(12):
        for i in range(12):
            ox = max(0.0, min(i + 1, x1) - max(i, x0))
            oy = max(0.0, min(j + 1, y1) - max(j, y0))
            assert abs(cov[j, i] - ox * oy) < 1e-12, (i, j, cov[j, i], ox * oy)
    # A triangle: total area, and one pixel against a hand computation.
    tri = np.array([(1.0, 1.0), (9.0, 1.0), (1.0, 9.0)])
    cov = rasterize([tri], 12, 12)
    assert abs(cov.sum() - 32.0) < 1e-12
    # Pixel [4,5] x [5,6] is cut corner to corner by x + y = 10; [4,5]^2 is inside.
    assert abs(cov[5, 4] - 0.5) < 1e-12 and abs(cov[4, 4] - 1.0) < 1e-12
    # A disc: the area-matched polygon has the disc's area.
    cov = rasterize([circle(16.3, 15.7, 9.4)], 32, 32)
    assert abs(cov.sum() - math.pi * 9.4 ** 2) < 1e-6, cov.sum() - math.pi * 9.4 ** 2


if __name__ == "__main__":
    _self_check()
    print("exact_raster: self-check passed")
