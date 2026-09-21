#!/usr/bin/env python3
"""Generate Inkvec Studio Lite's binary assets from the shapes that define them.

Everything this writes is derived, not drawn by hand, so that the app icon, the installer
bitmaps and the four bundled samples can be regenerated after any change to the marks
below rather than being opaque blobs in the tree.

    python3 studio/tools/make_assets.py            # writes into studio/
    python3 studio/tools/make_assets.py --check    # fails if anything is stale

The app icon is designed at 16 px first, which is a real constraint: it has to read in a
taskbar. At that size the three stair-stepped pixels drop to two and the copper curve
thickens, because a 1.5-px stroke at 16 px is a grey smudge. The larger sizes are the same
mark with the detail put back.

Dependencies: Pillow. ICNS is assembled here rather than left to Pillow, whose writer is
platform-dependent; the container is a magic word, a length and a run of tagged PNGs.
"""

from __future__ import annotations

import argparse
import hashlib
import math
import re
import struct
import sys
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
ICONS = ROOT / "src-tauri" / "icons"
INSTALLER = ROOT / "src-tauri" / "installer"
SAMPLES = ROOT / "src-tauri" / "samples"
ASSETS = ROOT / "src" / "assets"
# The Inkvec mark, as the site serves it. Copied rather than re-drawn so the app cannot
# show a mark the rest of the project has moved on from; `--check` fails if it has.
SITE_LOGO = ROOT.parent / "web" / "logo.svg"

# The palette, from the design system. Named here so the mark and the installer art
# cannot drift apart.
COPPER = (201, 117, 74)
CREAM = (250, 248, 245)
PIXEL_DIM = (154, 147, 140)
PIXEL_LIT = (207, 199, 191)
CARD = (42, 39, 36)
STAGE = (20, 18, 16)
VOID = (12, 10, 9)

# Supersampling factor. Everything is drawn large and reduced, which is where the
# anti-aliasing comes from — and anti-aliasing is the whole subject of this app, so the
# samples in particular have to have real coverage at their edges.
SS = 8


# --------------------------------------------------------------------------- drawing ---


def cubic(p0, p1, p2, p3, steps=96):
    """Points along one cubic Bezier."""
    out = []
    for i in range(steps + 1):
        t = i / steps
        u = 1 - t
        x = u**3 * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t**3 * p3[0]
        y = u**3 * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t**3 * p3[1]
        out.append((x, y))
    return out


def rounded_rect_mask(size, radius):
    m = Image.new("L", (size * SS, size * SS), 0)
    d = ImageDraw.Draw(m)
    d.rounded_rectangle(
        [0, 0, size * SS - 1, size * SS - 1], radius=radius * SS, fill=255
    )
    return m.resize((size, size), Image.LANCZOS)


def icon(size: int) -> Image.Image:
    """The app icon at `size` px.

    Three stair-stepped pixels and one copper curve cutting across them: a raster being
    read as a continuous edge, which is the product's whole argument in one mark.
    """
    small = size <= 20
    s = size * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    # Warm ground, lighter at the top left, as in the design sheet.
    for y in range(s):
        t = y / max(s - 1, 1)
        c = tuple(round(CARD[i] + (STAGE[i] - CARD[i]) * t) for i in range(3))
        d.line([(0, y), (s, y)], fill=c + (255,))

    # The mark is specified on a 48-unit grid.
    k = s / 48.0

    if small:
        # The 16 px cut, and the one the mark was designed against first. Sixteen device
        # pixels do not hold three 8-unit squares and a 3-unit stroke: an 8-unit square is
        # 2.7 px, which reads as grit. So the lowest step is dropped and everything that
        # is left grows until it is at least four pixels across — two 12-unit squares and
        # a 6-unit stroke, which is 4 px and 2 px at this size.
        squares = [(11, 24, PIXEL_LIT), (25, 11, PIXEL_DIM)]
        side = 12
        width = 6.0 * k
        curve = ((5, 40), (18, 40), (30, 27), (41, 8))
    else:
        squares = [(6, 26, PIXEL_DIM), (14, 20, PIXEL_LIT), (22, 14, PIXEL_DIM)]
        side = 8
        width = (4.0 if size <= 40 else 3.0) * k
        curve = ((6, 38), (18, 38), (30, 26), (38, 10))

    for x, y, colour in squares:
        d.rectangle(
            [x * k, y * k, (x + side) * k, (y + side) * k], fill=colour + (255,)
        )

    pts = cubic(*[(px * k, py * k) for px, py in curve])
    d.line(pts, fill=COPPER + (255,), width=round(width), joint="curve")
    r = width / 2
    for end in (pts[0], pts[-1]):
        d.ellipse(
            [end[0] - r, end[1] - r, end[0] + r, end[1] + r], fill=COPPER + (255,)
        )

    img = img.resize((size, size), Image.LANCZOS)
    # Platform icons are rounded rectangles. A small icon gets a proportionally tighter
    # corner, because at 16 px the standard radius eats the mark's own edges.
    radius = max(2, round(size * (0.14 if small else 0.1875)))
    img.putalpha(rounded_rect_mask(size, radius))
    return img


def icns(images: dict[str, Image.Image]) -> bytes:
    """Assemble an ICNS from tagged PNGs."""
    chunks = b""
    for tag, img in images.items():
        buf = _png_bytes(img)
        chunks += tag.encode("ascii") + struct.pack(">I", len(buf) + 8) + buf
    return b"icns" + struct.pack(">I", len(chunks) + 8) + chunks


def _png_bytes(img: Image.Image) -> bytes:
    import io

    b = io.BytesIO()
    img.save(b, format="PNG")
    return b.getvalue()


# ------------------------------------------------------------------------- installer ---


def installer_sidebar() -> Image.Image:
    """NSIS sidebar. 164 x 314, and that size is not negotiable."""
    w, h = 164, 314
    img = Image.new("RGB", (w * SS, h * SS), VOID)
    d = ImageDraw.Draw(img)
    for y in range(h * SS):
        t = (y / (h * SS)) ** 0.8
        c = tuple(round(CARD[i] + (VOID[i] - CARD[i]) * t) for i in range(3))
        d.line([(0, y), (w * SS, y)], fill=c)

    # A copper spotlight bleeding from the top right, as in the design.
    glow = Image.new("L", (w * SS, h * SS), 0)
    gd = ImageDraw.Draw(glow)
    cx, cy, rad = int(w * SS * 0.8), int(h * SS * 0.1), int(w * SS * 0.95)
    for i in range(48, 0, -1):
        r = rad * i / 48
        gd.ellipse([cx - r, cy - r, cx + r, cy + r], fill=int(56 * (1 - i / 48) ** 2))
    img = Image.composite(Image.new("RGB", img.size, COPPER), img, glow)

    mark = icon(64).resize((64 * SS, 64 * SS), Image.LANCZOS)
    img.paste(mark, (16 * SS, 22 * SS), mark)

    img = img.resize((w, h), Image.LANCZOS)
    d = ImageDraw.Draw(img)
    _text_block(d, 16, h - 60, ["Inkvec", "Studio Lite"], CREAM, 15, serif=True)
    _text_block(d, 16, h - 24, ["Raster to SVG, exactly"], PIXEL_DIM, 9)
    return img


def installer_header() -> Image.Image:
    """NSIS header. 150 x 57, also fixed."""
    w, h = 150, 57
    img = Image.new("RGB", (w, h), STAGE)
    mark = icon(26)
    img.paste(mark, (12, (h - 26) // 2), mark)
    d = ImageDraw.Draw(img)
    _text_block(d, 46, 18, ["Inkvec", "Studio Lite"], CREAM, 10)
    return img


def _text_block(draw, x, y, lines, colour, size, serif=False):
    """Draw a run of lines with whatever font is actually available.

    Deliberately forgiving: these two bitmaps are built on whatever machine runs this
    script, and a missing font must not stop the assets being produced. The layout is
    what carries the design; the typeface is a bonus.
    """
    from PIL import ImageFont

    candidates = (
        [
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
            "/System/Library/Fonts/Supplemental/Georgia.ttf",
            "C:/Windows/Fonts/georgia.ttf",
        ]
        if serif
        else [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "C:/Windows/Fonts/arial.ttf",
        ]
    )
    font = None
    for path in candidates:
        if Path(path).exists():
            font = ImageFont.truetype(path, size)
            break
    if font is None:
        font = ImageFont.load_default()
    for i, line in enumerate(lines):
        draw.text((x, y + i * (size + 3)), line, fill=colour, font=font)


# --------------------------------------------------------------------------- samples ---


def sample_flat_logo() -> Image.Image:
    """A flat mark: a few large faces, hard edges. The default case."""
    s = 512 * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    u = s / 200.0
    d.ellipse([22 * u, 22 * u, 178 * u, 178 * u], fill=(20, 69, 63, 255))
    d.ellipse([74 * u, 48 * u, 126 * u, 100 * u], fill=(231, 176, 74, 255))
    d.polygon(
        [(30 * u, 148 * u), (74 * u, 92 * u), (104 * u, 130 * u), (128 * u, 104 * u), (170 * u, 148 * u)],
        fill=(245, 242, 234, 255),
    )
    d.polygon(
        [(104 * u, 130 * u), (128 * u, 104 * u), (150 * u, 128 * u), (118 * u, 128 * u)],
        fill=(207, 198, 180, 255),
    )
    return img.resize((512, 512), Image.LANCZOS)


def sample_icon_64() -> Image.Image:
    """A small flat mark at the size icons are actually drawn: 64 px.

    Not a downscale of something larger — at 64 px the anti-aliasing *is* the drawing,
    which is exactly the case the speckle floor and the Icon preset exist for.
    """
    s = 64 * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    u = s / 200.0
    d.rounded_rectangle([28 * u, 28 * u, 172 * u, 172 * u], radius=30 * u, fill=(31, 42, 68, 255))
    d.line(
        [(64 * u, 128 * u), (100 * u, 64 * u), (136 * u, 128 * u)],
        fill=(232, 228, 220, 255),
        width=round(10 * u),
        joint="curve",
    )
    return img.resize((64, 64), Image.LANCZOS)


def sample_crest() -> Image.Image:
    """Filigree: thin concentric strokes and a fine curve. The Fine-detail case."""
    s = 512 * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    u = s / 200.0
    gold = (176, 138, 74, 255)
    d.ellipse([28 * u, 28 * u, 172 * u, 172 * u], outline=gold, width=round(4 * u))
    d.ellipse([42 * u, 42 * u, 158 * u, 158 * u], outline=gold, width=round(2 * u))
    d.line(
        cubic((74 * u, 118 * u), (90 * u, 70 * u), (110 * u, 70 * u), (126 * u, 118 * u)),
        fill=gold,
        width=round(3 * u),
        joint="curve",
    )
    for i in range(12):
        a = i / 12 * math.tau
        cx, cy = 100 * u + 64 * u * math.cos(a), 100 * u + 64 * u * math.sin(a)
        r = 3 * u
        d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=gold)
    return img.resize((512, 512), Image.LANCZOS)


def sample_signature() -> Image.Image:
    """A signature in one ink: the Black & white case, and a JPEG-ish grey ground."""
    s = 512 * SS
    img = Image.new("RGBA", (s, s), (255, 255, 255, 255))
    d = ImageDraw.Draw(img)
    u = s / 200.0
    ink = (26, 24, 22, 255)
    d.line(
        cubic((40 * u, 132 * u), (66 * u, 44 * u), (128 * u, 44 * u), (152 * u, 132 * u)),
        fill=ink,
        width=round(7 * u),
        joint="curve",
    )
    d.line([(58 * u, 106 * u), (140 * u, 106 * u)], fill=ink, width=round(7 * u))
    d.line(
        cubic((146 * u, 126 * u), (158 * u, 150 * u), (120 * u, 154 * u), (108 * u, 140 * u)),
        fill=ink,
        width=round(5 * u),
        joint="curve",
    )
    return img.resize((512, 512), Image.LANCZOS)


SAMPLE_SET = {
    "flat-logo.png": sample_flat_logo,
    "icon-64.png": sample_icon_64,
    "crest-filigree.png": sample_crest,
    "signature-bw.png": sample_signature,
}


# ----------------------------------------------------------------------------- main ---


def mono_mark() -> bytes:
    """`web/logo.svg` as one flat colour, for the app and for a README.

    Mono is already this project's treatment for the mark: the documentation site
    recolours every fill to the copper accent. This does the same, except that the ink
    becomes `currentColor` with the copper set as the root's `color`. Both readings are
    then true of one file. Rendered on its own — a README image, a file preview — it is
    the copper mono mark the site shows. Inside the app, `appMark` drops the `color` and
    the mark takes whatever the text around it is using, so eighteen pixels in the
    chrome and seventy-two on About are each the right colour and the light theme needs
    no second file.
    """
    svg = SITE_LOGO.read_text(encoding="utf-8").strip()
    inks = re.findall(r'fill="#[0-9a-fA-F]{3,8}"', svg)
    if not inks:
        raise SystemExit(f"{SITE_LOGO} has no solid fill to make mono")
    for ink in set(inks):
        svg = svg.replace(ink, 'fill="currentColor"')
    # The width and height are the app's to choose; the viewBox is the drawing's.
    svg = re.sub(r'\s(width|height)="[^"]*"', "", svg)
    copper = "#%02x%02x%02x" % COPPER
    svg = svg.replace("<svg ", f'<svg color="{copper}" ', 1)
    header = (
        "<!-- Generated by studio/tools/make_assets.py from web/logo.svg."
        " Do not edit. -->\n"
    )
    return (header + svg + "\n").encode("utf-8")


def write(path: Path, data: bytes, check: bool, stale: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists() and hashlib.sha256(path.read_bytes()).digest() == hashlib.sha256(data).digest():
        return
    if check:
        stale.append(str(path.relative_to(ROOT.parent)))
        return
    path.write_bytes(data)
    print(f"  wrote {path.relative_to(ROOT.parent)} ({len(data)} bytes)")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--check",
        action="store_true",
        help="do not write; exit non-zero if any asset would change",
    )
    args = ap.parse_args()
    stale: list[str] = []

    # Icons. Tauri reads these five by name; the ICO and ICNS carry the whole set.
    for size, name in ((32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png")):
        write(ICONS / name, _png_bytes(icon(size)), args.check, stale)

    import io

    ico = io.BytesIO()
    icon(256).save(
        ico, format="ICO", sizes=[(16, 16), (32, 32), (48, 48), (256, 256)]
    )
    write(ICONS / "icon.ico", ico.getvalue(), args.check, stale)

    write(
        ICONS / "icon.icns",
        icns(
            {
                "icp4": icon(16),
                "icp5": icon(32),
                "icp6": icon(64),
                "ic07": icon(128),
                "ic08": icon(256),
                "ic09": icon(512),
                "ic10": icon(1024),
                "ic11": icon(32),
                "ic12": icon(64),
                "ic13": icon(256),
                "ic14": icon(512),
            }
        ),
        args.check,
        stale,
    )

    # A square PNG for the Linux desktop entry and the About screen.
    write(ICONS / "icon.png", _png_bytes(icon(512)), args.check, stale)

    # Installer bitmaps, at NSIS's exact sizes.
    for name, img in (
        ("sidebar.bmp", installer_sidebar()),
        ("header.bmp", installer_header()),
    ):
        b = io.BytesIO()
        img.convert("RGB").save(b, format="BMP")
        write(INSTALLER / name, b.getvalue(), args.check, stale)

    # The mark the app draws in its bar and on About, mono.
    write(ASSETS / "mark.svg", mono_mark(), args.check, stale)

    # The four bundled samples.
    for name, make in SAMPLE_SET.items():
        write(SAMPLES / name, _png_bytes(make()), args.check, stale)

    if stale:
        print("These assets are stale; re-run without --check:", file=sys.stderr)
        for s in stale:
            print(f"  {s}", file=sys.stderr)
        return 1
    print("assets up to date" if args.check else "assets written")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
