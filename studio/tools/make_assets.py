#!/usr/bin/env python3
"""Generate Inkvec Studio's binary assets from the shapes that define them.

Everything this writes is derived, not drawn by hand, so that the app icon, the installer
bitmaps and the four bundled samples can be regenerated after any change to the marks
below rather than being opaque blobs in the tree.

    python3 studio/tools/make_assets.py            # writes into studio/
    python3 studio/tools/make_assets.py --check    # fails if anything is stale

The app icon is the Inkvec droplet from `web/logo.svg`, in copper on a transparent
ground, rasterised here from the same path the app draws in its bar. Nothing is copied by
hand: change the mark and every icon follows.

Dependencies: Pillow. ICNS is assembled here rather than left to Pillow, whose writer is
platform-dependent; the container is a magic word, a length and a run of tagged PNGs.
"""

from __future__ import annotations

import argparse
import functools
import hashlib
import math
import re
import struct
import sys
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw

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


class _Scan:
    """Just enough of an SVG path reader for the mark: numbers, and arc flags.

    Arc flags are single characters that may be run together with the number after them
    (`0 00-67.1`), which is why a generic split on whitespace and signs is not enough.
    """

    _NUM = re.compile(r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")

    def __init__(self, d: str):
        self.d = d
        self.i = 0

    def _skip(self) -> None:
        while self.i < len(self.d) and self.d[self.i] in " \t\r\n,":
            self.i += 1

    def command(self) -> str | None:
        self._skip()
        if self.i < len(self.d) and self.d[self.i].isalpha():
            c = self.d[self.i]
            self.i += 1
            return c
        return None

    def more(self) -> bool:
        self._skip()
        return self.i < len(self.d) and not self.d[self.i].isalpha()

    def num(self) -> float:
        self._skip()
        m = self._NUM.match(self.d, self.i)
        if not m:
            raise ValueError(f"expected a number at {self.i} in the mark's path")
        self.i = m.end()
        return float(m.group())

    def flag(self) -> bool:
        self._skip()
        c = self.d[self.i]
        self.i += 1
        return c == "1"


def _arc(p0, rx, ry, phi_deg, large, sweep, p1, steps_per_turn=64):
    """Points along an SVG elliptical arc, endpoint form (SVG 1.1, F.6.5)."""
    (x1, y1), (x2, y2) = p0, p1
    if rx == 0 or ry == 0 or p0 == p1:
        return [p1]
    phi = math.radians(phi_deg)
    cp, sp = math.cos(phi), math.sin(phi)
    dx, dy = (x1 - x2) / 2, (y1 - y2) / 2
    x1p, y1p = cp * dx + sp * dy, -sp * dx + cp * dy
    rx, ry = abs(rx), abs(ry)
    lam = x1p**2 / rx**2 + y1p**2 / ry**2
    if lam > 1:
        rx, ry = rx * math.sqrt(lam), ry * math.sqrt(lam)
    num = rx**2 * ry**2 - rx**2 * y1p**2 - ry**2 * x1p**2
    den = rx**2 * y1p**2 + ry**2 * x1p**2
    co = math.sqrt(max(0.0, num / den)) * (-1 if large == sweep else 1)
    cxp, cyp = co * rx * y1p / ry, -co * ry * x1p / rx
    cx = cp * cxp - sp * cyp + (x1 + x2) / 2
    cy = sp * cxp + cp * cyp + (y1 + y2) / 2

    def angle(ux, uy, vx, vy):
        a = math.atan2(ux * vy - uy * vx, ux * vx + uy * vy)
        return a

    t1 = angle(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry)
    dt = angle((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry)
    if not sweep and dt > 0:
        dt -= 2 * math.pi
    elif sweep and dt < 0:
        dt += 2 * math.pi
    n = max(4, math.ceil(abs(dt) / (2 * math.pi) * steps_per_turn))
    out = []
    for k in range(1, n + 1):
        t = t1 + dt * k / n
        ex, ey = rx * math.cos(t), ry * math.sin(t)
        out.append((cp * ex - sp * ey + cx, sp * ex + cp * ey + cy))
    out[-1] = p1
    return out


def _path_polygons(d: str) -> list[list[tuple[float, float]]]:
    """Every subpath of `d` as a polygon. Lines, cubics and arcs; that is all the mark uses."""
    sc = _Scan(d)
    polys: list[list[tuple[float, float]]] = []
    cur = start = (0.0, 0.0)
    poly: list[tuple[float, float]] = []
    cmd = None
    while True:
        c = sc.command()
        if c is None:
            if not sc.more():
                break
            c = cmd  # implicit repeat; after a moveto that means lineto
            if c in ("M", "m"):
                c = "L" if c == "M" else "l"
        cmd = c
        rel = c.islower()
        u = c.upper()
        if u == "Z":
            if poly:
                polys.append(poly)
            poly, cur = [], start
            continue
        if u == "M":
            if poly:
                polys.append(poly)
            x, y = sc.num(), sc.num()
            cur = (cur[0] + x, cur[1] + y) if rel else (x, y)
            start = cur
            poly = [cur]
        elif u == "L":
            x, y = sc.num(), sc.num()
            cur = (cur[0] + x, cur[1] + y) if rel else (x, y)
            poly.append(cur)
        elif u == "H":
            x = sc.num()
            cur = (cur[0] + x if rel else x, cur[1])
            poly.append(cur)
        elif u == "V":
            y = sc.num()
            cur = (cur[0], cur[1] + y if rel else y)
            poly.append(cur)
        elif u == "C":
            v = [sc.num() for _ in range(6)]
            if rel:
                v = [v[i] + cur[i % 2] for i in range(6)]
            poly += cubic(cur, (v[0], v[1]), (v[2], v[3]), (v[4], v[5]), steps=24)[1:]
            cur = (v[4], v[5])
        elif u == "A":
            rx, ry, rot = sc.num(), sc.num(), sc.num()
            large, sweep = sc.flag(), sc.flag()
            x, y = sc.num(), sc.num()
            end = (cur[0] + x, cur[1] + y) if rel else (x, y)
            poly += _arc(cur, rx, ry, rot, large, sweep, end)
            cur = end
        else:
            raise ValueError(f"the mark's path uses '{c}', which this reader does not implement")
    if poly:
        polys.append(poly)
    return polys


@functools.lru_cache(maxsize=None)
def _mark_geometry():
    """The mark's polygons and its viewBox, from the same SVG the app draws in its bar."""
    svg = mono_mark().decode("utf-8")
    vb = re.search(r'viewBox="([^"]+)"', svg)
    box = tuple(float(v) for v in vb.group(1).split())
    polys = [p for d in re.findall(r'<path[^>]*\sd="([^"]+)"', svg) for p in _path_polygons(d)]
    return polys, box


def mark_image(height: int, ss: int | None = None) -> Image.Image:
    """The mark in copper on a transparent ground, exactly `height` px tall and as wide as it is.

    Filled even-odd, like the SVG: each subpath is drawn into its own bitmap and the
    bitmaps are XORed, which is what turns the head and the chevron into holes.
    """
    polys, (vx, vy, vw, vh) = _mark_geometry()
    ss = ss or (SS if height <= 256 else 4)
    width = max(1, round(height * vw / vh))
    k = height * ss / vh
    acc = None
    for poly in polys:
        m = Image.new("1", (width * ss, height * ss), 0)
        ImageDraw.Draw(m).polygon([((x - vx) * k, (y - vy) * k) for x, y in poly], fill=1)
        acc = m if acc is None else ImageChops.logical_xor(acc, m)
    alpha = acc.convert("L").resize((width, height), Image.LANCZOS)
    img = Image.new("RGBA", (width, height), COPPER + (0,))
    img.putalpha(alpha)
    return img


def icon(size: int) -> Image.Image:
    """The app icon at `size` px: the Inkvec droplet, copper, on a transparent ground.

    No tile. Windows and Linux draw an icon straight onto the taskbar, and a copper mark
    with nothing behind it reads on both a light one and a dark one. The mark is taller
    than it is wide, so its *height* sets the size, with a small margin so the tip and the
    base are not clipped by whatever draws the icon.
    """
    margin = 0.04 if size <= 20 else 0.06
    mark = mark_image(max(1, round(size * (1 - 2 * margin))))
    img = Image.new("RGBA", (size, size), COPPER + (0,))
    img.alpha_composite(mark, ((size - mark.width) // 2, (size - mark.height) // 2))
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

    mark = mark_image(64 * SS, ss=2)
    img.paste(mark, (16 * SS, 22 * SS), mark)

    img = img.resize((w, h), Image.LANCZOS)
    d = ImageDraw.Draw(img)
    _text_block(d, 16, h - 60, ["Inkvec", "Studio"], CREAM, 15, serif=True)
    _text_block(d, 16, h - 24, ["Raster to SVG, exactly"], PIXEL_DIM, 9)
    return img


def installer_header() -> Image.Image:
    """NSIS header. 150 x 57, also fixed."""
    w, h = 150, 57
    img = Image.new("RGB", (w, h), STAGE)
    mark = mark_image(30)
    img.paste(mark, (14, (h - mark.height) // 2), mark)
    d = ImageDraw.Draw(img)
    _text_block(d, 46, 18, ["Inkvec", "Studio"], CREAM, 10)
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
