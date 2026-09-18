"""How finely can this harness discriminate at all?

Suggested by the training job on 2026-09-09, and it is the right question to ask before
believing any A/B. Two numbers come out.

**The floor.** Tracing a clean render does not score zero -- the tracer has its own
irreducible error, and a model that reaches it is at the ceiling, not merely good. Any
comparison has to be read against that floor rather than against zero.

**The flip rate.** Score `trace(clean)` against `trace(degraded)` per icon. The clean input
carries strictly more information, so it should win every time. Wherever it does not, the
metric has told us something untrue about which input was better -- and the fraction of
icons where that happens bounds the resolution of every checkpoint-versus-checkpoint
comparison run through this harness. A difference smaller than the flip rate is not a
result.

    python bench/oracle.py --n 60 --quality 50
"""

from __future__ import annotations

import argparse
import io
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "bench"))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--exe", default=str(ROOT / "target/release/inkvec.exe"))
    ap.add_argument("--n", type=int, default=60)
    ap.add_argument("--tier", default="128ss")
    ap.add_argument("--quality", type=int, default=50, help="JPEG quality for the degraded arm")
    a = ap.parse_args()

    from inkvec_bench import config, render, svgmodel  # noqa: PLC0415
    from inkvec_bench.metrics import color as mcolor  # noqa: PLC0415
    import svgeval  # noqa: PLC0415

    items = []
    for it in svgeval.load_sets()["screen"]:
        png = config.CORPUS_RASTER_DIR / it["corpus"] / a.tier / f"{it['stem']}.png"
        gt = config.CORPUS_SVG_DIR / it["corpus"] / f"{it['stem']}.svg"
        if png.exists() and gt.exists():
            items.append((it["corpus"], it["stem"], png, gt))
        if len(items) >= a.n:
            break

    tmp = ROOT / "bench" / "data" / "_oracle.svg"
    deg = ROOT / "bench" / "data" / "_oracle.jpg"
    J = 512

    def trace_and_score(src: Path, truth: np.ndarray, gt_params: int):
        r = subprocess.run([a.exe, str(src), "-o", str(tmp), "-q"], capture_output=True)
        if r.returncode != 0:
            return None
        svg = tmp.read_text(encoding="utf-8")
        got = render.composite(render.render(svg, J, J))
        de = mcolor.delta_e00(truth, got)["de00_mean"]
        n = svgmodel.parse(svg).n_params
        return float(de), n / max(1, gt_params)

    rows = []
    for corpus, stem, png, gt in items:
        gsvg = gt.read_text(encoding="utf-8")
        truth = render.composite(render.render(gsvg, J, J))
        gp = svgmodel.parse(gsvg).n_params

        # degraded arm: the same raster through JPEG
        rgba = np.asarray(Image.open(png).convert("RGBA")).astype(np.float32) / 255
        flat = rgba[..., :3] * rgba[..., 3:4] + (1 - rgba[..., 3:4])
        Image.fromarray((flat * 255 + 0.5).astype(np.uint8)).save(deg, "JPEG", quality=a.quality)

        c = trace_and_score(png, truth, gp)
        d = trace_and_score(deg, truth, gp)
        if c and d:
            rows.append((corpus, stem, c, d))

    if not rows:
        print("no rows")
        return 1

    clean_de = np.array([r[2][0] for r in rows])
    deg_de = np.array([r[3][0] for r in rows])
    clean_ra = np.array([r[2][1] for r in rows])
    deg_ra = np.array([r[3][1] for r in rows])
    flips = deg_de < clean_de

    print(f"{len(rows)} icons, tier {a.tier}, degraded arm = JPEG q{a.quality}\n")
    print(f"{'':22s}{'dE00 median':>13s}{'dE00 mean':>11s}{'ratio median':>14s}")
    print(f"{'trace(clean)':22s}{np.median(clean_de):13.4f}{clean_de.mean():11.4f}{np.median(clean_ra):14.4f}")
    print(f"{'trace(degraded)':22s}{np.median(deg_de):13.4f}{deg_de.mean():11.4f}{np.median(deg_ra):14.4f}")
    print(f"\n  the tracer's own floor on clean input: dE00 {np.median(clean_de):.4f}")
    print(f"  a model cannot score below it; a number near it means at ceiling, not good.\n")
    print(f"  FLIP RATE: the degraded input scored better on {flips.sum()} of {len(rows)} "
          f"icons ({100 * flips.mean():.1f}%)")
    if flips.any():
        m = np.abs(deg_de - clean_de)[flips]
        print(f"  flipped margins: median {np.median(m):.4f}, max {m.max():.4f} dE00")
    print(f"\n  A/B differences below roughly {np.median(np.abs(deg_de - clean_de)) * 0.5:.4f} dE00 "
          f"are inside this harness's own noise.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
