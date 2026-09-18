"""Runner interface.

Every tracer has a quality knob. Comparing one operating point per method is how this
field currently misleads itself: a tracer tuned for detail beats one tuned for
compactness on fidelity and loses on anchor count, and whichever number the author
chose to lead with decides the story. So a runner does not expose "run"; it exposes a
*grid* of settings spanning its own quality range, and the harness reports the Pareto
frontier over that grid.
"""
from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field
from typing import Any, Iterable


@dataclass(frozen=True)
class ParamSet:
    """One point in a runner's quality sweep."""

    label: str
    params: dict[str, Any] = field(default_factory=dict)

    @property
    def key(self) -> str:
        blob = json.dumps(self.params, sort_keys=True)
        return f"{self.label}-{hashlib.sha1(blob.encode()).hexdigest()[:8]}"


class Runner:
    """A raster-to-vector engine under test."""

    name: str = "base"
    #: True when the runner costs real money per image, or otherwise leaves the machine.
    is_remote: bool = False
    #: True when the runner only handles bilevel input.
    bilevel_only: bool = False
    #: Quality level this runner was invoked at (DESIGN.md 5.7). Recorded with every row
    #: so a `draft` result can never be silently compared against a `best` one.
    quality_level: str = "n/a"

    def available(self) -> tuple[bool, str]:
        """Return (usable, reason-if-not)."""
        return True, ""

    def grid(self) -> list[ParamSet]:
        """Quality sweep. Ordered coarse -> fine by convention."""
        return [ParamSet("default")]

    def run(self, png_bytes: bytes, params: dict[str, Any]) -> str:
        """Trace a PNG and return SVG source. May raise."""
        raise NotImplementedError


class RunnerError(RuntimeError):
    pass
