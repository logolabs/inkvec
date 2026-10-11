"""Boundary-point estimators from box-filtered coverage, scored against exact geometry.

The estimators, each reading a two-tone coverage image `a` (1 = ink) and returning boundary
points:

* `levelset`: the 1/2 crossing of the linear interpolation between pixel centres, along
  columns (and rows), the reading of `contour.rs` and `planar::root_find_half`.
* `strip_mean`: the column-sum theorem (`InkvecTheory.column_sum_eq_average`): the sum of a
  column's coverages between saturated pixels is the column mean of the boundary height.
* `strip`: the same with the cell-mean-to-point correction
  (`InkvecTheory.boundary_point_from_pixels`), exact for cubic boundaries.

Columns are scanned for boundaries flatter than 45 degrees, rows for steeper ones (the
axis along which the boundary is a graph with slope at most 1).

Run `python3 strip_eval.py` for the synthetic comparison; `inkvec_compare.py` adds inkvec's
own sub-pixel stage to it.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np

import exact_raster as er


@dataclass
class Crossing:
    """One boundary crossing of a scan line (a column, or a row)."""

    line: int  # column (or row) index
    lo: int  # first partial pixel along the line (or the pixel after the border)
    hi: int  # last partial pixel (lo - 1 when the boundary sits on a pixel border)
    ink_first: bool  # ink on the low-index side
    mean: float  # column mean of the boundary position (column-sum theorem)
    half: float  # the 1/2 crossing of the interpolated coverage


def scan(a: np.ndarray, eps: float) -> list[Crossing]:
    """Crossings of every column of `a` (rows are the scan direction: a[j, i], j varies).

    A crossing is a run of partial pixels flanked by saturated pixels of opposite kinds.
    """
    h, w = a.shape
    out: list[Crossing] = []
    for i in range(w):
        col = a[:, i]
        sat = np.where(col >= 1.0 - eps, 1, np.where(col <= eps, 0, -1))
        j = 0
        while j < h:
            if sat[j] == -1:
                j += 1
                continue
            # saturated pixel j; find the next saturated pixel k > j
            k = j + 1
            while k < h and sat[k] == -1:
                k += 1
            if k >= h:
                break
            if sat[k] != sat[j]:
                lo, hi = j + 1, k - 1
                ink_first = sat[j] == 1
                # The window runs from flank to flank: the column-sum theorem holds for any
                # window that contains the crossing, and the flanks' own (noisy) values keep
                # the sum unbiased where a near-saturated pixel was classed as saturated.
                part = col[j:k + 1]
                if ink_first:
                    mean = j + float(part.sum())
                else:
                    mean = j + float((1.0 - part).sum())
                # 1/2 crossing of the linear interpolation between centres j+1/2 .. k+1/2
                seg = col[j:k + 1]
                half = math.nan
                for t in range(len(seg) - 1):
                    u, v = seg[t] - 0.5, seg[t + 1] - 0.5
                    if u * v <= 0.0 and seg[t] != seg[t + 1]:
                        half = (j + t + 0.5) + (seg[t] - 0.5) / (seg[t] - seg[t + 1])
                        break
                out.append(Crossing(i, lo, hi, ink_first, mean, half))
            j = k
    return out


def _neighbours(cr: list[Crossing]) -> dict[tuple[int, int], Crossing]:
    return {(c.line, k): c for k, c in enumerate(cr)}


def points_from_scan(cr: list[Crossing], transpose: bool) -> dict[str, list[tuple[float, float]]]:
    """Boundary points from one scan direction, by each estimator. A crossing is used only
    when its neighbours on both adjacent lines exist, have the same orientation and are
    within one pixel (slope at most 1 along this axis)."""
    by_line: dict[int, list[Crossing]] = {}
    for c in cr:
        by_line.setdefault(c.line, []).append(c)

    def match(c: Crossing, line: int) -> Crossing | None:
        best = None
        for d in by_line.get(line, []):
            if d.ink_first == c.ink_first and abs(d.mean - c.mean) <= 1.0:
                if best is None or abs(d.mean - c.mean) < abs(best.mean - c.mean):
                    best = d
        return best

    pts: dict[str, list[tuple[float, float]]] = {"levelset": [], "strip_mean": [], "strip": []}
    for c in cr:
        l, r = match(c, c.line - 1), match(c, c.line + 1)
        if l is None or r is None:
            continue
        x = c.line + 0.5
        y_mean = c.mean
        y_pt = c.mean - (r.mean - 2.0 * c.mean + l.mean) / 24.0
        cand = {"levelset": c.half, "strip_mean": y_mean, "strip": y_pt}
        for k, y in cand.items():
            if math.isnan(y):
                continue
            pts[k].append((y, x) if transpose else (x, y))
    return pts


def estimate(a: np.ndarray, eps: float = 0.02) -> dict[str, np.ndarray]:
    """Boundary points (x, y) by every estimator, from column and row scans."""
    out: dict[str, list] = {"levelset": [], "strip_mean": [], "strip": []}
    for transpose in (False, True):
        img = a.T if transpose else a
        p = points_from_scan(scan(img, eps), transpose)
        for k in out:
            out[k].extend(p[k])
    return {k: np.array(v) for k, v in out.items()}


def dist_to_polygon(pts: np.ndarray, poly: np.ndarray) -> np.ndarray:
    """Unsigned distance from each point to the closed polygon's boundary."""
    a = poly
    b = np.roll(poly, -1, axis=0)
    ab = b - a
    ab2 = (ab * ab).sum(1)
    d = np.full(len(pts), np.inf)
    for s in range(0, len(pts), 256):
        p = pts[s:s + 256, None, :]
        t = np.clip(((p - a[None]) * ab[None]).sum(2) / ab2[None], 0.0, 1.0)
        q = a[None] + t[..., None] * ab[None]
        d[s:s + 256] = np.sqrt(((p - q) ** 2).sum(2)).min(1)
    return d


def add_noise(a: np.ndarray, sigma: float, rng: np.random.Generator, quantize: bool) -> np.ndarray:
    b = a + rng.normal(0.0, sigma, a.shape) if sigma > 0 else a.copy()
    if quantize:
        b = np.round(np.clip(b, 0.0, 1.0) * 255.0) / 255.0
    return np.clip(b, 0.0, 1.0)


def summarize(d: np.ndarray) -> str:
    if len(d) == 0:
        return "n=0"
    return (f"n={len(d):5d} mean={d.mean():.4f} rms={math.sqrt((d * d).mean()):.4f} "
            f"p99={np.quantile(d, 0.99):.4f} max={d.max():.4f}")


def shapes(rng: np.random.Generator, size: int = 64) -> list[tuple[str, np.ndarray]]:
    out = []
    for _ in range(12):
        r = rng.uniform(4.0, 24.0)
        cx, cy = rng.uniform(r + 4, size - r - 4, 2) if r < size / 2 - 4 else (size / 2, size / 2)
        out.append((f"circle r={r:.1f}", er.circle(cx, cy, r)))
    for _ in range(8):
        a = rng.uniform(6.0, 24.0)
        b = rng.uniform(4.0, a)
        out.append((f"ellipse {a:.1f}x{b:.1f}",
                    er.ellipse(size / 2 + rng.uniform(-2, 2), size / 2 + rng.uniform(-2, 2), a, b,
                               rng.uniform(0, math.pi))))
    for _ in range(8):
        th = rng.uniform(0, math.pi)
        out.append((f"edge th={math.degrees(th):.0f}",
                    er.half_plane(size, size, size / 2 + rng.uniform(-1, 1),
                                  size / 2 + rng.uniform(-1, 1), th)))
    return out


def edge_only(poly: np.ndarray, size: int) -> np.ndarray:
    """For a half-plane clipped to the frame, the true boundary excludes the frame sides:
    keep the polygon (the distance to the frame never wins far from it)."""
    return poly


def main() -> None:
    rng = np.random.default_rng(20261009)
    size = 64
    conds = [("noiseless, float", 0.0, False), ("8-bit", 0.0, True),
             ("8-bit + noise 1/255", 1.0 / 255, True), ("8-bit + noise 4/255", 4.0 / 255, True)]
    sh = shapes(rng, size)
    for cname, sigma, quant in conds:
        acc: dict[str, list[np.ndarray]] = {}
        for name, poly in sh:
            a = er.rasterize([poly], size, size)
            b = add_noise(a, sigma, rng, quant)
            eps = max(0.02, 4.0 * sigma)
            pts = estimate(b, eps)
            for k, p in pts.items():
                if len(p) == 0:
                    continue
                d = dist_to_polygon(p, poly)
                # half-planes: drop points within 2 px of the frame (the frame is not a boundary)
                if name.startswith("edge"):
                    keep = (p[:, 0] > 2) & (p[:, 0] < size - 2) & (p[:, 1] > 2) & (p[:, 1] < size - 2)
                    d = d[keep]
                acc.setdefault(k, []).append(d)
        print(f"== {cname}")
        for k in ("levelset", "strip_mean", "strip"):
            print(f"  {k:11s} {summarize(np.concatenate(acc[k]))}")


if __name__ == "__main__":
    main()
