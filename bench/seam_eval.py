"""Measure anti-aliasing seams: background bleeding through where two fills abut.

    python bench/seam_eval.py --exe target/release/inkvec.exe [--out out/seams] [--svg-dir DIR]

A renderer anti-aliases each shape on its own. Where two shapes share an edge and
neither is painted under the other, an edge pixel half-covered by each composites to
``0.5*B + 0.25*A + 0.25*ground`` -- a hairline of whatever lies underneath. The seam is a
property of the *renderer's* per-shape coverage, so it is measured against the same
document rendered supersampled and box-filtered: there each fill covers its sub-pixels
outright and the hairline is diluted by the supersampling factor. The difference between
the two renders is the seam, plus an unbiased sliver of anti-aliasing rounding that the
single-shape controls (`prim_*`) put a number on.

Each document is rendered at 1x, 1.37x and 3x its input size, at the grid origin and at a
sub-pixel offset, because a seam that falls exactly on a pixel boundary shows nothing.
A pixel is a seam pixel when its colour differs from the supersampled render by more than
``--thresh`` (0..1 per channel, default 0.04 = 10/255).
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT / "bench")]

SCALES = (1.0, 1.37, 3.0)
OFFSETS = ((0.0, 0.0), (0.37, 0.21))
SS = 8


def _shift(svg: str, dx: float, dy: float) -> str:
    """Move the drawing by (dx, dy) *output* pixels, via the viewBox origin."""
    m = re.search(r'viewBox="([^"]+)"', svg)
    x, y, w, h = [float(v) for v in re.split(r"[ ,]+", m.group(1).strip())]
    return svg[: m.start(1)] + f"{x - dx:g} {y - dy:g} {w:g} {h:g}" + svg[m.end(1):]


def _render(svg: str, w: int, h: int) -> np.ndarray:
    from inkvec_bench import render

    return render.composite(render.render(svg, w, h))


def off_segment(direct: np.ndarray, ideal: np.ndarray) -> np.ndarray:
    """Distance from each direct pixel to the nearest blend of two ideal colours nearby.

    Anti-aliasing an edge between A and B -- however coarsely the renderer quantises
    coverage -- yields some mix of A and B, both of which the supersampled render shows
    within a pixel of it. A seam yields ``0.5*B + 0.25*A + 0.25*G`` with G the ground,
    which lies off every such segment. So the distance to the nearest segment between two
    of the 3x3 ideal neighbours is ~0 for honest anti-aliasing and ~0.25*|G - mix| at a
    seam, whatever colour the ground is.
    """
    h, w, _ = ideal.shape
    pad = np.pad(ideal, ((1, 1), (1, 1), (0, 0)), mode="edge")
    nb = [pad[dy:dy + h, dx:dx + w] for dy in range(3) for dx in range(3)]
    best = np.full((h, w), np.inf, dtype=np.float32)
    for i in range(9):
        a = nb[i]
        best = np.minimum(best, np.linalg.norm(direct - a, axis=2))
        for j in range(i + 1, 9):
            b = nb[j]
            ab = b - a
            den = (ab * ab).sum(axis=2)
            t = np.where(den > 1e-9, ((direct - a) * ab).sum(axis=2) / np.maximum(den, 1e-9), 0.0)
            t = np.clip(t, 0.0, 1.0)[..., None]
            best = np.minimum(best, np.linalg.norm(direct - (a + t * ab), axis=2))
    return best


def seam_map(svg: str, w0: int, h0: int, scale: float, off: tuple[float, float]) -> np.ndarray:
    """Per-pixel off-segment distance at one scale and offset (see `off_segment`)."""
    w, h = max(1, round(w0 * scale)), max(1, round(h0 * scale))
    # Offsets are in output pixels; the viewBox is in user units (input pixels).
    s = _shift(svg, off[0] / scale, off[1] / scale)
    direct = _render(s, w, h)
    big = _render(s, w * SS, h * SS)
    ideal = big.reshape(h, SS, w, SS, 3).mean(axis=(1, 3))
    return off_segment(direct, ideal)


def measure(svg: str, w0: int, h0: int, thresh: float) -> dict:
    out = {}
    for sc in SCALES:
        for off in OFFSETS:
            d = seam_map(svg, w0, h0, sc, off)
            key = f"{sc:g}x@{off[0]:g},{off[1]:g}"
            sel = d > thresh
            out[key] = {
                "n": int(sel.sum()),
                "strong": int((d > 0.10).sum()),
                "max": float(d.max()),
                "mean": float(d[sel].mean()) if sel.any() else 0.0,
                "mass": float(d[sel].sum()) / (w0 * h0),
            }
    return out


def _dims(svg: str) -> tuple[int, int]:
    m = re.search(r'viewBox="([^"]+)"', svg)
    _, _, w, h = [float(v) for v in re.split(r"[ ,]+", m.group(1).strip())]
    return round(w), round(h)


def one(args):
    exe, corpus, stem, png, gt, out_dir, extra, thresh = args
    out = Path(out_dir) / f"{corpus}__{stem}.svg"
    if exe:
        r = subprocess.run([exe, str(png), "-o", str(out), "--quiet", *extra], capture_output=True)
        if r.returncode != 0 or not out.exists():
            return {"corpus": corpus, "stem": stem, "fail": r.stderr.decode(errors="replace")[-300:]}
    svg = out.read_text(encoding="utf-8")
    from PIL import Image

    w0, h0 = Image.open(png).size
    res = {"corpus": corpus, "stem": stem, "bytes": len(svg.encode()), "ours": measure(svg, w0, h0, thresh)}
    if gt is not None:
        # The artist's file, rendered at the same pixel size as ours for comparison.
        g = gt.read_text(encoding="utf-8")
        try:
            gw, gh = _dims(g)
            fac = w0 / max(gw, 1)
            g = re.sub(r'(<svg[^>]*?)\swidth="[^"]*"', r"\1", g, count=1)
            g = re.sub(r'(<svg[^>]*?)\sheight="[^"]*"', r"\1", g, count=1)
            res["gt"] = measure(g, w0, round(gh * fac), thresh)
        except Exception as e:  # noqa: BLE001
            res["gt_fail"] = repr(e)
    return res


def items(families: list[str], per_family: int) -> list[tuple[str, str, Path, Path]]:
    import svgeval

    sel = svgeval.load_sets()["screen"]
    got: list = []
    for it in sel:
        if families and it["corpus"] not in families:
            continue
        png, gt = svgeval.item_paths(it)
        got.append((it["corpus"], it["stem"], png, gt))
    if per_family:
        seen: dict = {}
        keep = []
        for g in got:
            seen[g[0]] = seen.get(g[0], 0) + 1
            if seen[g[0]] <= per_family:
                keep.append(g)
        got = keep
    tier = svgeval.tier()
    for stem in ("mosaic_pie6", "mosaic_pie12", "mosaic_grid4", "mosaic_grid6", "logo_like",
                 "stack_overlap", "prim_circle", "prim_star5"):
        png = ROOT / "bench/data/corpus_raster/synthetic" / tier / f"{stem}.png"
        gt = ROOT / "bench/data/corpus_svg/synthetic" / f"{stem}.svg"
        if png.exists() and not any(g[1] == stem for g in got):
            got.append(("synthetic", stem, png, gt))
    return got


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--exe", default=None, help="tracer; omit to re-measure SVGs already in --out")
    ap.add_argument("--out", default=str(ROOT / "out" / "seams"))
    ap.add_argument("--families", default="")
    ap.add_argument("--per-family", type=int, default=0)
    ap.add_argument("--thresh", type=float, default=0.04)
    ap.add_argument("--workers", type=int, default=max(1, (os.cpu_count() or 4) // 2))
    ap.add_argument("--no-gt", action="store_true")
    ap.add_argument("extra", nargs="*", help="extra tracer args, after --")
    a = ap.parse_args()
    out_dir = Path(a.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    fams = [f for f in a.families.split(",") if f]
    exe = str(Path(a.exe).resolve()) if a.exe else None
    jobs = [(exe, c, s, p, None if a.no_gt else g, str(out_dir), a.extra, a.thresh)
            for c, s, p, g in items(fams, a.per_family)]
    with ProcessPoolExecutor(a.workers) as ex:
        results = list(ex.map(one, jobs))
    (out_dir / "seams.json").write_text(json.dumps(results, indent=1), encoding="utf-8")

    def summary(side: str):
        rows = [r for r in results if side in r]
        by: dict = {}
        for r in rows:
            by.setdefault(r["corpus"], []).append(r)
        print(f"\n== {side}  (seam px per image = pixels > {a.thresh:g} off the {SS}x supersampled render)")
        keys = list(rows[0][side].keys()) if rows else []
        print(f"{'family':14s} {'n':>3s} " + " ".join(f"{k:>16s}" for k in keys) + "   max")
        for fam, rs in sorted(by.items()):
            cells = []
            mx = 0.0
            for k in keys:
                cells.append(f"{np.mean([r[side][k]['n'] for r in rs]):16.1f}")
                mx = max(mx, max(r[side][k]["max"] for r in rs))
            print(f"{fam:14s} {len(rs):3d} " + " ".join(cells) + f"   {mx:.3f}")
        tot = {k: float(np.sum([r[side][k]["n"] for r in rows])) for k in keys}
        strong = {k: float(np.sum([r[side][k]["strong"] for r in rows])) for k in keys}
        print("total seam px  " + " ".join(f"{tot[k]:16.0f}" for k in keys))
        print("total >0.10    " + " ".join(f"{strong[k]:16.0f}" for k in keys))
        return tot

    summary("ours")
    if not a.no_gt:
        summary("gt")
    fails = [r for r in results if "fail" in r]
    if fails:
        print(f"\n{len(fails)} failed: " + ", ".join(r["stem"] for r in fails[:10]))
    print(f"bytes total {sum(r.get('bytes', 0) for r in results)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
