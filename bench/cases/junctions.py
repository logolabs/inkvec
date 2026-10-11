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
import re

import numpy as np

from _common import INK_TOL, Case, Check, poly_attr, render, svg_doc

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
        # visible corner strays furthest is the one that moved the junction.
        worst = _junction_error(_window(t.truth_svg), _window(t.out_svg))
        return [
            Check("junction_err_px", worst, 0.2, "<",
                  "0.2 px = 2x the output coordinate grid; beyond it the corner is visibly cut"),
            Check("seam_gap_px", gap, GAP_LIMIT, "<",
                  "above the 0.031 px anti-aliasing phase difference at 8x, below a real hairline"),
            Check("bleed_px2", bleed, BLEED_LIMIT, "<",
                  "half a source pixel; a 0.1 px hairline down these seams would cost ~13"),
        ]

    return Case(name=name, what=what, svg=svg_doc(SIZE, body), check=check)


# The junction is read from a render of the four source pixels about it, at 64 samples
# per pixel: on the whole image at SCALE the nearest sample inside a 120-degree corner can
# sit 0.2 px from its apex, the size of the limit itself.
WIN, WIN_RES = 2.0, 64


def _window(svg: str) -> np.ndarray:
    """The square of side 2*WIN about the true junction, rendered at WIN_RES per pixel.

    The document is nested, as drawn on the image's pixel grid, in one whose viewBox is
    the window, so the two frames (the truth's grid, inkvec's pixel centres) stay the
    document's own business.
    """
    n = int(2 * WIN * WIN_RES)
    svg = re.sub(r"<\?xml[^>]*\?>", "", svg).strip()
    m = re.search(r"<svg\b[^>]*>", svg)
    if m is None:  # no output: no region reaches the junction
        return np.zeros((n, n, 4), dtype=np.float32)
    tag = re.sub(r'\s(?:x|y|width|height)="[^"]*"', "", m.group(0))
    tag = tag[:-1] + f' x="0" y="0" width="{SIZE}" height="{SIZE}">'
    outer = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{n}" height="{n}" '
             f'viewBox="{JX - WIN} {JY - WIN} {2 * WIN} {2 * WIN}">'
             f'{svg[:m.start()]}{tag}{svg[m.end():]}</svg>')
    return render.render(outer, n, n)


def _regions(rgba: np.ndarray) -> list[tuple[int, np.ndarray]]:
    """Each ink's 4-connected areas of its own colour, a quarter pixel or more: (ink, mask).

    Read from a render, not from the paths, because a path is not what shows. A face drawn
    whole beneath the faces painted over it (its covered part completed) has no corner at
    the junction of its own; its visible corner is where its covers' edges meet. Blended
    pixels match no ink, so two areas of one colour meeting only at a point stay apart.
    """
    from scipy import ndimage

    out = []
    for k, ink in enumerate((A, B, C)):
        rgb = np.asarray([int(ink[i:i + 2], 16) / 255.0 for i in (1, 3, 5)], dtype=np.float32)
        near = (np.abs(rgba[..., :3] - rgb).max(-1) <= INK_TOL) & (rgba[..., 3] >= 0.99)
        lab, n = ndimage.label(near)
        for i in range(1, n + 1):
            m = lab == i
            if m.sum() >= 0.25 * WIN_RES * WIN_RES:
                out.append((k, m))
    return out


def _reach(mask: np.ndarray) -> float:
    """How near, in source px, a region's samples come to the true junction."""
    ys, xs = np.nonzero(mask)
    d2 = ((xs + 0.5) / WIN_RES - WIN) ** 2 + ((ys + 0.5) / WIN_RES - WIN) ** 2
    return float(np.sqrt(d2.min()))


def _junction_error(truth: np.ndarray, out: np.ndarray) -> float:
    """The furthest any of the artist's regions, as traced, sits from the true junction.

    Each region of the artist's is matched to the traced area of its ink that overlaps it
    most, and the truth's own reach (its blended samples about the junction, 0.01-0.03
    px) is taken off. A junction moved by d leaves none of the corners further than d
    away, and one of three 120-degree corners at least 0.87 d away, or one of four right
    angles d away, so this reads the displacement itself: 0.05, 0.1, 0.2 and 0.5 px moves
    of the true geometry read 0.06, 0.11, 0.20 and 0.43-0.50.
    """
    traced = _regions(out)
    worst = 0.0
    for k, m in _regions(truth):
        mine = [o for j, o in traced if j == k]
        best = max(mine, key=lambda o: int((o & m).sum()), default=None)
        if best is None or not (best & m).any():
            return float("inf")
        worst = max(worst, _reach(best) - _reach(m))
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
