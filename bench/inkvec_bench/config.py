"""Paths and global settings for the benchmark harness."""
from __future__ import annotations

import os
from dataclasses import dataclass, field
from pathlib import Path

# Repository root is two levels up from this file (bench/inkvec_bench/config.py).
REPO_ROOT = Path(__file__).resolve().parents[2]
BENCH_ROOT = REPO_ROOT / "bench"

# Data lives outside the package so it can be gitignored wholesale.
DATA_ROOT = Path(os.environ.get("INKVEC_BENCH_DATA", BENCH_ROOT / "data"))

CORPUS_SVG_DIR = DATA_ROOT / "corpus_svg"      # ground-truth SVG sources
CORPUS_RASTER_DIR = DATA_ROOT / "corpus_raster"  # rendered inputs, by resolution tier
RUNS_DIR = DATA_ROOT / "runs"                    # traced SVG outputs, by runner+params
RESULTS_DIR = DATA_ROOT / "results"              # scored CSV/parquet
REPORT_DIR = DATA_ROOT / "report"                # plots and HTML

# Resolution tiers the corpus is rendered at. The <=128px tiers are the hard,
# commercially common case: anti-aliasing dominates and sub-pixel recovery matters most.
RASTER_TIERS = (32, 64, 128, 256, 512)

# Scales at which fidelity is evaluated, relative to the input raster size.
# An SVG must be correct when zoomed; single-scale losses over-fit to source resolution.
# 4x already exposes boundary placement error; 16x is ~4x the cost for a marginal
# amount of extra signal, so it is opt-in (`run --deep`) for publication runs.
EVAL_SCALES = (1, 4)
EVAL_SCALES_DEEP = (1, 4, 16)

# Cap on evaluation raster area (pixels) to keep 16x tractable.
MAX_EVAL_PIXELS = 4096 * 4096

# Alpha thresholds used by the coverage-based structural metrics.
OPAQUE_ALPHA = 0.9
TRANSPARENT_ALPHA = 0.5


@dataclass(frozen=True)
class Paths:
    """Resolved output locations for one benchmark invocation."""

    tag: str = "default"

    @property
    def runs(self) -> Path:
        return RUNS_DIR / self.tag

    @property
    def results(self) -> Path:
        return RESULTS_DIR / self.tag

    @property
    def report(self) -> Path:
        return REPORT_DIR / self.tag

    def ensure(self) -> "Paths":
        for p in (self.runs, self.results, self.report):
            p.mkdir(parents=True, exist_ok=True)
        return self


def ensure_data_dirs() -> None:
    for p in (CORPUS_SVG_DIR, CORPUS_RASTER_DIR, RUNS_DIR, RESULTS_DIR, REPORT_DIR):
        p.mkdir(parents=True, exist_ok=True)


# --- Vectorizer.AI API credentials -------------------------------------------------
# Paid, per-image. Never invoked unless explicitly opted in on the command line AND
# credentials are present. See runners/vectorizer_ai.py.
VECTORIZER_AI_ID = os.environ.get("VECTORIZER_AI_ID")
VECTORIZER_AI_SECRET = os.environ.get("VECTORIZER_AI_SECRET")
VECTORIZER_AI_ENDPOINT = "https://vectorizer.ai/api/v1/vectorize"
