"""Pilot: does a learned x4 upscale before tracing beat tracing the 128 px intake?

    python bench/sr_pilot.py <exe> --sr-dir <dir with <stem>.png at 512> [--stems ...]

For each icon: trace the 128ss intake directly, and trace the SR 512 px image with the
MDL exchange rate held fixed (precision x4 so lambda = ln(extent/precision) is the
same; min-area x16). Both judged at 1024 px against the artist's SVG. Prints paired
dE00 / DISTS / params ratio and the per-icon rows.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "bench"))
from inkvec_bench import render, svgmodel  # noqa: E402
from inkvec_bench.metrics import color as mcolor  # noqa: E402
from inkvec_bench.metrics import raster  # noqa: E402
import svgeval  # noqa: E402

J = 1024


def score(exe: Path, png: Path, gt_ref: np.ndarray, gt_params: int, extra: list[str], out: Path) -> dict:
    subprocess.run([str(exe), str(png), "-o", str(out), "--quiet", *extra], check=True, capture_output=True)
    svg = out.read_text(encoding="utf-8")
    b = render.composite(render.render(svg, J, J))
    return dict(de00=float(mcolor.delta_e00(gt_ref, b)["de00_mean"]), dists=float(raster.dists_distance(gt_ref, b)),
                ratio=svgmodel.parse(svg).n_params / max(1, gt_params))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("exe")
    ap.add_argument("--sr-dir", required=True)
    ap.add_argument("--stems", nargs="*")
    a = ap.parse_args()
    exe = Path(a.exe).resolve()
    sets = svgeval.load_sets()
    items = {it["stem"]: it for it in sets["full"]}
    sr_dir = Path(a.sr_dir)
    stems = a.stems or [p.stem for p in sorted(sr_dir.glob("*.png")) if p.stem in items]
    rows = []
    for stem in stems:
        it = items[stem]
        png, gt = svgeval.item_paths(it)
        sr = sr_dir / f"{stem}.png"
        if not sr.exists():
            print("no SR image for", stem); continue
        ref = render.composite(render.render(gt.read_text(encoding="utf-8"), J, J))
        d = score(exe, png, ref, it["gt_params"], [], svgeval.WORK / "_srp_direct.svg")
        s = score(exe, sr, ref, it["gt_params"], ["--precision", "0.4", "--min-area", "32"], svgeval.WORK / "_srp_sr.svg")
        rows.append((stem, it["corpus"], d, s))
        print(f"{stem:40s} {it['corpus']:13s} direct dE {d['de00']:.3f} DISTS {d['dists']:.4f} ratio {d['ratio']:.2f} | "
              f"SR x4 dE {s['de00']:.3f} DISTS {s['dists']:.4f} ratio {s['ratio']:.2f}", flush=True)
    if rows:
        dd = np.array([r[3]["de00"] - r[2]["de00"] for r in rows]); di = np.array([r[3]["dists"] - r[2]["dists"] for r in rows])
        print(f"\n{len(rows)} icons: dE00 mean delta {dd.mean():+.4f} (better {int((dd < 0).sum())} / worse {int((dd > 0).sum())}), "
              f"DISTS mean delta {di.mean():+.5f} (better {int((di < 0).sum())} / worse {int((di > 0).sum())}), "
              f"ratio {np.mean([r[2]['ratio'] for r in rows]):.2f} -> {np.mean([r[3]['ratio'] for r in rows]):.2f}")
        json.dump([dict(stem=r[0], corpus=r[1], direct=r[2], sr=r[3]) for r in rows],
                  open(ROOT / "bench" / "data" / "sr_pilot.json", "w"), indent=1)


if __name__ == "__main__":
    main()
