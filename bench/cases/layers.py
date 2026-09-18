"""Layered colour: rings that must stay separate, and ramps that must stay one ramp.

Two opposite failures of the same decision — how many inks are there?

* **Concentric rings.** Four annuli a few units of dE apart. Merge them and the figure
  becomes one blob; the score barely moves, because the merged colour is close to all
  four. This has happened.
* **Gradients.** One ramp, one gradient. Quantise it and it comes back as a stack of
  bands, each an opaque path with its own paint: more parameters, worse at 4x, and a
  document with nothing to edit.

Both are read off the render rather than the file, so a gradient expressed as a mesh of
paths and a gradient expressed as a gradient are judged by what a viewer sees; the
element and paint counts are then a separate reading of what a designer would open.
"""
from __future__ import annotations

import re

from _common import Case, Check, de00, ink_area, svg_doc

CX, CY = 64.27, 63.73
SCALE = 4

# Four blues about six units of dE00 apart: far enough that merging any pair is a defect
# by the tracer's own palette criterion, close enough that a sloppy one will.
RING_COLOURS = ["#20509c", "#3a68ac", "#5480bc", "#6e98cc"]
RING_RADII = [52.0, 43.0, 34.0, 25.0]

RINGS = svg_doc(128, "".join(
    f'<circle cx="{CX}" cy="{CY}" r="{r}" fill="{c}"/>'
    for r, c in zip(RING_RADII, RING_COLOURS)))


def _hex_rgb(h: str):
    return tuple(int(h[i:i + 2], 16) / 255.0 for i in (1, 3, 5))


def _rings(t):
    truth, out = t.truth_rgba(SCALE), t.out_rgba(SCALE)
    present, worst = 0, 0.0
    for h in RING_COLOURS:
        rgb = _hex_rgb(h)
        ta = ink_area(truth, rgb, SCALE)
        oa = ink_area(out, rgb, SCALE)
        if ta > 0 and oa >= 0.5 * ta:
            present += 1
        if ta > 0:
            worst = max(worst, abs(oa - ta) / ta)
    return [
        Check("rings_present", present, len(RING_COLOURS), "==",
              "all four of the artist's inks, each still covering at least half its area"),
        Check("worst_area_err", worst, 0.15, "<",
              "a ring boundary off by 0.05 px changes its area by well under 1 %"),
    ]


LINEAR = svg_doc(128, (
    '<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="0">'
    '<stop offset="0" stop-color="#102a6a"/><stop offset="1" stop-color="#e0a020"/>'
    '</linearGradient></defs>'
    '<rect x="14.3" y="20.7" width="99" height="86" fill="url(#g)"/>'))

RADIAL = svg_doc(128, (
    '<defs><radialGradient id="g" cx="0.45" cy="0.4" r="0.6">'
    '<stop offset="0" stop-color="#f0e0a0"/><stop offset="1" stop-color="#204080"/>'
    '</radialGradient></defs>'
    f'<circle cx="{CX}" cy="{CY}" r="50" fill="url(#g)"/>'))

_GRAD_TAG = re.compile(r"<(?:linear|radial)Gradient\b")


def _gradient(t):
    paints = len(_GRAD_TAG.findall(t.out_svg))
    return [
        Check("gradient_paints", paints, 1, "==",
              "the artist wrote one ramp; a second paint means it was cut into bands"),
        Check("elements", len(t.out_shapes), 2, "<=",
              "one shape, plus one for a backdrop the tracer may legitimately separate"),
        # dE00 2 is roughly one just-noticeable difference; the tracer's mean over the
        # 980-icon corpus is ~0.33, so 2 is loose for a single flat-lit ramp.
        Check("de00_mean", de00(t, SCALE), 2.0, "<",
              "dE00 2 is about one just-noticeable difference over the whole shape"),
    ]


CASES = [
    Case(name="rings_concentric", what="four similar annuli stay four annuli",
         svg=RINGS, check=_rings),
    Case(name="gradient_linear", what="a linear ramp comes back as one gradient",
         svg=LINEAR, check=_gradient),
    Case(name="gradient_radial", what="a radial ramp comes back as one gradient",
         svg=RADIAL, check=_gradient),
]
