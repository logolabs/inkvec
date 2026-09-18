"""Potrace — bilevel control baseline.

Potrace only handles 1-bit input, so it cannot compete on the colour corpus. It is
here as a *control*: it is the best-understood tracing algorithm in existence, its
optimal-polygon dynamic program is the anchor-economy technique VTracer dropped for
speed, and it therefore marks how much anchor efficiency that decision costs. If our
engine cannot match Potrace's anchor economy on a bilevel input, the sub-pixel and
node-budget work is not paying for itself.

Uses the pure-Python ``potracer`` port, so no GPL C library is linked. (Potrace itself
is GPL and must not be vendored into this project.)
"""
from __future__ import annotations

import io
from typing import Any

import numpy as np
from PIL import Image

from .base import ParamSet, Runner, RunnerError


class PotraceRunner(Runner):
    name = "potrace"
    bilevel_only = True

    def available(self) -> tuple[bool, str]:
        try:
            import potrace  # noqa: F401
        except ImportError:
            return False, "pip install potracer"
        return True, ""

    def grid(self) -> list[ParamSet]:
        out = []
        for alphamax in (0.0, 0.6, 1.0, 1.334):
            for opttolerance in (0.2, 1.0):
                out.append(ParamSet(
                    f"a{alphamax:g}-o{opttolerance:g}",
                    dict(alphamax=alphamax, opttolerance=opttolerance, turdsize=2),
                ))
        return out

    def run(self, png_bytes: bytes, params: dict[str, Any]) -> str:
        import potrace

        img = Image.open(io.BytesIO(png_bytes)).convert("RGBA")
        w, h = img.size
        arr = np.asarray(img, dtype=np.float32) / 255.0
        # Composite on white, then threshold at mid grey.
        rgb = arr[..., :3] * arr[..., 3:4] + (1.0 - arr[..., 3:4])
        lum = rgb @ np.array([0.2126, 0.7152, 0.0722], dtype=np.float32)
        bitmap = potrace.Bitmap(lum < 0.5)

        try:
            path = bitmap.trace(
                turdsize=params.get("turdsize", 2),
                turnpolicy=potrace.POTRACE_TURNPOLICY_MINORITY,
                alphamax=params.get("alphamax", 1.0),
                opticurve=params.get("opttolerance", 0.2) > 0,
                opttolerance=params.get("opttolerance", 0.2),
            )
        except Exception as e:
            raise RunnerError(f"potrace failed: {type(e).__name__}: {e}") from e

        return _to_svg(path, w, h)


def _xy(p) -> tuple[float, float]:
    """potracer yields _Point objects here, tuples there. Accept both."""
    if hasattr(p, "x") and hasattr(p, "y"):
        return float(p.x), float(p.y)
    a, b = p
    return float(a), float(b)


def _to_svg(path, width: int, height: int) -> str:
    parts: list[str] = []
    for curve in path:
        sx, sy = _xy(curve.start_point)
        d = [f"M{sx:.3f},{sy:.3f}"]
        for seg in curve:
            ex, ey = _xy(seg.end_point)
            if seg.is_corner:
                cx, cy = _xy(seg.c)
                d.append(f"L{cx:.3f},{cy:.3f} L{ex:.3f},{ey:.3f}")
            else:
                c1x, c1y = _xy(seg.c1)
                c2x, c2y = _xy(seg.c2)
                d.append(f"C{c1x:.3f},{c1y:.3f} {c2x:.3f},{c2y:.3f} {ex:.3f},{ey:.3f}")
        d.append("Z")
        parts.append(" ".join(d))

    body = (
        f'<path d="{" ".join(parts)}" fill="#000000" fill-rule="evenodd"/>' if parts else ""
    )
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" '
        f'width="{width}" height="{height}">'
        f'<rect width="{width}" height="{height}" fill="#ffffff"/>{body}</svg>'
    )
