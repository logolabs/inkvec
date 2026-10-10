"""inkvec's own sub-pixel stage against the strip estimator, on exactly rendered shapes.

Renders each shape of `strip_eval.shapes` with the exact area rasteriser, writes it as an
8-bit black-on-white PNG, runs the inkvec binary with `INKVEC_DUMP_CONTOUR` (the boundary
points `planar::measure_subpixel` measured, stage 07, before the boundary solve), and scores
those points and the strip estimator's against the true geometry.

    python3 bench/theory/inkvec_compare.py --exe target/release/inkvec [--mode quality]

inkvec puts pixel centres on integers; this file's pixels are [i, i+1], so inkvec's points
are shifted by +1/2 before scoring.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import tempfile

import numpy as np
from PIL import Image

import exact_raster as er
import strip_eval as se


def read_dump(path: str) -> np.ndarray:
    pts = []
    with open(path) as f:
        for line in f:
            if line.startswith("#") or not line.strip():
                continue
            x, y, _ = (float(t) for t in line.split())
            pts.append((x + 0.5, y + 0.5))
    return np.array(pts)


def run(exe: str, png: str, mode: str, workdir: str, extra_env: dict | None = None) -> np.ndarray:
    dump = os.path.join(workdir, "contour.txt")
    if os.path.exists(dump):
        os.remove(dump)
    env = dict(os.environ, INKVEC_DUMP_CONTOUR=dump, **(extra_env or {}))
    subprocess.run([exe, png, "-o", os.path.join(workdir, "out.svg"), "--mode", mode],
                   env=env, check=True, capture_output=True)
    return read_dump(dump)


def interior(p: np.ndarray, size: int, margin: float = 2.0) -> np.ndarray:
    return (p[:, 0] > margin) & (p[:, 0] < size - margin) & (p[:, 1] > margin) & (p[:, 1] < size - margin)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default="target/release/inkvec")
    ap.add_argument("--mode", default="quality")
    ap.add_argument("--size", type=int, default=64)
    args = ap.parse_args()
    rng = np.random.default_rng(20261009)
    size = args.size
    acc: dict[str, list[np.ndarray]] = {"inkvec stage 07": [], "levelset": [], "strip": []}
    per_kind: dict[str, dict[str, list[np.ndarray]]] = {}
    with tempfile.TemporaryDirectory() as td:
        for name, poly in se.shapes(rng, size):
            a = er.rasterize([poly], size, size)
            img = np.round((1.0 - a) * 255.0).astype(np.uint8)
            png = os.path.join(td, "in.png")
            Image.fromarray(np.stack([img] * 3, axis=2)).save(png)
            got = {"inkvec stage 07": run(args.exe, png, args.mode, td)}
            est = se.estimate(img_to_cov(img), 0.02)
            got["levelset"] = est["levelset"]
            got["strip"] = est["strip"]
            kind = name.split()[0]
            for k, p in got.items():
                p = p[interior(p, size)]
                d = se.dist_to_polygon(p, poly)
                acc[k].append(d)
                per_kind.setdefault(kind, {}).setdefault(k, []).append(d)
    print(f"mode={args.mode} size={size}, 8-bit input, distance to the true boundary (px)")
    for k, v in acc.items():
        print(f"  {k:16s} {se.summarize(np.concatenate(v))}")
    for kind, dd in per_kind.items():
        print(f" {kind}")
        for k, v in dd.items():
            print(f"    {k:16s} {se.summarize(np.concatenate(v))}")


def img_to_cov(img: np.ndarray) -> np.ndarray:
    return 1.0 - img.astype(np.float64) / 255.0


if __name__ == "__main__":
    main()
