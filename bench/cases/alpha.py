"""Transparency: what must survive it, and what must not be invented from it.

Four properties, each held still on its own:

* **An anti-aliased edge against nothing** is the common case and the one that must never
  regress. The rim of a shape on a transparent ground is partial alpha for a *geometric*
  reason, and the tracer's whole claim is that it reads geometry out of those values. The
  shape must come back opaque, at its own colour, with its edge in the right place — not
  as a translucent shape, and not as a rim of extra faces.
* **A hole is a hole.** A ring's counter is transparent in the source; painted white it is
  invisible on white and wrong everywhere else, which is precisely what a corpus scored
  over white cannot see. Measured on the alpha channel.
* **A flat wash is one opacity**, recovered with its own colour rather than baked against
  whatever the tracer matted the image onto.
* **A soft ramp is not a stack of washes.** Alpha that varies across a shape — a glow, a
  feathered edge — is one thing; quantising it into bands is the alpha version of
  quantising a colour gradient, and the count of emitted shapes is what catches it.

Every check reads the render, so a document that expresses transparency some other way
still passes if a viewer cannot tell.
"""
from __future__ import annotations

import numpy as np

from _common import Case, Check, svg_doc

SCALE = 4
CUTOUT = ("--cutout",)


def _alpha(rgba: np.ndarray) -> np.ndarray:
    return rgba[..., 3]


def _alpha_error(t) -> float:
    return float(np.abs(_alpha(t.out_rgba(SCALE)) - _alpha(t.truth_rgba(SCALE))).mean())


def _over(rgba: np.ndarray, ground) -> np.ndarray:
    a = rgba[..., 3:4]
    return np.clip(rgba[..., :3] * a + np.asarray(ground, np.float32) * (1.0 - a), 0.0, 1.0)


def _dark_error(t) -> float:
    """Mean absolute error over a dark ground — where paint that should be a hole shows."""
    dark = (0.078, 0.071, 0.063)
    return float(np.abs(_over(t.out_rgba(SCALE), dark) - _over(t.truth_rgba(SCALE), dark)).mean())


def _opaque_share(t) -> float:
    """Of the pixels the truth draws solidly, how many does the trace also draw solidly."""
    ta, oa = _alpha(t.truth_rgba(SCALE)), _alpha(t.out_rgba(SCALE))
    solid = ta > 0.98
    return float((oa[solid] > 0.9).mean()) if solid.any() else 0.0


# --------------------------------------------------------------------------- inputs

# A plain disc on nothing. Every boundary pixel is partial alpha, and none of it is a wash.
AA_DISC = svg_doc(128, '<circle cx="63.4" cy="64.6" r="41.3" fill="#c9754a"/>')

# A ring: the counter is transparent, and on white it is invisible.
RING = svg_doc(
    128,
    '<path d="M64,18 A46,46 0 1,1 63.9,18 Z M64,42 A22,22 0 1,0 63.9,42 Z" '
    'fill="#1a4a8a" fill-rule="evenodd"/>',
)

# One flat wash over an opaque bar and over nothing, which is the case a matte destroys:
# at 30% white the wash and the empty ground are the same colour once composited.
WASH = svg_doc(
    128,
    '<rect x="14" y="60" width="100" height="40" fill="#1a1816"/>'
    '<rect x="24" y="24" width="80" height="56" fill="#faf8f5" fill-opacity="0.3"/>',
)

# A ramp from opaque to clear across one shape.
RAMP = svg_doc(
    128,
    '<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="0">'
    '<stop offset="0" stop-color="#c9754a" stop-opacity="1"/>'
    '<stop offset="1" stop-color="#c9754a" stop-opacity="0"/>'
    "</linearGradient></defs>"
    '<rect x="16" y="34" width="96" height="60" fill="url(#g)"/>',
)


# --------------------------------------------------------------------------- checks


def _aa_edge(t):
    return [
        Check("alpha_error", _alpha_error(t), 0.01, "<",
              "an anti-aliased rim is geometry; the shape itself is opaque"),
        Check("solid_kept", _opaque_share(t), 0.99, ">=",
              "every pixel the artist drew solidly comes back solid"),
        Check("shapes", len(t.out_shapes), 2, "<=",
              "one disc, and at most one more for a background the tracer may separate"),
    ]


def _hole(t):
    return [
        Check("alpha_error", _alpha_error(t), 0.02, "<",
              "the counter is transparent in the source and must be in the output"),
        Check("dark_error", _dark_error(t), 0.03, "<",
              "paint in the counter is invisible on white and obvious on anything else"),
    ]


def _wash(t):
    out = t.out_rgba(SCALE)
    truth = t.truth_rgba(SCALE)
    # The wash over nothing: top-left of the wash rectangle, well away from the bar.
    n = out.shape[0]
    box = (slice(int(0.28 * n), int(0.40 * n)), slice(int(0.25 * n), int(0.70 * n)))
    got, want = out[box], truth[box]
    return [
        Check("wash_alpha", float(np.abs(got[..., 3].mean() - want[..., 3].mean())), 0.05, "<",
              "the wash keeps the opacity the artist drew it at"),
        Check("wash_colour",
              float(np.abs(got[..., :3].mean(axis=(0, 1)) - want[..., :3].mean(axis=(0, 1))).max()),
              0.08, "<",
              "and its own colour, not the colour it had over the matte"),
        Check("dark_error", _dark_error(t), 0.04, "<",
              "which is only visible on a ground other than white"),
    ]


def _ramp(t):
    return [
        Check("shapes", len(t.out_shapes), 4, "<=",
              "one ramp is one shape; a stack of washes is the alpha version of banding"),
        Check("alpha_error", _alpha_error(t), 0.06, "<",
              "and it fades the way the artist drew it"),
    ]


CASES = [
    Case(name="alpha_aa_edge", what="an anti-aliased rim is geometry, not translucency",
         svg=AA_DISC, check=_aa_edge),
    Case(name="alpha_aa_edge_cutout", what="the same, with transparency carried out",
         svg=AA_DISC, check=_aa_edge, args=CUTOUT),
    Case(name="alpha_hole", what="a transparent counter stays transparent",
         svg=RING, check=_hole, args=CUTOUT),
    Case(name="alpha_wash", what="a flat wash keeps its opacity and its colour",
         svg=WASH, check=_wash, args=CUTOUT),
    Case(name="alpha_ramp", what="a ramp of alpha is one shape, not a stack",
         svg=RAMP, check=_ramp, args=CUTOUT),
]
