"""Where does the colour error come from? Attribute each icon's dE00 to its sources.

The gate's dE00 is the mean CIEDE2000 over every pixel of the trace and the artist's file,
both rendered at 1024 px. Every pixel's error is assigned to one bin by where it lies in the
artist's render:

* `edge`     within 1 source pixel of an edge of the artist's render: boundary placement
* `near`     1-3 source pixels from an edge: boundary shape (rounded corners, bulges),
             slivers, and the anti-aliasing a fitted curve paints differently
* `flat`     further from any edge, where the artist's render is flat: fill colour, and
             features the trace added or lost
* `shaded`   further from any edge, where the artist's render varies smoothly: gradients

The bins' contributions sum to the icon's dE00. A large connected patch of interior error
(at least `BLOB_PX` source pixels above `BLOB_DE`) is counted separately as `blob`: a missing
or extra feature, not a colour that is slightly off.

    python3 bench/theory/attribution.py --exe target/release/inkvec --tier 512ss [--mode fast]
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))

EDGE_DE = 2.0  # neighbour difference that marks an edge of the artist's render
SHADE_DE = 0.25  # neighbour difference above which an interior pixel is "shaded"
BLOB_DE = 3.0
BLOB_PX = 2.0  # source pixels (area)


def de_map(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    from skimage.color import deltaE_ciede2000, rgb2lab
    return deltaE_ciede2000(rgb2lab(np.clip(a, 0, 1)), rgb2lab(np.clip(b, 0, 1)))


def neighbour_de(lab: np.ndarray) -> np.ndarray:
    """Largest CIEDE2000 between each pixel and its four neighbours."""
    from skimage.color import deltaE_ciede2000
    out = np.zeros(lab.shape[:2])
    for dy, dx in ((0, 1), (1, 0)):
        a = lab[: lab.shape[0] - dy, : lab.shape[1] - dx]
        b = lab[dy:, dx:]
        d = deltaE_ciede2000(a, b)
        out[: lab.shape[0] - dy, : lab.shape[1] - dx] = np.maximum(
            out[: lab.shape[0] - dy, : lab.shape[1] - dx], d)
        out[dy:, dx:] = np.maximum(out[dy:, dx:], d)
    return out


def edge_structure(lab: np.ndarray, nde: np.ndarray, scale: float) -> np.ndarray:
    """Per pixel: 'j' (junction), 'c' (corner), 's' (smooth edge) or ' ' (no edge in reach)."""
    from scipy import ndimage
    flat = nde < 0.5
    regions, _ = ndimage.label(flat)
    r = max(1, int(round(1.5 * scale)))
    near_edge = ndimage.distance_transform_edt(~(nde > EDGE_DE)) <= scale
    ys, xs = np.nonzero(near_edge)
    h, w = nde.shape
    offs = [(dy, dx) for dy in range(-r, r + 1, max(1, r // 3)) for dx in range(-r, r + 1, max(1, r // 3))
            if dy * dy + dx * dx <= r * r]
    lab_at = np.stack([regions[np.clip(ys + dy, 0, h - 1), np.clip(xs + dx, 0, w - 1)]
                       for dy, dx in offs], axis=1)
    lab_at.sort(axis=1)
    distinct = ((np.diff(lab_at, axis=1) != 0) & (lab_at[:, 1:] != 0)).sum(1) + (lab_at[:, 0] != 0)
    # structure tensor on the three Lab channels, at the scale of a source pixel
    sig = max(1.0, scale)
    jxx = jyy = jxy = 0
    for ch in range(3):
        gy, gx = np.gradient(lab[..., ch])
        jxx = jxx + ndimage.gaussian_filter(gx * gx, sig)
        jyy = jyy + ndimage.gaussian_filter(gy * gy, sig)
        jxy = jxy + ndimage.gaussian_filter(gx * gy, sig)
    tr = jxx + jyy
    det = jxx * jyy - jxy * jxy
    disc = np.sqrt(np.maximum(tr * tr / 4 - det, 0))
    lmax, lmin = tr / 2 + disc, tr / 2 - disc
    ratio = np.where(lmax > 1e-9, lmin / np.maximum(lmax, 1e-9), 0)
    out = np.full(nde.shape, " ", dtype="<U1")
    kind = np.where(distinct >= 3, "j", np.where(ratio[ys, xs] > 0.15, "c", "s"))
    out[ys, xs] = kind
    return out


def score(args: tuple) -> dict:
    exe, it, tier, mode, td, extra = args
    import svgeval
    from inkvec_bench import render
    from scipy import ndimage
    from skimage.color import rgb2lab
    svgeval.set_tier(tier)
    png, gt = svgeval.item_paths(it)
    out = Path(td) / f"{it['corpus']}__{it['stem']}__{mode}.svg"
    subprocess.run([exe, str(png), "-o", str(out), "--quiet", "--mode", mode, *extra],
                   check=True, capture_output=True)
    b = render.composite(render.render(out.read_text(encoding="utf-8"), 1024, 1024))
    b = svgeval._from_rgb8(svgeval._rgb8(b))
    ref = svgeval.gt_render(gt, it["corpus"], it["stem"])
    de = de_map(ref, b)
    from PIL import Image
    src_w = Image.open(png).size[0]
    scale = 1024 / src_w  # judge pixels per source pixel
    lab = rgb2lab(ref)
    nde = neighbour_de(lab)
    edges = nde > EDGE_DE
    if edges.any():
        dist = ndimage.distance_transform_edt(~edges) / scale
    else:
        dist = np.full(de.shape, np.inf)
    shaded = nde > SHADE_DE
    n = de.size
    bins = {
        "edge": dist <= 1.0,
        "near": (dist > 1.0) & (dist <= 3.0),
    }
    interior = dist > 3.0
    # Blobs: connected interior error above BLOB_DE of at least BLOB_PX source pixels.
    hot = interior & (de > BLOB_DE)
    lab_ids, k = ndimage.label(hot)
    blob = np.zeros_like(hot)
    if k:
        sizes = ndimage.sum(hot, lab_ids, index=np.arange(1, k + 1))
        big = np.flatnonzero(sizes / scale**2 >= BLOB_PX) + 1
        blob = np.isin(lab_ids, big)
    bins["blob"] = blob
    bins["flat"] = interior & ~blob & ~shaded
    bins["shaded"] = interior & ~blob & shaded
    # Split the edge band by local structure of the artist's render: a junction has three or
    # more flat regions within reach, a corner two regions and a structure tensor with two
    # strong directions, a smooth edge the rest.
    struct = edge_structure(lab, nde, scale)
    for kname in ("junction", "corner", "smooth"):
        m = bins["edge"] & (struct == kname[0])
        bins["edge_" + kname] = m
    res = {"icon": f"{it['corpus']}/{it['stem']}", "family": it["corpus"],
           "de00": float(de.mean())}
    for kname, m in bins.items():
        res[kname] = float(de[m].sum() / n)
        res[kname + "_area"] = float(m.mean())
    # Fill colour: the flat interior's own mean error per pixel (not weighted by area).
    flat = bins["flat"]
    res["flat_de_per_px"] = float(de[flat].mean()) if flat.any() else 0.0
    out.unlink(missing_ok=True)
    return res


def main() -> None:
    import svgeval
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(ROOT / "target/release/inkvec"))
    ap.add_argument("--tier", default="512ss")
    ap.add_argument("--mode", default="quality")
    ap.add_argument("--set", default="screen")
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--json", default=None)
    ap.add_argument("--extra", default="", help="further tracer arguments, space-separated")
    a = ap.parse_args()
    items = svgeval.load_sets()[a.set]
    with tempfile.TemporaryDirectory() as td:
        with ProcessPoolExecutor(a.workers) as pool:
            rows = list(pool.map(score, [(a.exe, it, a.tier, a.mode, td, a.extra.split()) for it in items],
                                 chunksize=2))
    keys = ["edge", "near", "flat", "shaded", "blob"]
    sub = ["edge_smooth", "edge_corner", "edge_junction"]
    fams = sorted({r["family"] for r in rows})
    print(f"{a.mode} {a.tier}: {len(rows)} icons; contribution to mean dE00 (share)")
    print(f"{'family':15s} {'dE00':>7s} " + " ".join(f"{k:>14s}" for k in keys))
    for f in fams + ["ALL (macro)"]:
        rs = rows if f.startswith("ALL") else [r for r in rows if r["family"] == f]
        if f.startswith("ALL"):
            per = {k: np.mean([np.mean([r[k] for r in rows if r["family"] == g]) for g in fams])
                   for k in keys + ["de00"]}
        else:
            per = {k: np.mean([r[k] for r in rs]) for k in keys + ["de00"]}
        tot = per["de00"]
        print(f"{f:15s} {tot:7.4f} " + " ".join(
            f"{per[k]:7.4f} ({100 * per[k] / max(tot, 1e-12):3.0f}%)" for k in keys))
    print("\nedge band by structure: share of the edge band's error / share of its area")
    for f in fams + ["ALL"]:
        rs = rows if f == "ALL" else [r for r in rows if r["family"] == f]
        e = sum(r["edge"] for r in rs)
        ea = sum(r["edge_area"] for r in rs)
        print(f"{f:15s} " + " ".join(
            f"{k[5:]:>9s} {100 * sum(r[k] for r in rs) / max(e, 1e-12):4.0f}% / {100 * sum(r[k + '_area'] for r in rs) / max(ea, 1e-12):4.0f}%"
            for k in sub))
    print("\nworst icons by bin:")
    for k in keys:
        top = sorted(rows, key=lambda r: -r[k])[:5]
        print(f"  {k:7s} " + ", ".join(f"{r['icon']} {r[k]:.3f}" for r in top))
    if a.json:
        Path(a.json).write_text(json.dumps(rows, indent=1))


if __name__ == "__main__":
    main()
