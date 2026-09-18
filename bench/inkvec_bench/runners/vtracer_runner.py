"""VTracer — the incumbent open-source colour tracer, and our primary baseline.

The Python bindings expose the three parameters the CLI keeps hidden from ``--help``
(``corner_threshold``, ``length_threshold``, ``splice_threshold``), which is what makes
a proper quality sweep possible here.

The sweep deliberately spans both the clustering knobs and the curve knobs, because
VTracer's anchor count is driven by both and tuning only one produces a misleadingly
flat Pareto curve.
"""
from __future__ import annotations

from typing import Any

from .base import ParamSet, Runner, RunnerError


class VTracerRunner(Runner):
    name = "vtracer"

    def available(self) -> tuple[bool, str]:
        try:
            import vtracer  # noqa: F401
        except ImportError:
            return False, "pip install vtracer"
        return True, ""

    def grid(self) -> list[ParamSet]:
        out: list[ParamSet] = []
        # Curve-fitting sweep at fixed colour settings: isolates anchor economy.
        for ct, lt, st in ((60, 4.0, 45), (60, 8.0, 45), (90, 12.0, 60), (120, 20.0, 75)):
            out.append(ParamSet(
                f"spline-ct{ct}-lt{lt:g}",
                dict(colormode="color", mode="spline", hierarchical="stacked",
                     filter_speckle=4, color_precision=6, layer_difference=16,
                     corner_threshold=ct, length_threshold=lt, splice_threshold=st,
                     path_precision=3),
            ))
        # Colour/clustering sweep at fixed curve settings: isolates region economy.
        for cp, ld, fs in ((8, 8, 2), (6, 16, 4), (4, 24, 8), (3, 48, 16)):
            out.append(ParamSet(
                f"spline-cp{cp}-ld{ld}",
                dict(colormode="color", mode="spline", hierarchical="stacked",
                     filter_speckle=fs, color_precision=cp, layer_difference=ld,
                     corner_threshold=60, length_threshold=8.0, splice_threshold=45,
                     path_precision=3),
            ))
        # Cutout mode: VTracer's seam-free mosaic. The interesting comparison for the
        # seam/overdraw pair, since stacked and cutout sit at opposite ends of it.
        out.append(ParamSet(
            "cutout",
            dict(colormode="color", mode="spline", hierarchical="cutout",
                 filter_speckle=4, color_precision=6, layer_difference=16,
                 corner_threshold=60, length_threshold=8.0, splice_threshold=45,
                 path_precision=3),
        ))
        # Polygon mode: the no-curve-fitting floor.
        out.append(ParamSet(
            "polygon",
            dict(colormode="color", mode="polygon", hierarchical="stacked",
                 filter_speckle=4, color_precision=6, layer_difference=16,
                 path_precision=3),
        ))
        return out

    def run(self, png_bytes: bytes, params: dict[str, Any]) -> str:
        import vtracer

        try:
            return vtracer.convert_raw_image_to_svg(png_bytes, img_format="png", **params)
        except Exception as e:
            raise RunnerError(f"vtracer failed: {type(e).__name__}: {e}") from e
