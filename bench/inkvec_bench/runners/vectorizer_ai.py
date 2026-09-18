"""Vectorizer.AI — the commercial ceiling.

This is the number the project is measured against, so the harness supports it
directly. Three things to be careful about, in order of importance:

1. **It costs money per image.** ``mode="production"`` consumes paid credits on the
   user's account. It is never selected by default and never reachable without an
   explicit opt-in flag; the registry will not even list this runner as enabled
   unless credentials are present *and* the caller asked for it.
2. **``mode="test"`` is free but watermarked**, which corrupts every pixel metric.
   Useful for checking that the plumbing works, useless for scoring. The harness
   tags results from test mode so they can never be silently mixed into a comparison.
3. **It uploads the corpus to a third party.** Fine for public emoji and synthetic
   shapes; think before pointing it at anything confidential.
"""
from __future__ import annotations

from typing import Any

from .. import config
from .base import ParamSet, Runner, RunnerError


class VectorizerAIRunner(Runner):
    name = "vectorizer_ai"
    is_remote = True

    def __init__(self, mode: str = "test") -> None:
        if mode not in ("test", "production"):
            raise ValueError("mode must be 'test' or 'production'")
        self.mode = mode

    def available(self) -> tuple[bool, str]:
        if not (config.VECTORIZER_AI_ID and config.VECTORIZER_AI_SECRET):
            return False, "set VECTORIZER_AI_ID and VECTORIZER_AI_SECRET in the environment"
        try:
            import requests  # noqa: F401
        except ImportError:
            return False, "pip install requests"
        return True, ""

    def grid(self) -> list[ParamSet]:
        # Their quality knob is shape-stacking plus curve alphabet. Sweep the two that
        # map onto our own axes: curve types allowed, and output stacking.
        out = []
        for curves in ("all", "cubic_only"):
            for stacking in ("stacked", "cutouts"):
                out.append(ParamSet(
                    f"{curves}-{stacking}",
                    {
                        "output.curves.circular_arcs": curves == "all",
                        "output.curves.elliptical_arcs": curves == "all",
                        "output.curves.quadratic_beziers": curves == "all",
                        "output.shape_stacking": (
                            "stack_shapes" if stacking == "stacked" else "cut_outs"
                        ),
                    },
                ))
        return out

    def run(self, png_bytes: bytes, params: dict[str, Any]) -> str:
        import requests

        ok, why = self.available()
        if not ok:
            raise RunnerError(why)

        data = {"mode": self.mode, "output.file_format": "svg"}
        for k, v in params.items():
            data[k] = str(v).lower() if isinstance(v, bool) else str(v)

        try:
            resp = requests.post(
                config.VECTORIZER_AI_ENDPOINT,
                files={"image": ("input.png", png_bytes, "image/png")},
                data=data,
                auth=(config.VECTORIZER_AI_ID, config.VECTORIZER_AI_SECRET),
                timeout=120,
            )
        except Exception as e:
            raise RunnerError(f"request failed: {type(e).__name__}: {e}") from e

        if resp.status_code != 200:
            raise RunnerError(f"HTTP {resp.status_code}: {resp.text[:300]}")
        return resp.text

    @property
    def result_tag(self) -> str:
        return f"{self.name}[{self.mode}]"
