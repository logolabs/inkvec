"""Features a few pixels across: do they survive, at the right size, in the right ink?

Below about three pixels a feature is mostly anti-aliasing, and a tracer has two ways to
lose it. It can drop it — the `--min-area` floor, or a smoother that treats it as noise.
Or it can keep it and pay for the lost contrast with size: a two-pixel disc comes back
2.2 px wide in a colour halfway to the backdrop, which is the right *mass* of ink and the
wrong feature. Size is therefore measured from area rather than from a fitted radius (a
dropped feature has no radius to fit), and measured twice, over coverage and over ink.

The last case is the one that decides whether a registered-trade-mark sign is readable at
icon size: a ring with a bar across it, nine pixels wide, whose two counters are about
half a pixel of ink apart.
"""
from __future__ import annotations

import math

from _common import (Case, Check, alpha_area, components, holes, ink_area,
                     painted_over_clear, svg_doc)

INK = "#204080"
INK_RGB = (0x20 / 255, 0x40 / 255, 0x80 / 255)
CX, CY = 64.27, 63.73  # off-grid centre: an integer centre is the case that works anyway

SCALE = 8  # 1024 px: ~60 render pixels across the smallest feature under test

# 0.15 px on a radius-1 disc is a 15 % change of area, which is the point at which a dot
# in a glyph starts to look like a different weight. It is also 3x the 0.05 px worst case
# the 0.1 px output grid can impose.
SIZE_LIMIT = 0.15

RADII = (1.0, 1.5, 2.0, 3.0, 5.0)


def _feature(name: str, what: str, body: str, truth_area: float, size_of, unit: str) -> Case:
    """`size_of` turns an area into the length the case is really about."""

    def check(t):
        out = t.out_rgba(SCALE)
        ink = ink_area(out, INK_RGB, SCALE)
        cov = alpha_area(out, SCALE)
        want = size_of(truth_area)
        return [
            Check(f"ink_{unit}_err_px", abs(size_of(ink) - want), SIZE_LIMIT, "<",
                  "0.15 px = 15 % of the smallest feature, and 3x the output coordinate grid"),
            Check(f"cov_{unit}_err_px", abs(size_of(cov) - want), SIZE_LIMIT, "<",
                  "same budget read over coverage; splits 'moved' from 'faded'"),
            Check("survives", cov / truth_area, 0.5, ">=",
                  "half the truth's area: below this the feature is gone, not merely wrong"),
        ]

    return Case(name=name, what=what, svg=svg_doc(128, body), check=check)


CASES = []
for r in RADII:
    CASES.append(_feature(
        f"disc_r{r:g}", f"a {r:g} px disc survives at the right size",
        f'<circle cx="{CX}" cy="{CY}" r="{r}" fill="{INK}"/>',
        math.pi * r * r, lambda a: math.sqrt(max(a, 0.0) / math.pi), "radius"))
for r in RADII:
    CASES.append(_feature(
        f"square_r{r:g}", f"a {2 * r:g} px square survives at the right size",
        f'<rect x="{CX - r}" y="{CY - r}" width="{2 * r}" height="{2 * r}" fill="{INK}"/>',
        (2 * r) ** 2, lambda a: math.sqrt(max(a, 0.0)) / 2, "halfside"))


# A 9 px ring with a 1.2 px bar across it: the structure of an (R) mark. Written as one
# even-odd path (outer disc minus inner disc) plus the bar, so the artist's figure is one
# connected region with exactly two counters.
_RB_OUTER, _RB_INNER, _RB_BAR = 4.5, 3.0, 1.2
RING_BAR = svg_doc(128, (
    f'<path fill-rule="evenodd" fill="{INK}" d="'
    f'M{CX - _RB_OUTER},{CY} a{_RB_OUTER},{_RB_OUTER} 0 1,0 {2 * _RB_OUTER},0 '
    f'a{_RB_OUTER},{_RB_OUTER} 0 1,0 {-2 * _RB_OUTER},0 Z '
    f'M{CX - _RB_INNER},{CY} a{_RB_INNER},{_RB_INNER} 0 1,0 {2 * _RB_INNER},0 '
    f'a{_RB_INNER},{_RB_INNER} 0 1,0 {-2 * _RB_INNER},0 Z"/>'
    f'<rect x="{CX - _RB_OUTER}" y="{CY - _RB_BAR / 2}" width="{2 * _RB_OUTER}" '
    f'height="{_RB_BAR}" fill="{INK}"/>'))


def _ring_bar(t):
    truth, out = t.truth_rgba(SCALE), t.out_rgba(SCALE)
    return [
        Check("counters", holes(out), holes(truth), "==",
              "the artist's own counter count, read off the truth render at 8x"),
        Check("components", components(out), components(truth), "==",
              "the glyph is one connected region; shattering it is the same defect twice"),
        Check("painted_clear_px2", painted_over_clear(truth, out, SCALE), 1.0, "<",
              "one square pixel of invented ink; a counter painted white instead of left "
              "open costs several and only shows on a coloured ground"),
    ]


CASES.append(Case(
    name="glyph_ring_bar",
    what="a 9 px ring with a bar keeps both of its counters",
    svg=RING_BAR,
    check=_ring_bar,
))
