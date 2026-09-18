"""Build wide, GitHub-readable README visuals from verified local assets and metrics."""
import io
from pathlib import Path
from urllib.request import urlopen

import numpy as np
from PIL import Image, ImageDraw, ImageFont
import resvg_py

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "docs" / "assets"
PLAYFAIR_URL = "https://github.com/google/fonts/raw/main/ofl/playfairdisplay/PlayfairDisplay%5Bwght%5D.ttf"
W, H = 1600, 720
BG = (23, 20, 17)
CREAM = (250, 248, 245)
MUTED = (179, 168, 157)
COPPER = (201, 117, 74)
GOLD = (184, 151, 108)
MINT = (52, 211, 153)


def font(size: int, semibold: bool = False) -> ImageFont.FreeTypeFont:
    f = ImageFont.truetype(io.BytesIO(urlopen(PLAYFAIR_URL, timeout=30).read()), size)
    if semibold:
        try:
            f.set_variation_by_name("SemiBold")
        except OSError:
            pass
    return f


def sans(size: int, bold: bool = False) -> ImageFont.FreeTypeFont:
    name = "segoeuib.ttf" if bold else "segoeui.ttf"
    return ImageFont.truetype(f"C:/Windows/Fonts/{name}", size)


def canvas(width: int, height: int) -> Image.Image:
    yy, xx = np.mgrid[:height, :width]
    # Warm spotlight behind the headline; deliberately restrained so README text stays legible.
    glow = np.exp(-(((xx - width * 0.62) / (width * 0.55)) ** 2 + ((yy - height * 0.1) / (height * 0.46)) ** 2) * 2.2)
    image = np.empty((height, width, 4), dtype=np.uint8)
    image[:, :, 0] = np.clip(BG[0] + glow * 19, 0, 255)
    image[:, :, 1] = np.clip(BG[1] + glow * 10, 0, 255)
    image[:, :, 2] = np.clip(BG[2] + glow * 5, 0, 255)
    image[:, :, 3] = 255
    return Image.fromarray(image, "RGBA")


def white_brand(width: int) -> Image.Image:
    logo = Image.open(ASSETS / "logolabs-brand-lockup.png").convert("RGBA")
    h = round(logo.height * width / logo.width)
    logo = logo.resize((width, h), Image.Resampling.LANCZOS)
    data = np.asarray(logo).copy()
    data[:, :, :3] = 250
    return Image.fromarray(data, "RGBA")


def centered_brand(image: Image.Image, y: int, width: int = 250) -> None:
    logo = white_brand(width)
    image.alpha_composite(logo, ((image.width - logo.width) // 2, y))


def rounded(draw: ImageDraw.ImageDraw, box: tuple[int, int, int, int], fill, outline=None) -> None:
    draw.rounded_rectangle(box, radius=22, fill=fill, outline=outline, width=2)


def fit_cover(image: Image.Image, size: tuple[int, int]) -> Image.Image:
    ratio = max(size[0] / image.width, size[1] / image.height)
    scaled = image.resize((round(image.width * ratio), round(image.height * ratio)), Image.Resampling.LANCZOS)
    left = (scaled.width - size[0]) // 2
    top = (scaled.height - size[1]) // 2
    return scaled.crop((left, top, left + size[0], top + size[1]))


def render_svg(path: Path, size: int) -> Image.Image:
    raw = resvg_py.svg_to_bytes(svg_path=str(path), width=size, height=size)
    return Image.open(io.BytesIO(raw)).convert("RGBA")


def hero() -> None:
    image = canvas(W, H)
    draw = ImageDraw.Draw(image)
    centered_brand(image, 30)
    draw.rounded_rectangle((560, 103, 1040, 137), radius=17, fill=(*COPPER, 255))
    label = "OPEN-SOURCE RASTER  →  VECTOR"
    box = draw.textbbox((0, 0), label, font=sans(15, True))
    draw.text(((W - (box[2] - box[0])) // 2, 111), label, font=sans(15, True), fill=CREAM)
    draw.text((84, 188), "Inkvec", font=font(102, True), fill=CREAM)
    draw.text((89, 306), "Pixels in. Precise, editable SVG out.", font=sans(29), fill=CREAM)
    detail = "Trace logos, icons, and flat artwork with shared boundaries,\nnative primitives, and a compact curve model."
    draw.multiline_text((89, 354), detail, font=sans(20), fill=MUTED, spacing=8)

    # A genuine source/trace pair, not a mockup.
    source = fit_cover(Image.open(ASSETS / "example.png").convert("RGBA"), (250, 250))
    traced = fit_cover(render_svg(ASSETS / "github-hero-example.svg", 512), (250, 250))
    for x, title, card in [(906, "RASTER INPUT", source), (1208, "INKVEC SVG", traced)]:
        rounded(draw, (x - 10, 208, x + 260, 528), (31, 28, 24, 255), (74, 66, 57, 255))
        draw.text((x, 224), title, font=sans(13, True), fill=GOLD if title == "RASTER INPUT" else MINT)
        image.alpha_composite(card, (x, 262))
    draw.text((1174, 361), "→", font=font(46), fill=COPPER)

    stats = [("0.119", "MEAN dE00 ↓", COPPER), ("0.992", "DINO ↑", MINT), ("585", "COORDINATES", GOLD), ("1.17 s", "PER IMAGE", CREAM)]
    x = 86
    for value, caption, colour in stats:
        rounded(draw, (x, 558, x + 332, 670), (31, 28, 24, 255), (66, 59, 51, 255))
        draw.text((x + 24, 575), value, font=font(42, True), fill=colour)
        draw.text((x + 24, 630), caption, font=sans(13, True), fill=MUTED)
        x += 358
    image.convert("RGB").save(ASSETS / "github-hero.png", "PNG", optimize=True)


def benchmarks() -> None:
    image = canvas(1600, 540)
    draw = ImageDraw.Draw(image)
    centered_brand(image, 26, 210)
    draw.text((74, 108), "Quality without coordinate sprawl.", font=font(58, True), fill=CREAM)
    draw.text((78, 181), "21 hash-selected real logos and icons · lower error is better", font=sans(19), fill=MUTED)
    headers = [("Inkvec", "0.119", "mean dE00", "585", "coordinates", COPPER),
               ("VTracer", "1.303", "mean dE00", "1,943", "coordinates", (181, 168, 157)),
               ("VTracer tuned", "1.264", "mean dE00", "1,239", "coordinates", (181, 168, 157)),
               ("Trazor", "0.518", "mean dE00", "1,172", "coordinates", (181, 168, 157))]
    x = 74
    for name, error, e_label, coords, c_label, colour in headers:
        rounded(draw, (x, 244, x + 350, 457), (46, 34, 27, 255) if name == "Inkvec" else (31, 28, 24, 255), colour if name == "Inkvec" else (66, 59, 51, 255))
        draw.text((x + 24, 266), name, font=font(27, True), fill=CREAM)
        draw.text((x + 24, 320), error, font=font(42, True), fill=colour)
        draw.text((x + 24, 370), e_label.upper(), font=sans(12, True), fill=MUTED)
        draw.text((x + 204, 324), coords, font=sans(27, True), fill=CREAM)
        draw.text((x + 204, 370), c_label.upper(), font=sans(12, True), fill=MUTED)
        x += 378
    draw.text((76, 488), "Full methodology and all engine versions: docs/results/2026-09-15.md", font=sans(16), fill=MUTED)
    image.convert("RGB").save(ASSETS / "github-benchmarks.png", "PNG", optimize=True)


def pipeline() -> None:
    image = canvas(1600, 470)
    draw = ImageDraw.Draw(image)
    centered_brand(image, 23, 190)
    draw.text((78, 96), "A trace is a geometric explanation of the pixels.", font=font(47, True), fill=CREAM)
    steps = [("01", "Read", "pixels + alpha"), ("02", "Map", "regions + edges"),
             ("03", "Solve", "sub-pixel boundaries"), ("04", "Fit", "lines, curves, arcs"),
             ("05", "Emit", "compact SVG")]
    x = 72
    for i, (number, name, detail) in enumerate(steps):
        rounded(draw, (x, 205, x + 272, 373), (31, 28, 24, 255), (66, 59, 51, 255))
        draw.text((x + 24, 225), number, font=sans(14, True), fill=COPPER)
        draw.text((x + 24, 259), name, font=font(32, True), fill=CREAM)
        draw.text((x + 24, 318), detail, font=sans(16), fill=MUTED)
        if i < len(steps) - 1:
            draw.text((x + 286, 271), "→", font=font(30), fill=GOLD)
        x += 310
    draw.text((80, 414), "Minimum description length keeps detail that explains the image and rejects coordinates that only explain noise.", font=sans(17), fill=MUTED)
    image.convert("RGB").save(ASSETS / "github-pipeline.png", "PNG", optimize=True)


if __name__ == "__main__":
    hero(); benchmarks(); pipeline()
    for name in ("github-hero.png", "github-benchmarks.png", "github-pipeline.png"):
        p = ASSETS / name
        print(f"wrote {p} ({p.stat().st_size:,} bytes)")
