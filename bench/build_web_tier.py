"""Build the `web` raster tier: the screen set as logos usually reach a tracer.

Each icon's committed 512 px raster (`512ss`) goes through the pipeline a logo meets on its
way to a user: flattened onto a white page, resized by a non-integer factor with bicubic
interpolation (Pillow's, `a = -0.5`: blur and a slight overshoot at every edge), and saved
as a JPEG at quality 80 with 4:2:0 chroma subsampling (8x8 block quantisation, colour at half
resolution). The JPEG files themselves are committed under
`bench/data/corpus_raster/<family>/web/<stem>.jpg` and the tracer decodes them, so the gate
reads the same bytes on every machine whatever Pillow or libjpeg a later run has.

    python bench/build_web_tier.py            # (re)build every icon of the screen set
    python bench/build_web_tier.py --check    # report icons whose file is missing

The degradation follows the practical models of Zhang, Liang, Van Gool, Timofte (2021),
Designing a practical degradation model for deep blind image super-resolution, ICCV,
arXiv 2103.14006 (blur, resampling, JPEG), fixed to one realistic setting rather than drawn
at random, so a gate can compare builds on it.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

#: The tier's name, its source tier and its parameters (recorded in the files' names'
#: directory and in the gate's documentation).
TIER = "web"
SOURCE = "512ss"
SIZE = 400          # px; 512 -> 400 is a factor of 0.78125
QUALITY = 80
SUBSAMPLING = "4:2:0"


def build_one(src: Path, dst: Path) -> None:
    from PIL import Image
    im = Image.open(src).convert("RGBA")
    page = Image.new("RGBA", im.size, (255, 255, 255, 255))
    page.alpha_composite(im)
    out = page.convert("RGB").resize((SIZE, SIZE), Image.BICUBIC)
    dst.parent.mkdir(parents=True, exist_ok=True)
    out.save(dst, format="JPEG", quality=QUALITY, subsampling=SUBSAMPLING, optimize=False)


def main() -> int:
    import svgeval
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true")
    a = ap.parse_args()
    items = svgeval.load_sets()["screen"]
    base = ROOT / "bench" / "data" / "corpus_raster"
    missing = 0
    for it in items:
        src = base / it["corpus"] / SOURCE / f"{it['stem']}.png"
        dst = base / it["corpus"] / TIER / f"{it['stem']}.jpg"
        if a.check:
            missing += not dst.exists()
            continue
        build_one(src, dst)
    if a.check:
        print(f"{missing} of {len(items)} missing")
        return 1 if missing else 0
    print(f"wrote {len(items)} files under {base}/<family>/{TIER}/")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
