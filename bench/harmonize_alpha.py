"""Does shape harmonization keep transparency honest?

    python bench/harmonize_alpha.py [--exe out/native-default.exe] [--args ""]

Harmonization stamps one consensus geometry over every member of a cluster of repeated
shapes, and a member's holes come with it. Under native transparency a hole is not always
empty: a translucent face sits in it at its own geometry, punched out of the face around it,
and a translucent or faded face is punched out of what lies under it. Moving one side of such
a pair and not the other opens a gap onto the ground or paints the ground twice -- invisible
over white, plain over the dark ground and on the alpha channel. Each case repeats one
compound shape at fractional offsets, so its traces differ by a fraction of a pixel and
cluster; each is traced with harmonization on and off and scored as `white_on_clear.py`
scores (white, dark, alpha; mean absolute error).
"""
from __future__ import annotations

import argparse
import shlex
import subprocess
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

from white_on_clear import DARK, SIZE, over, rgba  # noqa: E402

from inkvec_bench import render  # noqa: E402

SQ = 'xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512"'
# Three by three, each cell off the pixel grid by a different fraction.
CELLS = [(28 + 160 * i + 0.37 * (j + 1), 28 + 160 * j + 0.29 * (i + 2)) for j in range(3) for i in range(3)]


def grid(cell) -> str:
    return "".join(cell(x, y) for x, y in CELLS)


def tile(x, y, fill, extra=""):
    # A rounded square with a round counter, as one even-odd path.
    return (f'<path fill-rule="evenodd" fill="{fill}"{extra} d="M{x + 16} {y}h96a16 16 0 0 1 16 16'
            f'v96a16 16 0 0 1 -16 16h-96a16 16 0 0 1 -16 -16v-96a16 16 0 0 1 16 -16z'
            f'M{x + 64} {y + 30}a34 34 0 1 0 0.001 0z"/>')


CASES = {
    # Control: opaque tiles, clear counters.
    "tiles_clear": f"<svg {SQ}>" + grid(lambda x, y: tile(x, y, "#1f2937")) + "</svg>",
    # A translucent lens in every counter: the lens is punched out of nothing, the counter
    # is the lens's outline.
    "tiles_lens": f"<svg {SQ}>" + grid(lambda x, y: tile(x, y, "#1f2937")
                                       + f'<circle cx="{x + 64}" cy="{y + 64}" r="34" '
                                         'fill="#2b6cb0" fill-opacity="0.5"/>') + "</svg>",
    # Translucent tiles: every face of the shape is written with fill-opacity.
    "tiles_glass": f"<svg {SQ}>" + grid(lambda x, y: tile(x, y, "#2b6cb0", ' fill-opacity="0.5"')) + "</svg>",
    # Translucent rings over one opaque plate: each ring is punched out of the plate.
    "rings_on_plate": f'<svg {SQ}><rect x="10" y="10" width="492" height="492" rx="40" '
                      'fill="#c9754a"/>' + grid(lambda x, y: f'<circle cx="{x + 64}" cy="{y + 64}" '
                                                            'r="44" fill="none" stroke="#ffffff" '
                                                            'stroke-opacity="0.55" stroke-width="20"/>')
                      + "</svg>",
    # A fade in every tile.
    "tiles_fade": f'<svg {SQ}><defs><linearGradient id="f" x1="0" y1="0" x2="0" y2="1">'
                  '<stop offset="0" stop-color="#1f2937" stop-opacity="1"/>'
                  '<stop offset="1" stop-color="#1f2937" stop-opacity="0.15"/></linearGradient></defs>'
                  + grid(lambda x, y: tile(x, y, "url(#f)")) + "</svg>",
}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(ROOT / "target/release/inkvec.exe"))
    ap.add_argument("--args", default="", help="extra CLI flags for both runs")
    ap.add_argument("--work", default=str(ROOT / "out/harmonize_alpha"))
    a = ap.parse_args()
    work = Path(a.work)
    work.mkdir(parents=True, exist_ok=True)
    extra = shlex.split(a.args)

    print(f"exe {a.exe}  args [{a.args.strip()}]")
    print(f"{'case':16} {'':3} {'paths':>5} {'white':>7} {'dark':>7} {'alpha':>7}")
    worst = 0.0
    for name, svg in CASES.items():
        png = work / f"{name}.png"
        png.write_bytes(render.render_to_png(svg, SIZE, SIZE))
        truth = rgba(svg)
        row = {}
        for tag, flags in (("on", []), ("off", ["--no-harmonize"])):
            out = work / f"{name}.{tag}.svg"
            p = subprocess.run([a.exe, str(png), "-o", str(out), "-q", *extra, *flags],
                               capture_output=True, text=True)
            if p.returncode != 0 or not out.is_file():
                print(f"{name:16} {tag:3} FAILED: {p.stderr.strip()[:200]}")
                continue
            traced = out.read_text(encoding="utf-8")
            trace = rgba(traced)
            one = np.ones(3, np.float32)
            s = (float(np.abs(over(trace, one) - over(truth, one)).mean()),
                 float(np.abs(over(trace, DARK) - over(truth, DARK)).mean()),
                 float(np.abs(trace[..., 3] - truth[..., 3]).mean()))
            row[tag] = s
            paths = traced.count("<path") + traced.count("<rect") + traced.count("<circle")
            print(f"{name:16} {tag:3} {paths:5} {s[0]:7.4f} {s[1]:7.4f} {s[2]:7.4f}")
        if "on" in row and "off" in row:
            worst = max(worst, row["on"][1] - row["off"][1], row["on"][2] - row["off"][2])
    print(f"worst dark/alpha cost of harmonizing: {worst:+.4f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
