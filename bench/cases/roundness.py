"""Is a circle round, and is it written as a circle?

Two separate questions, so two separate cases. A boundary can sit within a hundredth of a
pixel of a circle and still be twenty-four cubic control points a designer cannot drag;
and a `<circle>` element can be emitted with the wrong radius. Only the first is a
fidelity defect, so only the first carries a tight threshold.
"""
from __future__ import annotations

import numpy as np

from _common import Case, Check, svg_doc

INK = "#204080"
CX, CY, R = 64.27, 63.73, 30.0

DISC = svg_doc(128, f'<circle cx="{CX}" cy="{CY}" r="{R}" fill="{INK}"/>')


def _fit_circle(p: np.ndarray):
    """Algebraic fit x^2+y^2 = 2ax + 2by + c, as in bench/regularity.py."""
    A = np.column_stack([2 * p[:, 0], 2 * p[:, 1], np.ones(len(p))])
    sol, *_ = np.linalg.lstsq(A, (p ** 2).sum(1), rcond=None)
    cx, cy, c = sol
    r2 = c + cx * cx + cy * cy
    if r2 <= 0:
        return None
    return float(cx), float(cy), float(np.sqrt(r2))


def _roundness(t):
    rings = [r for r in t.out_rings() if len(r) >= 8]
    if not rings:
        return [Check("max_dev_px", float("nan"), 0.05, "<", "no boundary was emitted")]
    p = max(rings, key=len)
    fit = _fit_circle(p)
    if fit is None:
        return [Check("max_dev_px", float("nan"), 0.05, "<", "boundary does not fit a circle")]
    cx, cy, r = fit
    dev = float(np.abs(np.hypot(p[:, 0] - cx, p[:, 1] - cy) - r).max())
    return [
        # A cubic approximation to a quarter circle is off by ~2.7e-4 of the radius, so
        # 0.05 px on a radius-30 disc is six times what an honest Bezier fit costs.
        Check("max_dev_px", dev, 0.05, "<",
              "0.05 px = 6x the error of a correct 4-cubic circle at this radius"),
        Check("radius_err_px", abs(r - R), 0.15, "<",
              "the same 0.15 px size budget the small-feature cases use"),
        Check("centre_err_px", float(np.hypot(cx - CX, cy - CY)), 0.15, "<",
              "a circle in the wrong place is not a rounder circle"),
    ]


def _primitive(t):
    prim = sum(1 for s in t.out_shapes
               if s.kind in ("Circle", "Ellipse") or s.seg_types.get("Arc", 0) >= 3)
    return [Check("circle_or_arc_shapes", prim, 1, ">=",
                  "a disc is three numbers; anything else is a document nobody can edit")]


CASES = [
    Case(name="circle_roundness", what="a traced disc is round to a twentieth of a pixel",
         svg=DISC, check=_roundness),
    Case(name="circle_primitive", what="a disc is emitted as a circle or arcs",
         svg=DISC, check=_primitive),
]
