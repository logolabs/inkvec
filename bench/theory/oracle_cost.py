"""Search error or model error? Each trace against the artist's own file, on the input's pixels.

The optimal tracer (`docs/theory/optimal.md` §2) writes the description that best trades
its fit to the input's pixels against its size. The artist's file is one description of
those pixels, so it bounds what the search could have found:

* `search`   the artist's file is no larger *and* fits the pixels no worse: a better
             description exists, and the tracer missed it. More moves, wider search.
* `ahead`    the trace is no larger and fits no worse: the tracer is at least as good as
             the artist's file on its own objective.
* `bigger`   the trace is larger and fits better: it spends parameters on the pixels. Is
             that fit the picture or the renderer's anti-aliasing? The description-length
             verdict (`mdl`) says which at the tracer's price per parameter.
* `smaller`  the trace is smaller and fits worse: it simplified. If the gate's dE00 against
             the artist's file is also worse, the objective preferred the wrong description:
             a model error (the prior or the loss), not a search error.

The fit is the sum of squared differences between the input raster and each SVG rendered at
the input's size, composited over white and over black (so transparency counts). The
description-length verdict prices a parameter at the tracer's `λ = ln(extent / 0.1)` nats
and measures the squared differences in units of the artist file's own residual per
boundary pixel, the renderer floor: what two renderers of the same file disagree by.

    python3 bench/theory/oracle_cost.py --exe target/release/inkvec --tier 512ss
"""

from __future__ import annotations

import argparse
import json
import math
import subprocess
import sys
import tempfile
from collections import Counter
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))

PRECISION = 0.1  # px: inkvec's default --precision
TIE = 0.02  # relative difference in fit treated as equal


def sse(src: np.ndarray, svg: str) -> tuple[float, np.ndarray]:
    """Squared difference of `svg` rendered at the input's size against the input, over
    white and over black, and the per-pixel map. An opaque input (a logo flattened onto a
    white page, the gate's `512ssop`) is compared over white only, as it was made."""
    from inkvec_bench import render
    h, w = src.shape[:2]
    r = render.render(svg, w, h)
    m = np.zeros((h, w))
    opaque = float(src[..., 3].min()) >= 0.999
    grounds = ((1.0, 1.0, 1.0),) if opaque else ((1.0, 1.0, 1.0), (0.0, 0.0, 0.0))
    for bg in grounds:
        d = render.composite(r, bg) - render.composite(src, bg)
        m += (d * d).sum(-1)
    return float(m.sum()), m


def one(args):
    exe, it, tier, mode, td, extra = args
    import svgeval
    from inkvec_bench import render, svgmodel
    svgeval.set_tier(tier)
    png, gt = svgeval.item_paths(it)
    out = Path(td) / f"{it['corpus']}__{it['stem']}.svg"
    subprocess.run([exe, str(png), "-o", str(out), "--quiet", "--mode", mode, *extra],
                   check=True, capture_output=True)
    src = render.load_rgba(png)
    ours_svg = out.read_text(encoding="utf-8")
    art_svg = gt.read_text(encoding="utf-8")
    s_ours, _ = sse(src, ours_svg)
    s_art, m_art = sse(src, art_svg)
    k_ours = svgmodel.parse(ours_svg).n_params
    k_art = svgmodel.parse(art_svg).n_params
    h, w = src.shape[:2]
    lam = math.log(max(max(w, h) / PRECISION, math.e))
    # The renderer floor: the artist file's own residual per pixel where it has any.
    hot = m_art > 1e-6
    sigma2 = float(m_art[hot].mean()) if hot.any() else 1e-6
    d_cost = 0.5 * (s_ours - s_art) / sigma2 + lam * (k_ours - k_art)
    tie = TIE * max(s_art, 1e-9)
    fit = "same" if abs(s_ours - s_art) <= tie else ("better" if s_ours < s_art else "worse")
    if k_ours <= k_art and fit != "worse":
        cls = "ahead"
    elif k_ours >= k_art and fit != "better":
        cls = "search"
    elif k_ours > k_art:
        cls = "bigger"
    else:
        cls = "smaller"
    out.unlink(missing_ok=True)
    return dict(icon=f"{it['corpus']}/{it['stem']}", family=it["corpus"], cls=cls,
                k_ours=k_ours, k_art=k_art, sse_ours=s_ours, sse_art=s_art,
                d_cost=d_cost, mdl="ours" if d_cost < 0 else "artist")


def main() -> None:
    import svgeval
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(ROOT / "target/release/inkvec"))
    ap.add_argument("--tier", default="512ss")
    ap.add_argument("--mode", default="quality")
    ap.add_argument("--set", default="screen")
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--extra", default="", help="further tracer arguments, space-separated")
    ap.add_argument("--json", default=None)
    a = ap.parse_args()
    items = svgeval.load_sets()[a.set]
    with tempfile.TemporaryDirectory() as td, ProcessPoolExecutor(a.workers) as pool:
        rows = list(pool.map(one, [(a.exe, it, a.tier, a.mode, td, a.extra.split())
                                   for it in items], chunksize=2))
    fams = sorted({r["family"] for r in rows})
    classes = ["ahead", "search", "bigger", "smaller"]
    print(f"{a.mode} {a.tier}: {len(rows)} icons; the trace against the artist's file on the "
          f"input's pixels")
    print(f"{'family':15s} " + " ".join(f"{c:>8s}" for c in classes) + "   MDL prefers ours")
    for f in fams + ["ALL"]:
        rs = rows if f == "ALL" else [r for r in rows if r["family"] == f]
        c = Counter(r["cls"] for r in rs)
        prefer = sum(r["mdl"] == "ours" for r in rs)
        print(f"{f:15s} " + " ".join(f"{c[k]:8d}" for k in classes) + f"   {prefer:3d}/{len(rs)}")
    print("\nclearest search errors (the artist's file smaller and fitting no worse), by "
          "parameters the trace spends over it:")
    se = sorted((r for r in rows if r["cls"] == "search"), key=lambda r: r["k_art"] - r["k_ours"])
    for r in se[:12]:
        print(f"  {r['icon']:45s} {r['k_ours']:5d} vs {r['k_art']:5d}  "
              f"fit {r['sse_ours']:.3g} vs {r['sse_art']:.3g}")
    if a.json:
        Path(a.json).write_text(json.dumps(rows, indent=1))


if __name__ == "__main__":
    main()
