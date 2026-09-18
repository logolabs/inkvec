"""Runner registry.

Local runners are enabled by default. Remote ones cost money and leave the machine,
so they are opt-in only and never appear in a default sweep.
"""
from __future__ import annotations

from .base import Runner
from .potrace_runner import PotraceRunner
from .vectorizer_ai import VectorizerAIRunner
from .inkvec_runner import InkvecRunner
from .vtracer_runner import VTracerRunner

LOCAL_RUNNERS = {
    "inkvec": InkvecRunner,
    "vtracer": VTracerRunner,
    "potrace": PotraceRunner,
}


def build(names: list[str] | None = None, vectorizer_ai_mode: str | None = None) -> list[Runner]:
    """Instantiate runners by name.

    ``vectorizer_ai_mode`` must be passed explicitly to enable the paid API; there is
    deliberately no way to get it from a default argument.
    """
    selected: list[Runner] = []
    names = names or list(LOCAL_RUNNERS)
    for n in names:
        if n == "vectorizer_ai":
            if not vectorizer_ai_mode:
                continue
            selected.append(VectorizerAIRunner(mode=vectorizer_ai_mode))
            continue
        cls = LOCAL_RUNNERS.get(n)
        if cls is None:
            raise KeyError(f"unknown runner {n!r}; known: {sorted(LOCAL_RUNNERS) + ['vectorizer_ai']}")
        selected.append(cls())
    return selected


def status() -> list[tuple[str, bool, str]]:
    out = []
    for n, cls in LOCAL_RUNNERS.items():
        ok, why = cls().available()
        out.append((n, ok, why))
    ok, why = VectorizerAIRunner().available()
    out.append(("vectorizer_ai", ok, why or "opt-in only (--vectorizer-ai-mode)"))
    return out
