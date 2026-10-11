"""Where the noise of a real input sits: the `web` tier against the exact raster it stands for.

The gate's `web` tier (`bench/build_web_tier.py`) is each screen icon as logos usually reach a
tracer: the 512 px raster on white, resized to 400 px (bicubic) and saved as a quality-80
JPEG with 4:2:0 chroma. Its ideal is the exact box-filtered raster at 400 px: the artist's
file rendered at 8x and averaged, on white, unrounded. Against it, the error splits in two:

* `resample`  bicubic(512) - exact(400): deterministic, the resampling kernel's blur and
              overshoot (not noise: a forward model can reproduce it);
* `jpeg`      jpeg - bicubic(512): the codec's quantisation (block steps, ringing, chroma at
              half resolution);
* `total`     jpeg - exact(400).

Each is reported as an RMS in 8-bit levels, by distance to the nearest edge of the exact
raster (a pixel that differs from a 4-neighbour by more than two levels), on luma (Y) and on
chroma (the Cb/Cr pair, as one RMS); next to `est`, the engine's own noise estimate
(`coverage::estimate_noise`: the tenth percentile of |Laplacian| of luma, read as a
Gaussian sigma). The question it answers: is one sigma read from flat regions the noise the
boundary windows see?

A second table checks the window identity under the effective blur: for each edge of the
exact raster, the coverage of one side summed over a column window grown by `r` pixels each
side, in the exact raster and in three observed ones (unmixed on luma between their own
plateau colours): the exact raster rounded to 8 bits, the bicubic one before the JPEG step,
and the `web` one. A column window's sum is the column-area profile convolved with the
kernel's horizontal marginal, plus the plateau step times the kernel's vertical first moment
(Fubini), when every input pixel's weights over the outputs sum to one. Measured
(2026-10-10, 41 icons): every mean is within 0.0004 px of zero, so the sums stay unbiased
under resampling and JPEG and growing the window only adds noise; the spread is what grows,
from 0.005 px of area (8-bit) to 0.039 (bicubic: Pillow normalises weights per output pixel,
so at a non-integer factor an edge's sum ripples with its phase against the output grid;
the same 0.041 from an exact 512 px render, about 0.004 px of it a translation) and 0.066
(`web`; the codec's part about 0.047).

    python3 bench/theory/noise_profile.py [--per-family 6]

Needs scipy and resvg-py (about a minute for six icons per family).
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))

SIZE = 400
SS = 8
#: Distance-to-edge bins, in pixels of the 400 px raster: [lo, hi).
BINS = ((0, 1), (1, 2), (2, 3), (3, 5), (5, 8), (8, 10_000))
#: A pixel is on an edge when it differs from a 4-neighbour by more than this (8-bit levels).
EDGE_LEVELS = 2.0
#: Window growth tested by the identity table, in pixels each side.
GROWTH = (0, 1, 2, 3, 4)
#: Plateau length an edge needs on both sides to enter the identity table.
PLATEAU = 10


def exact_on_white(svg: str) -> np.ndarray:
    """The artist's file at 400 px, box-filtered from 8x, composited on white, in [0, 1]."""
    from inkvec_bench import render
    rgba = render.render(svg, SIZE * SS, SIZE * SS)
    a = rgba[..., 3:4]
    pre = np.concatenate([rgba[..., :3] * a, a], axis=-1)
    blk = pre.reshape(SIZE, SS, SIZE, SS, 4).mean(axis=(1, 3))
    return blk[..., :3] + (1.0 - blk[..., 3:4])


def bicubic_on_white(png: Path) -> np.ndarray:
    """`build_web_tier`'s pipeline without the JPEG step, unrounded where Pillow allows."""
    from PIL import Image
    im = Image.open(png).convert("RGBA")
    page = Image.new("RGBA", im.size, (255, 255, 255, 255))
    page.alpha_composite(im)
    out = page.convert("RGB").resize((SIZE, SIZE), Image.BICUBIC)
    return np.asarray(out, dtype=np.float64) / 255.0


def ycc(rgb: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """JPEG's (BT.601 full-range) luma and the chroma pair, in the same units as `rgb`."""
    r, g, b = rgb[..., 0], rgb[..., 1], rgb[..., 2]
    y = 0.299 * r + 0.587 * g + 0.114 * b
    cb = -0.168736 * r - 0.331264 * g + 0.5 * b
    cr = 0.5 * r - 0.418688 * g - 0.081312 * b
    return y, np.stack([cb, cr], axis=-1)


def engine_sigma(y: np.ndarray) -> float:
    """`coverage::estimate_noise` on luma: the tenth percentile of |4c - neighbours|, read
    through the half-normal's tenth percentile and the kernel's gain, floored at 0.5/255."""
    lap = np.abs(4 * y[1:-1, 1:-1] - y[1:-1, :-2] - y[1:-1, 2:] - y[:-2, 1:-1] - y[2:, 1:-1])
    flat = np.sort(lap.ravel())
    q = flat[int(len(flat) * 0.10)]
    return max(q / 0.12566 / np.sqrt(20.0), 0.5 / 255.0)


def edge_distance(ref: np.ndarray) -> np.ndarray:
    from scipy.ndimage import distance_transform_edt
    d = np.zeros(ref.shape[:2], dtype=bool)
    step = EDGE_LEVELS / 255.0
    dx = np.abs(ref[:, 1:] - ref[:, :-1]).max(axis=-1) > step
    dy = np.abs(ref[1:, :] - ref[:-1, :]).max(axis=-1) > step
    d[:, 1:] |= dx
    d[:, :-1] |= dx
    d[1:, :] |= dy
    d[:-1, :] |= dy
    return distance_transform_edt(~d)


def column_windows(ref_y: np.ndarray) -> list[tuple[int, int, int, float, float]]:
    """Column runs where luma ramps between two plateaus that differ by at least 0.25: each as
    (column, first, last, the plateau above, the plateau below). Only isolated edges with
    plateaus at least `PLATEAU` px long on both sides are kept, so a grown window holds one
    edge and the outer part of each plateau is beyond any window's reach."""
    out = []
    h, w = ref_y.shape
    for x in range(w):
        col = ref_y[:, x]
        flat = np.abs(np.diff(col)) < 1e-4
        y = 0
        while y < h - 1:
            if flat[y]:
                y += 1
                continue
            a = y
            while y < h - 1 and not flat[y]:
                y += 1
            b = y                          # col[a] .. col[b] ramps; flat before a, after b
            lo, hi = a - PLATEAU, b + PLATEAU
            if lo < 0 or hi >= h:
                continue
            if not (flat[lo:a].all() and flat[b:hi].all()):
                continue
            top, bot = col[a], col[b]
            if abs(top - bot) < 0.25:
                continue
            out.append((x, a, b, top, bot))
    return out


def window_bias(ref_y: np.ndarray, obs: dict[str, np.ndarray]) -> dict[str, dict[int, list]]:
    """Per observed raster and growth `r`: its window sum minus the exact one, in pixels of
    area, over every column window of `column_windows` (the coverage of the `top` plateau's
    ink). An observed column is unmixed between its own plateaus, read as medians of the
    outer `PLATEAU - 6` pixels each side (robust ink colours: JPEG shifts a plateau by a
    level or two)."""
    out = {name: {r: [] for r in GROWTH} for name in obs}
    k = PLATEAU - 6
    for x, a, b, top, bot in column_windows(ref_y):
        for name, o in obs.items():
            top_o = float(np.median(o[a - PLATEAU:a - PLATEAU + k, x]))
            bot_o = float(np.median(o[b + PLATEAU - k + 1:b + PLATEAU + 1, x]))
            if abs(top_o - bot_o) < 0.2:
                break
            for r in GROWTH:
                s = slice(a - r, b + 1 + r)
                cov_ref = (ref_y[s, x] - bot) / (top - bot)
                cov_o = (o[s, x] - bot_o) / (top_o - bot_o)
                out[name][r].append(float(cov_o.sum() - cov_ref.sum()))
    return out


def main() -> int:
    import svgeval
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--per-family", type=int, default=6)
    a = ap.parse_args()
    from PIL import Image
    items = svgeval.load_sets()["screen"]
    by_fam: dict[str, list] = {}
    for it in items:
        by_fam.setdefault(it["corpus"], []).append(it)
    base = ROOT / "bench" / "data"
    acc = {k: {b: [] for b in BINS} for k in ("resample_y", "jpeg_y", "total_y", "jpeg_c")}
    sig = []
    bias = {name: {r: [] for r in GROWTH} for name in ("8-bit", "bicubic", "web")}
    n = 0
    for fam, its in sorted(by_fam.items()):
        for it in its[: a.per_family]:
            svg = (base / "corpus_svg" / fam / f"{it['stem']}.svg").read_text(encoding="utf-8")
            ref = exact_on_white(svg)
            bic = bicubic_on_white(base / "corpus_raster" / fam / "512ss" / f"{it['stem']}.png")
            web = np.asarray(Image.open(base / "corpus_raster" / fam / "web" / f"{it['stem']}.jpg")
                             .convert("RGB"), dtype=np.float64) / 255.0
            dist = edge_distance(ref)
            (ry, _), (by, _), (wy, wc) = ycc(ref), ycc(bic), ycc(web)
            _, bc = ycc(bic)
            for lo, hi in BINS:
                m = (dist >= lo) & (dist < hi)
                if not m.any():
                    continue
                acc["resample_y"][(lo, hi)].append((by - ry)[m])
                acc["jpeg_y"][(lo, hi)].append((wy - by)[m])
                acc["total_y"][(lo, hi)].append((wy - ry)[m])
                acc["jpeg_c"][(lo, hi)].append(np.linalg.norm(wc - bc, axis=-1)[m] / np.sqrt(2))
            sig.append(engine_sigma(wy))
            obs = {"8-bit": np.round(ry * 255) / 255, "bicubic": by, "web": wy}
            for name, per_r in window_bias(ry, obs).items():
                for r, v in per_r.items():
                    bias[name][r].extend(v)
            n += 1
    lv = 255.0
    print(f"{n} icons, {SIZE} px; RMS in 8-bit levels by distance to the nearest exact edge\n")
    print(f"{'distance':>10} {'share':>6} {'resample Y':>11} {'jpeg Y':>8} {'total Y':>8} "
          f"{'jpeg CbCr':>10}")
    total_px = sum(np.concatenate(acc["total_y"][b]).size for b in BINS if acc["total_y"][b])
    for b in BINS:
        if not acc["total_y"][b]:
            continue
        cols = {k: np.concatenate(acc[k][b]) for k in acc}
        rms = {k: float(np.sqrt(np.mean(v ** 2))) * lv for k, v in cols.items()}
        label = f"{b[0]}-{b[1]}" if b[1] < 1000 else f">={b[0]}"
        print(f"{label:>10} {cols['total_y'].size / total_px:6.1%} {rms['resample_y']:11.2f} "
              f"{rms['jpeg_y']:8.2f} {rms['total_y']:8.2f} {rms['jpeg_c']:10.2f}")
    s = np.array(sig) * lv
    print(f"\nengine estimate (one sigma per image, luma): median {np.median(s):.2f}, "
          f"p10 {np.percentile(s, 10):.2f}, p90 {np.percentile(s, 90):.2f} levels")
    print("\ncolumn windows grown by r px each side: observed sum - exact sum (px of area);"
          "\n8-bit = the exact raster rounded, bicubic = before the JPEG step, web = the tier")
    print(f"{'':>9} {'r':>3} {'windows':>8} {'mean':>8} {'rms':>8} {'p95 |.|':>8}")
    for name, per_r in bias.items():
        for r in GROWTH:
            v = np.array(per_r[r])
            if v.size:
                print(f"{name:>9} {r:>3} {v.size:>8} {v.mean():8.4f} "
                      f"{np.sqrt(np.mean(v ** 2)):8.4f} {np.percentile(np.abs(v), 95):8.4f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
