"""Write the contract fixture images. Run rarely: only to change the inputs themselves.

    python bindings/contract/make_fixtures.py

The expectations in cases.json are produced by the Rust facade, not by this script:

    INKVEC_BLESS=1 cargo test -p inkvec --release --test contract

Inputs written here:

  tiny.png            copy of web/samples/tiny.png (96 x 96, opaque RGB logo)
  tiny.rgba           the same pixels as raw straight RGBA8, row-major (96 * 96 * 4 bytes)
  white_on_clear.png  64 x 64 white artwork on a fully transparent ground, anti-aliased
  white_on_clear.rgba the same pixels as raw straight RGBA8 (64 * 64 * 4 bytes)

Needs Pillow.
"""

from __future__ import annotations

import shutil
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent


def white_on_clear(size: int = 64, ss: int = 8) -> Image.Image:
    """A white ring and a white bar on transparency, drawn at `ss`x and box-filtered down,
    so the edges carry real partial alpha rather than a hard mask."""
    big = size * ss
    im = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    white = (255, 255, 255, 255)
    d.ellipse([6 * ss, 6 * ss, 42 * ss, 42 * ss], fill=white)
    d.ellipse([14 * ss, 14 * ss, 34 * ss, 34 * ss], fill=(0, 0, 0, 0))
    d.rounded_rectangle([30 * ss, 44 * ss, 58 * ss, 56 * ss], radius=4 * ss, fill=white)
    # Pillow resamples RGBA premultiplied, so the colour of the transparent ground cannot
    # bleed into the edges.
    return im.resize((size, size), Image.Resampling.BOX)


def write_rgba(png: Path) -> None:
    im = Image.open(png).convert("RGBA")
    png.with_suffix(".rgba").write_bytes(im.tobytes())
    print(f"wrote {png.with_suffix('.rgba').name} ({im.size[0]}x{im.size[1]})")


def main() -> None:
    tiny = HERE / "tiny.png"
    shutil.copyfile(ROOT / "web" / "samples" / "tiny.png", tiny)
    print("wrote tiny.png")
    clear = HERE / "white_on_clear.png"
    white_on_clear().save(clear, optimize=True)
    print("wrote white_on_clear.png")
    write_rgba(tiny)
    write_rgba(clear)


if __name__ == "__main__":
    main()
