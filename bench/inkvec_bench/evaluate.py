"""The evaluation driver: run every (input x runner x setting) cell and score it.

One row per cell. Failures are recorded as rows with an ``error`` field rather than
aborting the sweep — a tracer that crashes on 3% of inputs has told us something
important, and losing the other 97% to an exception would hide it.
"""
from __future__ import annotations

import gzip
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Iterable

import numpy as np

from . import config, render, svgmodel
from .corpora.build import CorpusItem
from .metrics import color as m_color
from .metrics import editability as m_edit
from .metrics import raster as m_raster
from .metrics import structure as m_struct
from .runners.base import ParamSet, Runner


@dataclass
class Row:
    uid: str
    name: str
    corpus: str
    tier: int
    runner: str
    setting: str
    setting_key: str = ""
    quality: str = "n/a"
    variant: str = "clean"      # clean | noise | jpeg | boundary
    error: str = ""
    runtime_s: float = float("nan")
    svg_bytes: float = float("nan")
    svg_gzip_bytes: float = float("nan")
    metrics: dict[str, float] = field(default_factory=dict)

    def flat(self) -> dict[str, Any]:
        d = {k: v for k, v in asdict(self).items() if k != "metrics"}
        d.update(self.metrics)
        return d


def _eval_scales(tier: int, scales=None) -> list[int]:
    out = []
    for s in (scales or config.EVAL_SCALES):
        if (tier * s) ** 2 <= config.MAX_EVAL_PIXELS:
            out.append(s)
    return out or [1]


def score(
    item: CorpusItem,
    cand_svg: str,
    input_rgba: np.ndarray | None = None,
    edit_locality: bool = False,
    scales=None,
) -> dict[str, float]:
    """All metrics for one traced SVG."""
    out: dict[str, float] = {}

    doc = svgmodel.parse(cand_svg)
    if doc.parse_error:
        out["parse_error"] = 1.0
        return out
    out["parse_error"] = 0.0

    out.update(m_edit.structure_metrics(doc))
    out.update(m_edit.coverage_metrics(doc))

    # A declared ground truth that does not resolve is a bug, not an absent one. Skipping
    # it quietly drops every structural metric from the report and leaves the run looking
    # complete, which is how a path-resolution problem survives a whole measurement.
    gt_svg = None
    out["gt_missing"] = 0.0
    if item.gt_svg_path:
        gt = Path(item.gt_svg_path)
        if gt.exists():
            gt_svg = gt.read_text(encoding="utf-8")
            out.update(m_struct.compare_structure(svgmodel.parse(gt_svg), doc))
        else:
            out["gt_missing"] = 1.0

    if input_rgba is None:
        input_rgba = render.load_rgba(item.png_path)

    for s in _eval_scales(item.tier, scales):
        px = item.tier * s
        try:
            cand = render.render(cand_svg, px, px)
        except Exception:
            continue

        if s == 1:
            ref = input_rgba
        elif gt_svg is not None:
            try:
                ref = render.render(gt_svg, px, px)
            except Exception:
                continue
        else:
            # No ground-truth vector: nothing legitimate to compare against at zoom.
            continue

        if ref.shape != cand.shape:
            continue

        ref_rgb = render.composite(ref)
        cand_rgb = render.composite(cand)
        for k, v in m_raster.compare(ref_rgb, cand_rgb).items():
            out[f"{k}@{s}x"] = v
        mask = ref[..., 3] >= config.OPAQUE_ALPHA
        for k, v in m_color.delta_e00(ref_rgb, cand_rgb, mask if mask.any() else None).items():
            out[f"{k}@{s}x"] = v
        for k, v in m_edit.seam_metrics(ref, cand).items():
            out[f"{k}@{s}x"] = v

    try:
        out["palette_size"] = float(m_color.palette_size(render.render(cand_svg, item.tier, item.tier)))
    except Exception:
        pass

    if edit_locality:
        out.update(m_edit.edit_locality(cand_svg, item.tier, item.tier))

    return out


def run_cell(
    item: CorpusItem,
    runner: Runner,
    ps: ParamSet,
    out_dir: Path,
    variant: str = "clean",
    png_override: Path | None = None,
    edit_locality: bool = False,
    scales=None,
) -> Row:
    row = Row(
        uid=item.uid, name=item.name, corpus=item.corpus, tier=item.tier,
        runner=getattr(runner, "result_tag", runner.name), setting=ps.label,
        setting_key=ps.key, quality=getattr(runner, "quality_level", "n/a"),
        variant=variant,
    )
    src = Path(png_override or item.png_path)
    png_bytes = src.read_bytes()

    t0 = time.perf_counter()
    try:
        svg = runner.run(png_bytes, ps.params)
    except Exception as e:
        row.error = f"{type(e).__name__}: {e}"[:300]
        return row
    row.runtime_s = time.perf_counter() - t0

    dest = Path(out_dir) / runner.name / variant / str(item.tier) / ps.key
    dest.mkdir(parents=True, exist_ok=True)
    (dest / f"{item.name}.svg").write_text(svg, encoding="utf-8")

    row.svg_bytes = float(len(svg.encode("utf-8")))
    row.svg_gzip_bytes = float(len(gzip.compress(svg.encode("utf-8"), 9)))

    try:
        row.metrics = score(item, svg, render.load_rgba(src), edit_locality=edit_locality, scales=scales)
    except Exception as e:
        row.error = f"scoring: {type(e).__name__}: {e}"[:300]
    return row


def sweep(
    items: Iterable[CorpusItem],
    runners: list[Runner],
    out_dir: Path,
    variant: str = "clean",
    png_map: dict[str, Path] | None = None,
    edit_locality: bool = False,
    scales=None,
    progress: bool = True,
) -> list[Row]:
    items = list(items)
    cells = [(it, r, ps) for it in items for r in runners for ps in r.grid()]
    rows: list[Row] = []

    it_iter = cells
    if progress:
        try:
            from tqdm import tqdm

            it_iter = tqdm(cells, desc=f"sweep[{variant}]", unit="cell")
        except ImportError:
            pass

    for item, runner, ps in it_iter:
        override = png_map.get(item.uid) if png_map else None
        rows.append(run_cell(item, runner, ps, out_dir, variant, override, edit_locality, scales))
    return rows


def rows_to_frame(rows: list[Row]):
    import pandas as pd

    return pd.DataFrame([r.flat() for r in rows])
