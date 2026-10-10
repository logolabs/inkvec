"""Checks of the boundary chain's variance model and precision formulas on exact renders.

`docs/theory/chain-boundary.md` states, informally, what the pixels' area measurements are
worth: how much a window sum varies under 8-bit quantisation and under n x n point
supersampling, how much information column sums keep against per-pixel terms, how precisely
two arms place a corner's vertex, and how small a fillet the pixels can tell from a sharp
corner. Each subcommand measures one of those claims against exactly known geometry.

* `calib`   column- and row-window sums of discs against the exact areas in the same windows:
            residual spread against `n_partial / (255^2 * 12)` (8-bit) and `1 / (12 n^3)`
            (point supersampling), by angle to the scan axis, and its lag-1 correlation.
* `info`    Fisher information about a straight edge's normal offset, per-pixel terms against
            column sums, with equal noise on every partial pixel: predicted ratio
            `(1 - t/2) / (1 - t/3)`, `t = |tan|`, never below 3/4.
* `vertex`  a wedge's vertex as the intersection of two arms fitted by column-sum least
            squares: RMS error against `sqrt(2 V_eff (1 + 12 s_m^2 / L^2) / L) / sin(alpha)`.
* `fillet`  per-pixel chi^2 between a sharp corner and a fillet of radius r, against the
            one-pixel bound `r = sqrt(3 sigma / (tan(tau/2) - tau/2))` for chi^2 = 9.

    python3 bench/theory/evidence_eval.py calib|info|vertex|fillet|all

Coordinates as in `exact_raster.py`: pixel (i, j) is [i, i+1] x [j, j+1], y down.
"""

from __future__ import annotations

import math
import sys

import numpy as np

import exact_raster as er

SIGMA_Q = 1.0 / (255.0 * math.sqrt(12.0))  # 8-bit quantisation, one channel (or a grey axis)


def q8(a: np.ndarray) -> np.ndarray:
    return np.round(a * 255.0) / 255.0


# ---------------------------------------------------------------------------------- calib

def _ss_disc(size: int, cx: float, cy: float, r: float, n: int) -> np.ndarray:
    s = (np.arange(size * n) + 0.5) / n
    x, y = np.meshgrid(s, s)
    inside = ((x - cx) ** 2 + (y - cy) ** 2 < r * r).astype(np.float64)
    return inside.reshape(size, n, size, n).mean(axis=(1, 3))


def _windows(obs: np.ndarray, exact: np.ndarray, cx: float, cy: float) -> list[tuple]:
    """Column windows where the boundary is within 45 degrees of horizontal, row windows
    elsewhere, each from a pure pixel to a pure pixel of the other kind. Returns
    (observed sum - exact sum, partial pixels, angle to the scan axis, polar angle)."""
    eps = 1e-9
    out = []
    for transpose in (False, True):
        a = obs.T if transpose else obs
        e = exact.T if transpose else exact
        h, w = a.shape
        for i in range(w):
            col = a[:, i]
            sat = np.where(col >= 1 - eps, 1, np.where(col <= eps, 0, -1))
            j = 0
            while j < h:
                if sat[j] == -1:
                    j += 1
                    continue
                k = j + 1
                while k < h and sat[k] == -1:
                    k += 1
                if k >= h:
                    break
                if sat[k] != sat[j] and k - j >= 2:
                    xm, ym = i + 0.5, 0.5 * (j + k) + 0.5
                    px, py = (ym, xm) if transpose else (xm, ym)
                    nx, ny = px - cx, py - cy
                    a_n, b_n = (nx, ny) if not transpose else (ny, nx)
                    if abs(b_n) >= abs(a_n):
                        seg = e[j:k + 1, i]
                        npart = int(((seg > eps) & (seg < 1 - eps)).sum())
                        out.append((col[j:k + 1].sum() - seg.sum(), npart,
                                    math.atan2(abs(a_n), abs(b_n)),
                                    math.atan2(py - cy, px - cx)))
                j = k
    return out


def calib(trials: int = 16, size: int = 64) -> None:
    rng = np.random.default_rng(7)
    print(f"8-bit sigma_q = 1/(255 sqrt 12) = {SIGMA_Q:.5f}")
    conds = [("exact float", None, False), ("exact + 8-bit", None, True)]
    conds += [(f"ss{n} + 8-bit", n, True) for n in (8, 16, 32)]
    for name, n, quant in conds:
        res = []
        for _ in range(trials):
            r = rng.uniform(6, 26)
            cx, cy = rng.uniform(r + 3, size - r - 3, 2)
            exact = er.rasterize([er.circle(cx, cy, r)], size, size)
            obs = exact if n is None else _ss_disc(size, cx, cy, r, n)
            if quant:
                obs = q8(obs)
            res.append(_windows(obs, exact, cx, cy))
        rr = np.array([w[0] for ws in res for w in ws])
        npart = np.array([w[1] for ws in res for w in ws])
        ang = np.array([w[2] for ws in res for w in ws])
        cors = []
        for ws in res:
            seq = np.array([w[0] for w in sorted(ws, key=lambda w: w[3])])
            if len(seq) > 10 and seq.std() > 0:
                cors.append(np.corrcoef(seq[:-1], seq[1:])[0, 1])
        pred = math.sqrt(npart.mean() * SIGMA_Q ** 2 * quant + (1 / (12 * n ** 3) if n else 0.0))
        lag = f"{np.mean(cors):+.3f}" if cors else "n/a"
        print(f"{name:14s} windows {len(rr):5d}  std {rr.std():.5f}  predicted {pred:.5f}  "
              f"rms/sqrt(partial) {np.sqrt((rr ** 2 / np.maximum(npart, 1)).mean()):.5f}  "
              f"lag-1 corr {lag}")
        if n:
            for lo, hi in [(0, 0.02), (0.02, 0.13), (0.13, 0.4), (0.4, 0.8)]:
                m = (ang >= lo) & (ang < hi)
                if m.sum() > 10:
                    print(f"      angle {lo:.2f}-{hi:.2f} rad: n {m.sum():4d}  mean {rr[m].mean():+.5f}"
                          f"  std {rr[m].std():.5f}   (near-axis bound 1/(2n) = {1 / (2 * n):.4f})")


# ----------------------------------------------------------------------------------- info

def info(trials: int = 24, size: int = 48) -> None:
    rng = np.random.default_rng(11)
    h = 1e-5

    def cov(theta: float, off: float) -> np.ndarray:
        nrm = np.array([-math.sin(theta), math.cos(theta)])
        p = np.array([size / 2, size / 2]) + off * nrm
        return er.rasterize([er.half_plane(size, size, p[0], p[1], theta)], size, size)

    print("information about the normal offset kept by column sums (equal noise per partial pixel)")
    for deg in (0, 10, 20, 30, 40):
        th = math.radians(deg) + 1e-3
        pix = col = 0.0
        for _ in range(trials):
            off = rng.uniform(0, 1)
            a0 = cov(th, off)[8:-8, 8:-8]
            g = (cov(th, off + h)[8:-8, 8:-8] - a0) / h
            part = (a0 > 1e-9) & (a0 < 1 - 1e-9)
            pix += (g[part] ** 2).sum()
            for i in range(g.shape[1]):
                m = part[:, i]
                if m.any():
                    col += g[m, i].sum() ** 2 / m.sum()
        t = math.tan(th)
        print(f"  {deg:2d} deg: measured {col / pix:.3f}  predicted {(1 - t / 2) / (1 - t / 3):.3f}")


# --------------------------------------------------------------------------------- vertex

def _wedge(size: int, v, u1, u2) -> np.ndarray:
    """The wedge {v + s u1 + t u2 : s, t >= 0} clipped to the frame (Sutherland-Hodgman)."""
    poly = [np.array(p, dtype=float) for p in [(0, 0), (size, 0), (size, size), (0, size)]]
    tests = (lambda p: u1[0] * (p[1] - v[1]) - u1[1] * (p[0] - v[0]),
             lambda p: (p[0] - v[0]) * u2[1] - (p[1] - v[1]) * u2[0])
    for f in tests:
        out = []
        for k in range(len(poly)):
            a, b = poly[k], poly[(k + 1) % len(poly)]
            fa, fb = f(a), f(b)
            if fa >= 0:
                out.append(a)
            if (fa >= 0) != (fb >= 0):
                out.append(a + fa / (fa - fb) * (b - a))
        poly = out
    return np.array(poly)


def _ss_convex(size: int, poly: np.ndarray, n: int) -> np.ndarray:
    s = (np.arange(size * n) + 0.5) / n
    x, y = np.meshgrid(s, s)
    p = np.vstack([poly, poly[:1]])
    sgn = np.sign(sum(p[k, 0] * p[k + 1, 1] - p[k + 1, 0] * p[k, 1] for k in range(len(poly))))
    inside = np.ones_like(x, dtype=bool)
    for k in range(len(poly)):
        (ax, ay), (bx, by) = p[k], p[k + 1]
        inside &= sgn * ((bx - ax) * (y - ay) - (by - ay) * (x - ax)) > 0
    return inside.reshape(size, n, size, n).mean(axis=(1, 3))


def _arm_line(a: np.ndarray, v, u, gap: float, length: float, size: int):
    """The arm's line from column-sum least squares over the windows at arclength
    [gap, gap + length] (rows when the arm is steeper than 45 degrees); n . p = d."""
    transpose = abs(u[1]) > abs(u[0])
    img = a.T if transpose else a
    uu, vv = (u[::-1], v[::-1]) if transpose else (u, v)
    x_lo, x_hi = sorted((vv[0] + gap * uu[0], vv[0] + (gap + length) * uu[0]))
    rows, rhs = [], []
    for i in range(int(math.floor(x_lo)), int(math.ceil(x_hi))):
        xc = i + 0.5
        sc = (xc - vv[0]) / uu[0]
        if not gap <= sc <= gap + length:
            continue
        jc = int(math.floor(vv[1] + sc * uu[1]))
        j0, j1 = jc - 1, jc + 1
        while j0 > 0 and 1e-9 < img[j0, i] < 1 - 1e-9:
            j0 -= 1
        while j1 < size - 1 and 1e-9 < img[j1, i] < 1 - 1e-9:
            j1 += 1
        w = img[j0:j1 + 1, i]
        pure = [q <= 1e-9 or q >= 1 - 1e-9 for q in (w[0], w[-1])]
        if not all(pure) or (w[0] > 0.5) == (w[-1] > 0.5):
            continue
        # the top face's area in the window is the boundary's column mean below j0
        top = w if w[0] > 0.5 else 1.0 - w
        rows.append([1.0, xc])
        rhs.append(j0 + top.sum())
    c, b = np.linalg.lstsq(np.array(rows), np.array(rhs), rcond=None)[0]
    return (np.array([1.0, -b]), c) if transpose else (np.array([-b, 1.0]), c)


def vertex(trials: int = 30, size: int = 48) -> None:
    rng = np.random.default_rng(5)
    cos_mean = 0.90  # mean cos of the angle to the scan axis over random orientations
    cases = (("exact + 8-bit", lambda p: q8(er.rasterize([p], size, size)), 1.42 * SIGMA_Q ** 2),
             ("ss8 + 8-bit", lambda p: q8(_ss_convex(size, p, 8)), 1 / (12 * 8 ** 3) + 1.42 * SIGMA_Q ** 2))
    print("vertex of a wedge from two arms of length L (column-sum least squares)")
    for name, render, v_col in cases:
        for deg in (30, 60, 90, 120, 150):
            al = math.radians(deg)
            gap = (2.0 / math.sin(al) if al < math.pi / 2 else 2.0) + 1.0
            for length in (6.0, 12.0):
                err = []
                for _ in range(trials):
                    rot = rng.uniform(0, 2 * math.pi)
                    v = np.array([size / 2, size / 2]) + rng.uniform(-0.5, 0.5, 2)
                    u1 = np.array([math.cos(rot), math.sin(rot)])
                    u2 = np.array([math.cos(rot + al), math.sin(rot + al)])
                    a = render(_wedge(size, v, u1, u2))
                    (n1, d1), (n2, d2) = (_arm_line(a, v, u, gap, length, size) for u in (u1, u2))
                    est = np.linalg.solve(np.array([n1, n2]), np.array([d1, d2]))
                    err.append(float(np.linalg.norm(est - v)))
                s_m = gap + length / 2
                pred = math.sqrt(2 * v_col * cos_mean * (1 + 12 * s_m ** 2 / length ** 2) / length) / math.sin(al)
                print(f"  {name:13s} alpha {deg:3d}  L {length:4.1f}: rms {math.sqrt(np.mean(np.square(err))):.4f} px"
                      f"   predicted {pred:.4f} px")


# --------------------------------------------------------------------------------- fillet

def _corner(v, rot: float, tau: float, r: float, n_arc: int = 400) -> np.ndarray:
    d_in = np.array([math.cos(rot), math.sin(rot)])
    d_out = np.array([math.cos(rot + tau), math.sin(rot + tau)])
    reach = 14.0
    if r == 0:
        path = [v - reach * d_in, v, v + reach * d_out]
    else:
        p0 = v - r * math.tan(tau / 2) * d_in
        c = p0 + r * np.array([-d_in[1], d_in[0]])
        a0 = math.atan2(p0[1] - c[1], p0[0] - c[0])
        arc = [c + r * np.array([math.cos(a0 + k * tau / n_arc), math.sin(a0 + k * tau / n_arc)])
               for k in range(n_arc + 1)]
        path = [v - reach * d_in] + arc + [v + reach * d_out]
    far = v + 0.5 * reach * (np.array([-d_in[1], d_in[0]]) + np.array([-d_out[1], d_out[0]]))
    return np.array(path + [far])


def fillet(trials: int = 8, size: int = 40) -> None:
    rng = np.random.default_rng(3)
    print("chi^2 between a sharp corner and a fillet of radius r (per-pixel, equal noise)")
    for deg in (90, 60, 30):
        tau = math.radians(deg)
        k = math.tan(tau / 2) - tau / 2
        for name, sg in (("8-bit", SIGMA_Q), ("8x8 ss", 1 / math.sqrt(12 * 8 ** 3))):
            cells = []
            for r in (0.1, 0.2, 0.4, 0.8, 1.6, 3.2):
                tot = 0.0
                for _ in range(trials):
                    v = np.array([size / 2, size / 2]) + rng.uniform(-0.5, 0.5, 2)
                    rot = rng.uniform(0, 2 * math.pi)
                    a0 = er.rasterize([_corner(v, rot, tau, 0.0)], size, size)
                    a1 = er.rasterize([_corner(v, rot, tau, r)], size, size)
                    tot += ((a1 - a0) ** 2).sum() / sg ** 2
                cells.append(f"r={r}: {tot / trials:9.1f}")
            print(f"  turn {deg:2d} {name:7s} bound r = {math.sqrt(3 * sg / k):.2f}   " + "  ".join(cells))


if __name__ == "__main__":
    what = sys.argv[1] if len(sys.argv) > 1 else "all"
    for name, fn in (("calib", calib), ("info", info), ("vertex", vertex), ("fillet", fillet)):
        if what in (name, "all"):
            fn()
