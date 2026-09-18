"""Render the ground-truth SVG corpus into raster inputs at several resolution tiers.

The small tiers are not padding. A 32px or 64px anti-aliased icon is the hardest and
most commercially common vectorization input there is: the boundary signal exists
almost entirely in partially-covered pixels, which is exactly the information a
thresholding tracer discards first. If an engine is going to lose, it loses here.
"""
from __future__ import annotations

import csv
import json
from dataclasses import dataclass
from pathlib import Path

from .. import config
from ..render import render_to_png


@dataclass
class CorpusItem:
    """One (source SVG, resolution tier) input to be traced."""

    name: str
    corpus: str
    tier: int
    png_path: Path
    gt_svg_path: Path | None  # None for in-the-wild rasters with no ground truth

    @property
    def uid(self) -> str:
        return f"{self.corpus}/{self.name}@{self.tier}"


def build_rasters(
    svg_dir: Path,
    out_dir: Path,
    corpus: str,
    tiers: tuple[int, ...] = config.RASTER_TIERS,
) -> list[CorpusItem]:
    """Render every SVG in ``svg_dir`` at each tier. Returns the manifest."""
    svg_dir, out_dir = Path(svg_dir), Path(out_dir)
    items: list[CorpusItem] = []

    for svg_path in sorted(svg_dir.glob("*.svg")):
        svg = svg_path.read_text(encoding="utf-8")
        for tier in tiers:
            dest = out_dir / corpus / str(tier)
            dest.mkdir(parents=True, exist_ok=True)
            png_path = dest / f"{svg_path.stem}.png"
            try:
                png_path.write_bytes(render_to_png(svg, tier, tier))
            except Exception as e:
                print(f"  ! render failed {svg_path.stem}@{tier}: {e}")
                continue
            items.append(CorpusItem(svg_path.stem, corpus, tier, png_path, svg_path))

    return items


def import_wild(
    image_dir: Path, out_dir: Path, corpus: str = "wild"
) -> list[CorpusItem]:
    """Import real rasters (logos, scans) that have no ground-truth SVG.

    Only 1x fidelity is meaningful for these — there is nothing to render a 4x
    reference from — but every structural and editability metric still applies, and
    those are the ones that distinguish engines anyway.
    """
    from PIL import Image

    image_dir, out_dir = Path(image_dir), Path(out_dir)
    dest = out_dir / corpus / "native"
    dest.mkdir(parents=True, exist_ok=True)
    items = []
    for p in sorted(image_dir.iterdir()):
        if p.suffix.lower() not in (".png", ".jpg", ".jpeg", ".webp", ".bmp", ".gif"):
            continue
        img = Image.open(p).convert("RGBA")
        png_path = dest / f"{p.stem}.png"
        img.save(png_path)
        items.append(CorpusItem(p.stem, corpus, max(img.size), png_path, None))
    return items


def write_manifest(items: list[CorpusItem], path: Path) -> None:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as f:
        w = csv.writer(f)
        w.writerow(["uid", "name", "corpus", "tier", "png_path", "gt_svg_path"])
        for it in items:
            w.writerow([
                it.uid, it.name, it.corpus, it.tier,
                str(Path(it.png_path).resolve()),
                str(Path(it.gt_svg_path).resolve()) if it.gt_svg_path else "",
            ])


def read_manifest(path: Path) -> list[CorpusItem]:
    """Load a manifest, resolving any relative path against the manifest's own directory.

    The writers disagree: the synthetic corpus records absolute paths and the fetched ones
    record paths relative to ``bench/``. That difference is invisible until someone
    evaluates from a different working directory, at which point ``gt_svg_path`` stops
    resolving — and ``evaluate`` only consults it behind an ``.exists()`` check, so every
    ground-truth metric (``dists@4x``, the whole structural family) silently vanishes from
    the report instead of failing. 180 of 200 rows in the current manifest are relative.

    Resolving here fixes it for both writers and for manifests already on disk.
    """
    # Which directory a relative entry is relative to depends on which writer produced it,
    # so try the plausible bases and take the one that exists rather than assuming. The
    # fetched corpora record paths relative to `bench/`, which is two levels above the
    # manifest; resolving against the manifest's own directory silently produces
    # `bench/data/corpus_raster/data/corpus_svg/...`, which exists nowhere.
    here = Path(path).resolve().parent
    bases = [here, here.parent, here.parent.parent, Path.cwd()]

    def fix(raw: str) -> Path:
        q = Path(raw)
        if q.is_absolute():
            return q
        for b in bases:
            cand = (b / q).resolve()
            if cand.exists():
                return cand
        return (bases[0] / q).resolve()

    items = []
    with Path(path).open(encoding="utf-8") as f:
        for row in csv.DictReader(f):
            items.append(CorpusItem(
                name=row["name"], corpus=row["corpus"], tier=int(row["tier"]),
                png_path=fix(row["png_path"]),
                gt_svg_path=fix(row["gt_svg_path"]) if row["gt_svg_path"] else None,
            ))
    return items
