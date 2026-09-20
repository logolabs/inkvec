"""What each tier of the minifier costs, against the artist's own file.

    python bench/svgmin_tiers.py [--n 40]

There are three different things `inkvec-svgmin` can do, and they are not the same
bargain:

* **lossless** (`--bytes-only`) respells the numbers and leaves every one of them alone.
  Relative commands, dropped letters and separators, `H`/`V`/`S`, and the document rules.
  Nothing moves, so there is nothing to measure but the bytes.
* **rounding** (`--bytes-only --decimals N`) additionally writes fewer digits. The drawing
  is the same drawing; its coordinates are on a coarser grid.
* **refitting** (the default) runs the description-length fit as well: segments are
  removed, merged and re-chosen, and a curve is redrawn wherever a cheaper one stays
  inside the tolerance.

dE00 is the mean colour error of the rendered result against the rendered original at
1024 px, which is the same basis every other number in this crate is quoted on. For scale,
the tracer's own error against these files is 0.148, and around 1.0 is where a trained eye
starts to see a difference.
"""
from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

import svgeval  # noqa: E402
from inkvec_bench import render  # noqa: E402
from inkvec_bench.metrics import color as mcolor  # noqa: E402

JUDGE = svgeval.JUDGE_SIZE

# label -> the flags that produce it
TIERS: list[tuple[str, list[str]]] = [
    ("lossless", ["--bytes-only"]),
    ("round 3dp", ["--bytes-only", "--decimals", "3"]),
    ("round 2dp", ["--bytes-only", "--decimals", "2"]),
    ("round 1dp", ["--bytes-only", "--decimals", "1"]),
    ("refit 0.1px", []),
    ("refit 0.5px", ["--tolerance", "0.5"]),
    ("refit 1.0px", ["--tolerance", "1.0"]),
]


def rgb(svg: str) -> np.ndarray:
    img = render.render(svg, JUDGE, JUDGE)
    if img.shape[-1] == 4:
        img = img[..., :3] * img[..., 3:] + (1.0 - img[..., 3:])
    return img


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(ROOT / "target/release/inkvec-svgmin.exe"))
    ap.add_argument("--n", type=int, default=40)
    a = ap.parse_args()

    items = svgeval.load_sets()["screen"]
    step = max(1, len(items) // a.n)
    items = items[::step][: a.n]
    tmp = Path(tempfile.mkdtemp())

    rows = {label: {"b0": 0, "b1": 0, "de": [], "px": 0.0} for label, _ in TIERS}
    for it in items:
        _, gt = svgeval.item_paths(it)
        src = gt.read_text(encoding="utf-8")
        ref = rgb(src)
        for label, flags in TIERS:
            out_path = tmp / f"{it['stem']}.{label.replace(' ', '')}.svg"
            subprocess.run(
                [a.exe, str(gt), "-o", str(out_path), *flags], check=True, capture_output=True
            )
            out = out_path.read_text(encoding="utf-8")
            got = rgb(out)
            r = rows[label]
            r["b0"] += len(src.encode("utf-8"))
            r["b1"] += len(out.encode("utf-8"))
            r["de"].append(float(mcolor.delta_e00(ref, got)["de00_mean"]))
            r["px"] = max(r["px"], float(np.abs(ref - got).max() * 255.0))

    print(f"{len(items)} artist SVGs, rendered at {JUDGE} px\n")
    print(f"{'tier':14} {'bytes saved':>12} {'mean dE00':>10} {'worst dE00':>11} {'worst px':>9}")
    for label, _ in TIERS:
        r = rows[label]
        print(
            f"{label:14} {1 - r['b1'] / max(r['b0'], 1):11.1%} {np.mean(r['de']):10.4f} "
            f"{max(r['de']):11.4f} {r['px']:8.1f}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
