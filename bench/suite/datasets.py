"""What to run the engines on, and what to do to the input first.

Three families:

  internal   this project's own devset_v2 splits, judged against the artist's SVG. The
             split matters: `screen` is for iterating and `full` is the only one that
             settles a global constant, because a change measured on one split has
             twice been the opposite of the change measured on another.
  external   benchmarks the literature reports on, so a number here sits beside a
             published one. SVGenius ships its stratified icons and its metric code;
             point `--svgenius` at a clone.
  degraded   the same images put through what a real input has been through. No paper
             reports this and it is where this tracer is weakest, so leaving it out
             would be measuring the easy half.

A degradation is a function from a PNG to a PNG, applied once and cached, so that every
engine sees byte-identical input.
"""
from __future__ import annotations

import io
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterator

import numpy as np
from PIL import Image, ImageFilter

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))


@dataclass
class Item:
    key: str
    png: Path
    gt_svg: Path
    family: str
    bilevel: bool = False


# ------------------------------------------------------------------------------ internal
def internal(split: str = "screen", limit: int | None = None) -> Iterator[Item]:
    import svgeval

    sets = svgeval.load_sets()
    if split not in sets:
        raise SystemExit(f"unknown split {split!r}; have {sorted(sets)}")
    n = 0
    for it in sets[split]:
        png = (ROOT / "bench/data/corpus_raster" / it["corpus"] / svgeval.tier()
               / f"{it['stem']}.png")
        gt = ROOT / "bench/data/corpus_svg" / it["corpus"] / f"{it['stem']}.svg"
        if not png.exists() or not gt.exists():
            continue
        # Two-colour families, where a bilevel engine is being asked a fair question.
        bil = it["corpus"] in {"lucide", "material-icons", "simple-icons"}
        yield Item(f"{it['corpus']}/{it['stem']}", png, gt, it["corpus"], bil)
        n += 1
        if limit and n >= limit:
            return


# ------------------------------------------------------------------------------ external
def svgenius(root: Path, tier: str = "hard", limit: int | None = None) -> Iterator[Item]:
    d = Path(root) / "src/data" / tier / "process"
    if not d.is_dir():
        raise SystemExit(f"SVGenius not found at {d}")
    n = 0
    for sp in sorted(d.glob("*.svg")):
        png = sp.with_suffix(".png")
        if not png.exists():
            continue
        yield Item(f"svgenius-{tier}/{sp.stem}", png, sp, f"svgenius-{tier}")
        n += 1
        if limit and n >= limit:
            return


# ----------------------------------------------------------------------------- degraded
def _composite(png: Path) -> Image.Image:
    im = Image.open(png).convert("RGBA")
    bg = Image.new("RGBA", im.size, (255, 255, 255, 255))
    return Image.alpha_composite(bg, im).convert("RGB")


def _jpeg(q: int) -> Callable[[Path], Image.Image]:
    def f(png: Path) -> Image.Image:
        buf = io.BytesIO()
        _composite(png).save(buf, "JPEG", quality=q)
        return Image.open(buf).convert("RGB")

    return f


def _blur(sigma: float) -> Callable[[Path], Image.Image]:
    def f(png: Path) -> Image.Image:
        return _composite(png).filter(ImageFilter.GaussianBlur(sigma))

    return f


def _noise(levels: float) -> Callable[[Path], Image.Image]:
    def f(png: Path) -> Image.Image:
        a = np.asarray(_composite(png), np.float64)
        rng = np.random.default_rng(abs(hash(png.name)) % (2**32))
        return Image.fromarray(
            np.clip(a + rng.normal(0, levels, a.shape), 0, 255).astype(np.uint8))

    return f


def _rescale(factor: float) -> Callable[[Path], Image.Image]:
    def f(png: Path) -> Image.Image:
        im = _composite(png)
        w, h = im.size
        small = im.resize((max(8, int(w * factor)), max(8, int(h * factor))), Image.LANCZOS)
        return small.resize((w, h), Image.LANCZOS)

    return f


#: Named degradations. `clean` is the control and must always be present, because every
#: other row is only meaningful as a distance from it.
DEGRADATIONS: dict[str, Callable[[Path], Image.Image] | None] = {
    "clean": None,
    "jpeg-q80": _jpeg(80),
    "jpeg-q50": _jpeg(50),
    "blur-0.5": _blur(0.5),
    "blur-1.0": _blur(1.0),
    "noise-2": _noise(2.0),
    "rescale-0.5": _rescale(0.5),
}


def apply_degradation(item: Item, name: str, cache: Path) -> Path:
    """Materialise the degraded input once, so every engine sees the same bytes."""
    fn = DEGRADATIONS[name]
    if fn is None:
        return item.png
    cache.mkdir(parents=True, exist_ok=True)
    out = cache / f"{name}__{item.key.replace('/', '__')}.png"
    if not out.exists():
        fn(item.png).save(out)
    return out
