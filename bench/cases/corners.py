"""Corners: does the emitted boundary reach the corner, or cut it off?

The level set of an anti-aliased corner is rounded — the coverage contour turns about a
pixel inside the true vertex — so a tracer that follows the level set faithfully rounds
every corner in the image by construction. It is the classic failure, it is worst on
acute angles, and it costs almost nothing on a colour score because the missing area is
one triangle a pixel across.

The reading is the distance from the corner we authored to the nearest point on the
emitted boundary: a boundary that passes through the vertex reads zero, and one that
chamfers it reads the depth of the chamfer.
"""
from __future__ import annotations

import math

from _common import Case, Check, dist_to_output, poly_attr, svg_doc

INK = "#204080"

# 0.15 px is three times the worst error the 0.1 px output grid can impose, and about a
# sixth of the ~1 px inset the coverage contour has at a right angle.
CORNER_LIMIT = 0.15

SQUARE = [(24.37, 20.63), (96.73, 20.63), (96.73, 104.27), (24.37, 104.27)]


def _wedge(apex, angle_deg, arm=88.0):
    """A triangle whose apex carries exactly `angle_deg`, opening downwards."""
    half = math.radians(angle_deg) / 2
    ax, ay = apex
    return [apex,
            (ax - arm * math.sin(half), ay + arm * math.cos(half)),
            (ax + arm * math.sin(half), ay + arm * math.cos(half))]


WEDGE_45 = _wedge((64.27, 22.63), 45.0)
WEDGE_20 = _wedge((64.27, 22.63), 20.0)


def _corners(points, label):
    def check(t):
        worst = max(dist_to_output(t, p) for p in points)
        return [Check(f"{label}_err_px", worst, CORNER_LIMIT, "<",
                      "0.15 px = 3x the output grid; the level set rounds a corner ~1 px inside")]
    return check


CASES = [
    Case(name="corner_right", what="right-angle corners are not chamfered",
         svg=svg_doc(128, f'<polygon points="{poly_attr(SQUARE)}" fill="{INK}"/>'),
         check=_corners(SQUARE, "worst_corner")),
    Case(name="corner_45", what="a 45-degree corner reaches its vertex",
         svg=svg_doc(128, f'<polygon points="{poly_attr(WEDGE_45)}" fill="{INK}"/>'),
         check=_corners(WEDGE_45[:1], "apex")),
    Case(name="corner_acute", what="a 20-degree corner reaches its vertex",
         svg=svg_doc(128, f'<polygon points="{poly_attr(WEDGE_20)}" fill="{INK}"/>'),
         check=_corners(WEDGE_20[:1], "apex")),
]
