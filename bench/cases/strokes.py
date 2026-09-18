"""Thin strokes: do they keep their shape, their width, and their colour?

A stroke two pixels wide is where the level set stops being a curve and starts being a
row of pixels, and it is where the tracer has historically produced triangular teeth
along both sides, or dissolved the ink into a pale core with two dark caps. Both are
invisible to a mean colour error and obvious at 4x.

Width is read twice, and the pair is the diagnosis:

* **coverage width** — alpha summed down each column. Says where the geometry is.
* **ink width** — the same sum restricted to pixels that still carry the artist's colour.
  A stroke traced as a blend of itself and the backdrop keeps its coverage and loses all
  of its ink, so only this reading moves.
"""
from __future__ import annotations

import numpy as np

from _common import (INK_TOL, Case, Check, components, poly_attr, ribbon_polygon,
                     svg_doc, total_turning)

INK = "#204080"
INK_RGB = (0x20 / 255, 0x40 / 255, 0x80 / 255)

BAR_X0, BAR_X1 = 14.0, 114.0
BAR_CY = 63.87  # off the pixel grid on purpose: a bar centred on a pixel is the easy case

SCALE = 4  # 512 px for a 128 px case: eight render rows across the thinnest bar

# A stroke that is out by 0.15 px at width 1.0 is out by 15 %, which reads as a visible
# change of weight; below that the eye is looking at the coordinate grid.
WIDTH_LIMIT = 0.15


def _widths(rgba, scale, rgb=None):
    """Stroke width per column, in source pixels, over the bar's interior."""
    a = rgba[..., 3].astype(np.float64)
    if rgb is not None:
        near = np.abs(rgba[..., :3] - np.asarray(rgb, np.float32)).max(-1) <= INK_TOL
        a = np.where(near, a, 0.0)
    # Trim three source pixels at each end so the caps never enter the average.
    c0, c1 = int((BAR_X0 + 3) * scale), int((BAR_X1 - 3) * scale)
    return a[:, c0:c1].sum(axis=0) / scale


def _ribbon_case(w: float) -> Case:
    body = (f'<rect x="{BAR_X0}" y="{BAR_CY - w / 2:.4f}" width="{BAR_X1 - BAR_X0}" '
            f'height="{w}" fill="{INK}"/>')

    def check(t):
        out = t.out_rgba(SCALE)
        cov = float(_widths(out, SCALE).mean())
        ink = float(_widths(out, SCALE, INK_RGB).mean())
        return [
            Check("ink_width_err_px", abs(ink - w), WIDTH_LIMIT, "<",
                  "0.15 px is 15 % of the thinnest stroke here, i.e. a visible weight change"),
            Check("cov_width_err_px", abs(cov - w), WIDTH_LIMIT, "<",
                  "same budget read over coverage; differs from ink only when the ink is diluted"),
            Check("components", components(out), 1, "==",
                  "one stroke in, one connected stroke out"),
        ]

    return Case(name=f"ribbon_w{w:g}", what=f"a {w:g} px stroke keeps its width and stays whole",
                svg=svg_doc(128, body), check=check)


# A 2 px stroke at 45 degrees: thin enough that the level set is ragged, long enough that
# every tooth costs segments. Authored as a filled quad so the truth is 4 edges and 2*pi
# of turning exactly.
SAW = ribbon_polygon(18.27, 109.73, 109.73, 18.27, 2.0)


def _sawtooth(t):
    truth_turn = total_turning([r for s in t.truth_shapes for r in s.rings])
    return [
        # One closed ring turns 2*pi however finely it is sampled, so this rises only
        # when the boundary doubles back — teeth — or when the stroke is shattered into
        # several rings that each pay 2*pi of their own. Both are the same defect.
        Check("turning_ratio", total_turning(t.out_rings()) / truth_turn, 2.0, "<",
              "2x the truth's 2*pi: room for cap detail, none for teeth or shards"),
        # Stated as a budget rather than a ratio: segment counts depend on whether a
        # ring closes with an explicit segment or with Z, and the truth is a quad.
        Check("segments", t.out_segments, 8, "<=",
              "2x the truth's four edges; a straight-sided quad needs no more"),
    ]


CASES = [_ribbon_case(w) for w in (1.0, 1.5, 2.0, 3.0)] + [
    Case(
        name="sawtooth_diag",
        what="a 2 px diagonal stroke comes back smooth, not toothed",
        svg=svg_doc(128, f'<polygon points="{poly_attr(SAW)}" fill="{INK}"/>'),
        check=_sawtooth,
    ),
]
