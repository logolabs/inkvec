"""How does the tolerance have to scale with resolution to keep the drawing the same?

The parameter count is not arbitrary: for a curve approximated to tolerance `d` by a
primitive of order `p`, the number of segments goes as

    N  ~  integral( curvature^(1/(p+1)) ds )  /  d^(1/(p+1))

The integral is a property of the *shape*. It does not change when the same logo is
exported larger. Only `d` changes -- and `d` is currently pinned to the pixel grid, so it
shrinks as the raster grows and buys segments the content never asked for.

That predicts `N ~ s^(1/2)` for a line-dominated fit and `s^(1/4)` for a cubic-dominated
one. This measures it instead of assuming it: the same icons at 128, 256, 512 and 1024, at
a range of `--precision`, counting the segments each returns. Two things fall out.

  * The exponent, from the default-precision column: how fast the drawing grows with
    resolution when nothing corrects for it.
  * The calibration: what precision each tier needs to land on the segment count the 128 px
    tier produces. If that precision tracks the resolution linearly, then "precision in
    content units" is the whole fix, and it is one multiplication.

    python bench/complexity.py --exe target/release/inkvec.exe --n 12
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "bench"))

TIERS = ("128ss", "256ss", "512ss", "1024ss")
SEG_RE = re.compile(r"segments\s+(\d+)\s+\((\d+) line, (\d+) cubic\)")


def trace(exe: str, png: Path, out: Path, precision: float) -> tuple[int, int, int] | None:
    r = subprocess.run(
        [exe, str(png), "-o", str(out), "--precision", f"{precision:g}"],
        capture_output=True, text=True,
    )
    m = SEG_RE.search(r.stdout + r.stderr)
    return (int(m.group(1)), int(m.group(2)), int(m.group(3))) if m else None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--exe", default=str(ROOT / "target/release/inkvec.exe"))
    ap.add_argument("--n", type=int, default=12, help="icons to average over")
    a = ap.parse_args()

    from inkvec_bench import config  # noqa: PLC0415
    import svgeval  # noqa: PLC0415

    items = []
    for it in svgeval.load_sets()["screen"]:
        fam, stem = it["corpus"], it["stem"]
        if all((config.CORPUS_RASTER_DIR / fam / t / f"{stem}.png").exists() for t in TIERS):
            items.append((fam, stem))
        if len(items) >= a.n:
            break
    print(f"{len(items)} icons present at every tier\n")

    tmp = ROOT / "bench" / "data" / "_cx.svg"

    # 1. How the drawing grows with resolution at the shipped precision.
    print("At the default precision (0.1 px):\n")
    print(f"{'tier':>8s} {'segments':>10s} {'vs 128':>8s} {'lines':>7s} {'cubics':>7s}")
    base = None
    for t in TIERS:
        tot = np.array([trace(a.exe, config.CORPUS_RASTER_DIR / f / t / f"{s}.png", tmp, 0.1)
                        or (0, 0, 0) for f, s in items], float)
        mean = tot.mean(axis=0)
        if base is None:
            base = mean[0]
        print(f"{t:>8s} {mean[0]:10.1f} {mean[0]/base:7.2f}x {mean[1]:7.1f} {mean[2]:7.1f}")

    scale = {t: 2 ** i for i, t in enumerate(TIERS)}
    growth = mean[0] / base
    print(f"\n  growth exponent: log({growth:.2f}) / log(8) = "
          f"{np.log(growth) / np.log(8):.3f}"
          f"   (1/2 = lines, 1/4 = cubics)\n")

    # 2. What precision each tier needs to land on the 128 px segment count.
    print("Precision needed to reproduce the 128 px drawing:\n")
    print(f"{'tier':>8s} {'precision':>10s} {'vs 128':>8s} {'segments':>10s} {'target':>8s}")
    target = base
    grid = [0.1 * 2 ** k for k in range(0, 7)]      # 0.1 .. 6.4 px
    for t in TIERS:
        best, best_p = None, None
        for p in grid:
            tot = np.array([trace(a.exe, config.CORPUS_RASTER_DIR / f / t / f"{s}.png", tmp, p)
                            or (0, 0, 0) for f, s in items], float)
            n = tot.mean(axis=0)[0]
            if best is None or abs(n - target) < abs(best - target):
                best, best_p = n, p
        print(f"{t:>8s} {best_p:10.2f} {best_p/0.1:7.1f}x {best:10.1f} {target:8.1f}")

    print("\nIf that middle column tracks the resolution (1, 2, 4, 8), the tolerance simply\n"
          "wants stating per unit of content rather than per pixel.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
