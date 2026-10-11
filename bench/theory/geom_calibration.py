"""Which metric measures "the same design"? `geom` against dE00 on known displacements.

Each case is an artist file and a trace of it whose every edge is moved by a known `δ`
(source pixels at a 512 px input): a disc grown by `δ`, in two colour contrasts (dark red on
transparent, and a pale grey close to white). A metric of geometric match should read the
displacement, the same for both contrasts, and in proportion to `δ`. `geom`
(`inkvec_bench/geomatch.py`) does by construction; dE00, the gate's colour error, reads the
contrast as much as the displacement.

    python3 bench/theory/geom_calibration.py
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))

DISC = ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">'
        '<circle cx="12" cy="12" r="{r}" fill="{c}"/></svg>')


def main() -> None:
    from inkvec_bench import render
    from inkvec_bench.geomatch import geomatch
    from inkvec_bench.metrics import color as mcolor
    print(f"{'colour':10s} {'delta px':>9s} {'geom px':>9s} {'dE00':>9s}")
    for colour in ("#d22f27", "#e8e8e8"):
        art = DISC.format(r=7.0, c=colour)
        ref = render.composite(render.render(art, 1024, 1024))
        for delta in (0.0, 0.05, 0.1, 0.25, 0.5):
            ours = DISC.format(r=7.0 + delta * 24 / 512, c=colour)
            g = geomatch(ours, art, 512)["geom"]
            b = render.composite(render.render(ours, 1024, 1024))
            de = float(mcolor.delta_e00(ref, b)["de00_mean"])
            print(f"{colour:10s} {delta:9.3f} {g:9.4f} {de:9.4f}")


if __name__ == "__main__":
    main()
