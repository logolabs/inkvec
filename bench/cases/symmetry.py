"""Symmetry the artist put in, and whether it comes back out.

Nothing in a colour score rewards symmetry, and a fitter that treats the two halves of a
figure as unrelated contours will break it by a tenth of a pixel here and there. A viewer
sees that immediately and cannot say why. `regularity.py` measures the same residual over
the real corpus; this pins it on figures that are symmetric to the last bit, so any
residual at all is ours.

The mirror axis is placed at exactly 64 in source units: for a render of 4*128 pixels,
`img[:, ::-1]` reflects about pixel (N-1)/2, which is source coordinate N/(2*scale) = 64.
Anywhere else and the case would be measuring a half-pixel resampling error.
"""
from __future__ import annotations

from _common import Case, Check, svg_doc, symmetry

INK, INK2 = "#204080", "#c04030"
AX = 64.0  # the mirror axis the render's own pixel grid reflects about
SCALE = 4

# 1e-4 mean absolute on a [0, 1] render is about a fortieth of an 8-bit level averaged
# over the whole canvas: below what the renderer's own rounding produces on a figure that
# really is symmetric, and far below a tenth of a pixel of contour drift.
SYM_LIMIT = 1e-4


def _figure(mirror: str) -> str:
    """A figure with several parts of different sizes, exactly symmetric under `mirror`."""
    def part(dx, dy, w, h, fill):
        # Draw the part and its image, so the figure is symmetric by construction rather
        # than by arithmetic that could quietly be off by a rounding.
        out = [f'<rect x="{AX + dx:.4f}" y="{AX + dy:.4f}" width="{w}" height="{h}" fill="{fill}"/>']
        if mirror == "v":
            out.append(f'<rect x="{AX - dx - w:.4f}" y="{AX + dy:.4f}" width="{w}" height="{h}" fill="{fill}"/>')
        elif mirror == "h":
            out.append(f'<rect x="{AX + dx:.4f}" y="{AX - dy - h:.4f}" width="{w}" height="{h}" fill="{fill}"/>')
        else:  # half turn
            out.append(f'<rect x="{AX - dx - w:.4f}" y="{AX - dy - h:.4f}" width="{w}" height="{h}" fill="{fill}"/>')
        return "".join(out)

    def disc(dx, dy, r, fill):
        out = [f'<circle cx="{AX + dx:.4f}" cy="{AX + dy:.4f}" r="{r}" fill="{fill}"/>']
        if mirror == "v":
            out.append(f'<circle cx="{AX - dx:.4f}" cy="{AX + dy:.4f}" r="{r}" fill="{fill}"/>')
        elif mirror == "h":
            out.append(f'<circle cx="{AX + dx:.4f}" cy="{AX - dy:.4f}" r="{r}" fill="{fill}"/>')
        else:
            out.append(f'<circle cx="{AX - dx:.4f}" cy="{AX - dy:.4f}" r="{r}" fill="{fill}"/>')
        return "".join(out)

    return (part(3.27, -42.73, 22.43, 18.63, INK)
            + part(11.73, -18.37, 31.23, 40.27, INK)
            + disc(28.63, 30.37, 9.27, INK2)
            + disc(9.37, 34.73, 4.63, INK))


AXES = {"v": "vertical mirror", "h": "horizontal mirror", "r": "half turn"}


def _sym(axis: str):
    def check(t):
        return [
            Check("out_residual", symmetry(t.out_rgba(SCALE))[axis], SYM_LIMIT, "<",
                  "1e-4 is under the renderer's own rounding on a figure that is truly symmetric"),
            # Guards the case, not the tracer: if this ever fails the figure is
            # mis-authored and the reading above means nothing.
            Check("truth_residual", symmetry(t.truth_rgba(SCALE))[axis], SYM_LIMIT, "<",
                  "the artist's figure is symmetric by construction"),
        ]
    return check


CASES = [
    Case(name=f"mirror_{a}", what=f"{label} the artist drew survives the trace",
         svg=svg_doc(128, _figure(a)), check=_sym(a))
    for a, label in AXES.items()
]
