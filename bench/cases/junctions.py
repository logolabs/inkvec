"""Regions that share an edge: does the seam hold, and do the corners agree?

When two regions are traced independently, each fits its own copy of the shared boundary
and the two copies disagree by a fraction of a pixel. The result is either overdraw
(harmless, invisible) or a hairline of background showing between them (a defect a
designer sees the moment the icon goes on a dark ground). Where three or four regions
meet, the same disagreement moves the junction point and rounds every corner that touches
it.

The seam reading is background *area* that shows through where the artist's document is
opaque, converted to an equivalent hairline width by dividing by the seam's length. Two
abutting paths always produce a little of it from anti-aliasing alone, so the truth's own
value is subtracted and the threshold is set above what the remaining phase difference
can account for.
"""
from __future__ import annotations

import math

import numpy as np

from _common import Case, Check, dist_to_polygon, poly_attr, svg_doc

A, B, C = "#204080", "#c04030", "#3f8f4f"
SIZE = 128
SCALE = 8  # 1024 px; the AA seam artefact this has to see past scales as 1/SCALE

# Two abutting paths leave f*(1-f)/SCALE px of show-through per px of seam from AA alone,
# at most 0.031 at SCALE=8, and truth and output sit at different sub-pixel phases. 0.05
# is therefore the smallest limit that cannot fire on phase alone; a real hairline — the
# defect this exists to catch — is 0.1 px or more.
GAP_LIMIT = 0.05

# Half a source pixel of outright background. AA at a four-way meeting costs ~0.02 px^2;
# a 0.1 px hairline down a 128 px seam costs 12.8.
BLEED_LIMIT = 0.5

JX, JY = 64.27, 63.73  # the junction, deliberately off the pixel grid


def _seam_checks(seam_len: float):
    def measure(t):
        truth, out = t.truth_rgba(SCALE), t.out_rgba(SCALE)
        # Ignore a two-pixel margin: the canvas border is not a seam.
        m = slice(2, -2)
        ta, oa = truth[m, m, 3], out[m, m, 3]
        gap = (float((1.0 - oa).sum() - (1.0 - ta).sum()) / (SCALE * SCALE)) / seam_len
        bleed = float(((ta >= 0.99) & (oa < 0.5)).sum()) / (SCALE * SCALE)
        return gap, bleed
    return measure


def _shape_area(s) -> float:
    tot = 0.0
    for r in s.rings:
        x, y = r[:, 0], r[:, 1]
        tot += abs(float(np.dot(x, np.roll(y, -1)) - np.dot(y, np.roll(x, -1)))) / 2
    return tot


def _abut(name, what, body, seam_len):
    measure = _seam_checks(seam_len)

    def check(t):
        gap, bleed = measure(t)
        return [
            Check("seam_gap_px", gap, GAP_LIMIT, "<",
                  "above the 0.031 px anti-aliasing phase difference at 8x, below a real hairline"),
            Check("bleed_px2", bleed, BLEED_LIMIT, "<",
                  "half a source pixel; a 0.1 px hairline down this seam would cost ~13"),
        ]

    return Case(name=name, what=what, svg=svg_doc(SIZE, body), check=check)


def _junction(name, what, body, seam_len):
    measure = _seam_checks(seam_len)

    def check(t):
        gap, bleed = measure(t)
        # Every region that meets at the junction owns a corner there. The one whose
        # boundary strays furthest is the one that moved the junction.
        big = [s for s in t.out_shapes if _shape_area(s) >= 200.0]
        worst = _junction_error(t, big)
        return [
            Check("junction_err_px", worst, 0.2, "<",
                  "0.2 px = 2x the output coordinate grid; beyond it the corner is visibly cut"),
            Check("seam_gap_px", gap, GAP_LIMIT, "<",
                  "above the 0.031 px anti-aliasing phase difference at 8x, below a real hairline"),
            Check("bleed_px2", bleed, BLEED_LIMIT, "<",
                  "half a source pixel; a 0.1 px hairline down these seams would cost ~13"),
        ]

    return Case(name=name, what=what, svg=svg_doc(SIZE, body), check=check)


def _junction_error(t, big) -> float:
    """The furthest any large region's boundary sits from the true junction point."""
    if not big:
        return float("nan")
    q = np.asarray([[JX, JY]], dtype=float)
    worst = 0.0
    for s in big:
        d = min((float(dist_to_polygon(q, r)[0]) for r in s.rings), default=float("inf"))
        worst = max(worst, d)
    return worst


# --- two and three regions abutting on vertical seams -------------------------------
TWO = (f'<rect x="0" y="0" width="53.67" height="{SIZE}" fill="{A}"/>'
       f'<rect x="53.67" y="0" width="{SIZE - 53.67}" height="{SIZE}" fill="{B}"/>')
THREE = (f'<rect x="0" y="0" width="41.27" height="{SIZE}" fill="{A}"/>'
         f'<rect x="41.27" y="0" width="{87.63 - 41.27}" height="{SIZE}" fill="{B}"/>'
         f'<rect x="87.63" y="0" width="{SIZE - 87.63}" height="{SIZE}" fill="{C}"/>')

# --- a three-way (Y) junction --------------------------------------------------------
_DIRS = [30.0, 150.0, 270.0]  # screen degrees, y down: 120 apart, none axis-aligned


def _ray_len(deg: float) -> float:
    dx, dy = math.cos(math.radians(deg)), math.sin(math.radians(deg))
    ts = []
    for d, p, lo, hi in ((dx, JX, 0.0, SIZE), (dy, JY, 0.0, SIZE)):
        if abs(d) > 1e-12:
            ts += [t for t in ((lo - p) / d, (hi - p) / d) if t > 0]
    return min(ts)


def _sector(a: float, b: float, r: float = 400.0) -> list[tuple[float, float]]:
    """The wedge from angle a to angle b, taken far enough out to leave the canvas."""
    steps = [a + (b - a) * i / 4 for i in range(5)]
    return [(JX, JY)] + [(JX + r * math.cos(math.radians(s)), JY + r * math.sin(math.radians(s)))
                         for s in steps]


TRI = "".join(
    f'<polygon points="{poly_attr(_sector(_DIRS[i], _DIRS[i] + 120.0))}" fill="{f}"/>'
    for i, f in enumerate((A, B, C)))
TRI_SEAM = sum(_ray_len(d) for d in _DIRS)

# --- a four-way checkerboard meeting -------------------------------------------------
QUAD = (f'<rect x="0" y="0" width="{JX}" height="{JY}" fill="{A}"/>'
        f'<rect x="{JX}" y="0" width="{SIZE - JX}" height="{JY}" fill="{B}"/>'
        f'<rect x="0" y="{JY}" width="{JX}" height="{SIZE - JY}" fill="{B}"/>'
        f'<rect x="{JX}" y="{JY}" width="{SIZE - JX}" height="{SIZE - JY}" fill="{A}"/>')

CASES = [
    _abut("abut_two", "two regions share an edge without a hairline", TWO, SIZE),
    _abut("abut_three", "three regions share two edges without a hairline", THREE, 2 * SIZE),
    _junction("junction_tri", "three regions meet at one point", TRI, TRI_SEAM),
    _junction("junction_quad", "a four-way checkerboard meeting holds together", QUAD, 2 * SIZE),
]
