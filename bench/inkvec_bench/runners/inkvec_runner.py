"""Inkvec itself — the engine under construction.

Shells out to the `inkvec` binary from the Rust workspace, so the harness measures the
real artefact rather than a Python reimplementation of it.

**Scope caveat, and it matters for reading any result.** The engine is currently bilevel:
S1 (colour region proposal) and S3 (the planar map) are not wired in, and the S4 segment
alphabet holds lines only. So:

* against **Potrace** — our bilevel control — the comparison is apples to apples, and this
  is the one to read;
* against **VTracer**, only the *structural* metrics are meaningful. Fidelity numbers on
  colour inputs measure the missing colour stage, not the fitting;
* curved shapes are penalised by the line-only alphabet. A circle needs ~4 cubics or 2
  arcs; in line segments it needs dozens no matter how good the segmentation is.
"""
from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import Any

from .base import ParamSet, Runner, RunnerError

# Workspace root is three levels up from this file.
_REPO = Path(__file__).resolve().parents[3]


def _binary() -> Path | None:
    """Locate the built binary, or fall back to one on PATH."""
    for rel in ("target/release/inkvec.exe", "target/release/inkvec",
                "target/debug/inkvec.exe", "target/debug/inkvec"):
        p = _REPO / rel
        if p.exists():
            return p
    found = shutil.which("inkvec")
    return Path(found) if found else None


class InkvecRunner(Runner):
    name = "inkvec"
    bilevel_only = True
    quality_level = "best"

    def available(self) -> tuple[bool, str]:
        if _binary() is None:
            return False, "build it first: cargo build --release"
        return True, ""

    def grid(self) -> list[ParamSet]:
        # tau sets the chord tolerance in standard deviations; precision sets the MDL cost
        # of a coordinate via lambda = ln(extent / precision). Together they span the
        # quality range the way the design intends — both are derived quantities rather
        # than free knobs, so the sweep is over *stated intent*, not over magic numbers.
        out = []
        for tau in (1.0, 2.0, 3.0, 5.0):
            out.append(ParamSet(f"tau{tau:g}", {"tau": tau, "precision": 0.1}))
        for prec in (0.02, 0.5, 2.0):
            out.append(ParamSet(f"prec{prec:g}", {"tau": 2.0, "precision": prec}))
        return out

    def run(self, png_bytes: bytes, params: dict[str, Any]) -> str:
        exe = _binary()
        if exe is None:
            raise RunnerError("inkvec binary not found")

        with tempfile.TemporaryDirectory() as d:
            src = Path(d) / "in.png"
            dst = Path(d) / "out.svg"
            src.write_bytes(png_bytes)
            cmd = [
                str(exe), str(src), "-o", str(dst), "--quiet",
                "--tau", str(params.get("tau", 2.0)),
                "--precision", str(params.get("precision", 0.1)),
            ]
            # Only override the engine's own default when a setting asks to. Passing
            # min_area=1.0 unconditionally disabled despeckling for every run, which is a
            # legitimate setting but not the default one — and measuring it silently made
            # the harness report a configuration nobody would ship. logo_like came out at
            # 1148 parameters that way against 49 at the default.
            if "min_area" in params:
                cmd += ["--min-area", str(params["min_area"])]
            try:
                r = subprocess.run(cmd, capture_output=True, timeout=120)
            except subprocess.TimeoutExpired as e:
                raise RunnerError("inkvec timed out") from e
            if r.returncode != 0:
                raise RunnerError(f"inkvec exited {r.returncode}: {r.stderr.decode()[:300]}")
            if not dst.exists():
                raise RunnerError("inkvec produced no output")
            return dst.read_text(encoding="utf-8")
