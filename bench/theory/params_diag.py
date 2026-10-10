"""Where do the parameters go? Inkvec's SVG against the artist's, by element and segment type.

For every icon of a set: the gate's parameter count (`svgmodel`: 2 per line, 6 per cubic,
7 per arc, a fixed count per primitive) of the trace and of the artist's file, split by what
spends them, and whether the artist drew with strokes (a stroked centre line is one path;
traced, it is an outline of two sides and two caps).

    python3 bench/theory/params_diag.py --exe target/release/inkvec --tier 512ss
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tempfile
from collections import Counter, defaultdict
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))


def breakdown(svg: str) -> Counter:
    from inkvec_bench import svgmodel
    doc = svgmodel.parse(svg)
    c: Counter = Counter()
    for e in doc.elements:
        if e.is_primitive:
            c["prim:" + e.kind] += e.params
        else:
            for name, n in e.seg_types.items():
                c["seg:" + name] += {"Line": 2, "QuadraticBezier": 4, "CubicBezier": 6,
                                     "Arc": 7}.get(name, 0) * n
    return c


def one(args):
    exe, it, tier, mode, td, extra = args
    import svgeval
    svgeval.set_tier(tier)
    png, gt = svgeval.item_paths(it)
    out = Path(td) / f"{it['corpus']}__{it['stem']}.svg"
    subprocess.run([exe, str(png), "-o", str(out), "--quiet", "--mode", mode, *extra],
                   check=True, capture_output=True)
    gsvg = gt.read_text(encoding="utf-8")
    stroked = bool(re.search(r'stroke-width|stroke="(?!none)', gsvg))
    return dict(icon=f"{it['corpus']}/{it['stem']}", family=it["corpus"],
                ours=breakdown(out.read_text(encoding="utf-8")), human=breakdown(gsvg),
                stroked=stroked)


def main() -> None:
    import svgeval
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(ROOT / "target/release/inkvec"))
    ap.add_argument("--tier", default="512ss")
    ap.add_argument("--mode", default="quality")
    ap.add_argument("--set", default="screen")
    ap.add_argument("--extra", default="")
    a = ap.parse_args()
    items = svgeval.load_sets()[a.set]
    with tempfile.TemporaryDirectory() as td, ProcessPoolExecutor(4) as pool:
        rows = list(pool.map(one, [(a.exe, it, a.tier, a.mode, td, a.extra.split()) for it in items]))
    fams = sorted({r["family"] for r in rows})
    print(f"{a.mode} {a.tier}: parameters, inkvec / artist (family means of per-icon ratios)")
    for f in fams + ["ALL"]:
        rs = rows if f == "ALL" else [r for r in rows if r["family"] == f]
        ratio = np.mean([sum(r["ours"].values()) / max(1, sum(r["human"].values())) for r in rs])
        st = sum(r["stroked"] for r in rs)
        ours, hum = Counter(), Counter()
        for r in rs:
            ours.update(r["ours"])
            hum.update(r["human"])
        to, th = sum(ours.values()), sum(hum.values())
        def fmt(c, t):
            return ", ".join(f"{k.split(':')[1]} {100 * v / t:.0f}%" for k, v in c.most_common(5))
        print(f"{f:15s} ratio {ratio:5.2f}  stroked {st:3d}/{len(rs):3d}  total {to:6d} vs {th:6d}")
        print(f"{'':15s}   inkvec: {fmt(ours, to)}")
        print(f"{'':15s}   artist: {fmt(hum, th)}")
    for flag in (True, False):
        rs = [r for r in rows if r["stroked"] == flag]
        if rs:
            print(f"artist {'stroked' if flag else 'filled '}: {len(rs)} icons, mean ratio "
                  f"{np.mean([sum(r['ours'].values()) / max(1, sum(r['human'].values())) for r in rs]):.2f}")


if __name__ == "__main__":
    main()
