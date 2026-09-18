"""Where a straight edge lands, and how many segments it costs.

An edge is the one thing a tracer must get right before anything else is worth
measuring, and it is the one thing a whole-icon score hides: half a pixel of drift on
every contour reads as a small colour error and nothing else. Here the truth is a
polygon we wrote down, so the displacement is the distance from the emitted boundary to
lines whose position we know exactly.

Offsets are deliberately non-integer. An edge on the pixel grid is the case a tracer
gets right by accident.
"""
from __future__ import annotations

import math

from _common import Case, Check, edge_displacement, poly_attr, svg_doc

INK = "#204080"

# A perfect fit still lands on inkvec's 0.1 px output grid, so ~0.025 px of the budget is
# spent before the fitter does anything. 0.05 px leaves the fitter the other half: it is
# the point at which a 4x zoom starts to show the edge in the wrong place.
EDGE_LIMIT = 0.05

AXIS_RECT = [(20.37, 24.63), (90.63, 24.63), (90.63, 84.82), (20.37, 84.82)]


def _rotated_square(cx, cy, half, deg):
    a = math.radians(deg)
    c, s = math.cos(a), math.sin(a)
    return [(cx + c * dx - s * dy, cy + s * dx + c * dy)
            for dx, dy in ((-half, -half), (half, -half), (half, half), (-half, half))]


DIAG_SQUARE = _rotated_square(64.27, 63.73, 38.0, 30.0)

# 1:6 rise over run. The level set of this edge is an 18-step staircase; a tracer that
# follows the staircase spends a segment per step instead of one per edge.
STAIR = [(10.27, 100.43), (118.27, 82.43), (118.27, 118.0), (10.27, 118.0)]


def _placement(poly):
    def check(t):
        return [Check("mean_edge_disp_px", edge_displacement(t, poly), EDGE_LIMIT, "<",
                      "0.05 px = the 0.1 px coordinate grid plus an equal fitting budget")]
    return check


def _staircase(t):
    disp = edge_displacement(t, STAIR)
    return [
        # A budget rather than a ratio: segment counts depend on whether a ring closes
        # with an explicit segment or with Z, and the truth here is a quadrilateral.
        Check("segments", t.out_segments, 12, "<=",
              "3x the truth's four edges; a per-step trace of an 18-step edge costs ~36"),
        Check("mean_edge_disp_px", disp, EDGE_LIMIT, "<",
              "same edge-placement budget as the other straight edges"),
    ]


CASES = [
    Case(
        name="edge_axis",
        what="axis-aligned edge lands within a twentieth of a pixel",
        svg=svg_doc(128, f'<polygon points="{poly_attr(AXIS_RECT)}" fill="{INK}"/>'),
        check=_placement(AXIS_RECT),
    ),
    Case(
        name="edge_diag30",
        what="30-degree edge lands within a twentieth of a pixel",
        svg=svg_doc(128, f'<polygon points="{poly_attr(DIAG_SQUARE)}" fill="{INK}"/>'),
        check=_placement(DIAG_SQUARE),
    ),
    Case(
        name="staircase_1_6",
        what="a 1:6 edge is a line, not eighteen steps",
        svg=svg_doc(128, f'<polygon points="{poly_attr(STAIR)}" fill="{INK}"/>'),
        check=_staircase,
    ),
]
