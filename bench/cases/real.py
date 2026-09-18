"""Three real icons, each held to one of the synthetic properties.

The synthetic cases are clean by construction, which is their value and their limit: a
tracer can be right on a lone 2 px bar and wrong on a bar that has a corner, a neighbour
and a counter within four pixels of it. These three take the same three readings —
counters, symmetry, single ink — off assets an artist actually drew, so a synthetic case
that passes while its real counterpart fails says the isolation is hiding something.

Three, and no more. This suite exists because the 980-icon run is slow; a corpus sweep
that crept in here would put it back.

They need the rendered corpus (`python bench/build_corpus_v2.py --no-fetch`). Without it
they are skipped and named in NOTES rather than failing.
"""
from __future__ import annotations

import numpy as np

from _common import (Case, Check, components, holes, ink_area,
                     painted_over_clear, symmetry)
from inkvec_bench import config

SCALE = 4
CASES: list[Case] = []
NOTES: list[str] = []


def _paths(family: str, stem: str):
    png = config.CORPUS_RASTER_DIR / family / "128ss" / f"{stem}.png"
    svg = config.CORPUS_SVG_DIR / family / f"{stem}.svg"
    return (png, svg) if png.exists() and svg.exists() else (None, None)


def _add(name, family, stem, what, check):
    png, svg = _paths(family, stem)
    if png is None:
        NOTES.append(f"{name}: {family}/{stem} not in the corpus; skipped")
        return
    CASES.append(Case(name=name, what=what, png_path=png, truth_path=svg, check=check))


# "abc" is three letterforms at 128 px: counters an eighth of the glyph across, and the
# structure a viewer reads the mark by. The synthetic ring-and-bar is this case with
# everything else taken away.
def _counters(t):
    truth, out = t.truth_rgba(SCALE), t.out_rgba(SCALE)
    return [
        Check("counters", holes(out), holes(truth), "==",
              "the artist's own counter count; losing one turns an 'a' into an 'o'"),
        Check("components", components(out), components(truth), "==",
              "the artist's own part count, neither shattered nor fused"),
        Check("painted_clear_px2", painted_over_clear(truth, out, SCALE), 1.0, "<",
              "one square pixel of invented ink; painting a counter white rather than "
              "leaving it open reads correct on white and wrong on anything else"),
    ]


_add("real_glyph_counters", "material-icons", "abc",
     "a real glyph keeps its counters and its parts", _counters)


# A stroke icon the artist drew symmetric to a millionth. Whatever residual comes back is
# ours: two halves of the same contour fitted independently and disagreeing.
def _symmetry(t):
    return [
        Check("out_residual", symmetry(t.out_rgba(SCALE))["v"], 1e-4, "<",
              "the same limit the synthetic mirrors use"),
        Check("truth_residual", symmetry(t.truth_rgba(SCALE))["v"], 1e-4, "<",
              "guards the choice of icon: the artist's own residual is ~1e-6"),
    ]


_add("real_symmetry", "lucide", "beaker",
     "a symmetric stroke icon comes back symmetric", _symmetry)


# A one-ink brand mark of thin letterforms. Thin ink is what a tracer dilutes, and here
# there is exactly one colour it could dilute towards, so the reading is unambiguous.
def _single_ink(t):
    truth, out = t.truth_rgba(SCALE), t.out_rgba(SCALE)
    opaque = truth[..., 3] > 0.9
    px = (truth[..., :3][opaque] * 255).astype(np.uint8)
    vals, counts = np.unique(px.reshape(-1, 3), axis=0, return_counts=True)
    ink = tuple(vals[counts.argmax()] / 255.0)
    ta = ink_area(truth, ink, SCALE)
    oa = ink_area(out, ink, SCALE)
    return [
        Check("ink_area_err", abs(oa - ta) / ta if ta else float("nan"), 0.10, "<",
              "10 % of the artist's ink is a visible change of weight on a wordmark"),
    ]


_add("real_single_ink", "simple-icons", "nokia",
     "a one-ink wordmark keeps all of its ink", _single_ink)
