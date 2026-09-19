"""Generate the Hugging Face model-card assets for inkvec-sr-001.

Mirrors the denoiser card's treatment (tools/make_denoiser_masthead.py and the
HF repo's assets/): a 1600x500 masthead, a 900x220 colophon, plus the shared
brand lockup and EuroHPC acknowledgement banner copied from docs/assets.
Outputs land in out/hf_current_sr/assets/, which tools/upload_sr_model.py ships.
"""
import io
import shutil
from pathlib import Path
from urllib.request import urlopen

import numpy as np
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "docs" / "assets"
OUT = ROOT / "out" / "hf_current_sr" / "assets"
PLAYFAIR_URL = "https://github.com/google/fonts/raw/main/ofl/playfairdisplay/PlayfairDisplay%5Bwght%5D.ttf"

BG = (23, 20, 17)
CREAM = (250, 248, 245)
MUTED = (179, 168, 157)
COPPER = (201, 117, 74)
GOLD = (184, 151, 108)
MINT = (52, 211, 153)
PILL_TEXT = (250, 218, 196)
CARD_FILL = (31, 28, 24, 255)
CARD_EDGE = (66, 59, 51, 255)


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


def centered(draw: ImageDraw.ImageDraw, y: int, text: str, fnt, fill) -> None:
    box = draw.textbbox((0, 0), text, font=fnt)
    draw.text(((1600 - (box[2] - box[0])) // 2 - box[0], y - box[1]), text, font=fnt, fill=fill)


def masthead() -> None:
    image = canvas(1600, 500)
    draw = ImageDraw.Draw(image)

    brand = white_brand(180)
    image.alpha_composite(brand, ((1600 - brand.width) // 2, 14))

    label = "RESEARCH RELEASE   ·   MAMBAIRV2-SMALL   ·   9.77M PARAMS   ·   X4"
    label_f = sans(12, True)
    box = draw.textbbox((0, 0), label, font=label_f)
    pw = box[2] - box[0] + 56
    draw.rounded_rectangle(((1600 - pw) // 2, 78, (1600 + pw) // 2, 106), radius=14, fill=(*COPPER, 255))
    draw.text(((1600 - (box[2] - box[0])) // 2, 84), label, font=label_f, fill=PILL_TEXT)

    centered(draw, 130, "Inkvec Super-Resolution", font(84, True), CREAM)
    centered(draw, 244, "Logolabs / inkvec-sr-001", sans(26, True), GOLD)
    centered(draw, 288, "A MambaIRv2 x4 upscaler that sharpens edges and lifts compression damage before vector tracing.",
             sans(21), MUTED)

    cards = [("-56%", "Colour Error (dE00)", "1.00 \u2192 0.44 on JPEG q50", COPPER),
             ("3.76\u00d7", "Perceptual Score", "LPIPS 0.084 \u2192 0.022", MINT),
             ("-66%", "Vector Parameters", "19.8\u00d7 \u2192 6.8\u00d7 the artist's count", COPPER),
             ("Apache 2.0", "Open Weights & Code", "Commercial & research ready", GOLD)]
    cw, ch, gap = 350, 128, 28
    x = (1600 - (4 * cw + 3 * gap)) // 2
    for value, caption, detail, colour in cards:
        draw.rounded_rectangle((x, 348, x + cw, 348 + ch), radius=22, fill=CARD_FILL, outline=CARD_EDGE, width=2)
        draw.text((x + 26, 366), value, font=font(42, True), fill=colour)
        draw.text((x + 26, 424), caption, font=sans(15, True), fill=CREAM)
        draw.text((x + 26, 450), detail, font=sans(13), fill=MUTED)
        x += cw + gap

    out = OUT / "masthead.png"
    image.convert("RGB").save(out, "PNG", optimize=True)
    print(f"wrote {out} ({out.stat().st_size:,} bytes)")


def colophon() -> None:
    image = Image.new("RGBA", (900, 220), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((3, 3, 897, 217), radius=18, fill=(20, 18, 16, 255), outline=(60, 53, 46, 255), width=2)

    # Copper ring mark with a solid square core, as on the denoiser colophon.
    draw.ellipse((428, 30, 472, 74), outline=(*COPPER, 255), width=3)
    draw.rectangle((444, 46, 456, 58), fill=(*COPPER, 255))

    centered_col = ImageDraw.Draw(image)
    text = "LogoLabs   \u00b7   Inkvec Super-Resolution 001"
    box = centered_col.textbbox((0, 0), text, font=font(30, True))
    centered_col.text(((900 - (box[2] - box[0])) // 2, 92), text, font=font(30, True), fill=CREAM)

    text = "Deleanu, Stefan-Lucian   \u00b7   LogoLabs Research"
    box = centered_col.textbbox((0, 0), text, font=sans(18, True))
    centered_col.text(((900 - (box[2] - box[0])) // 2, 136), text, font=sans(18, True), fill=(205, 178, 148))

    text = "Released under the Apache 2.0 Open Source Licence   \u00b7   2026"
    box = centered_col.textbbox((0, 0), text, font=sans(16))
    centered_col.text(((900 - (box[2] - box[0])) // 2, 172), text, font=sans(16), fill=(139, 130, 120))

    out = OUT / "colophon.png"
    image.save(out, "PNG", optimize=True)
    print(f"wrote {out} ({out.stat().st_size:,} bytes)")


def shared() -> None:
    for name in ("logolabs-brand-lockup.png", "acknowledgment-banner.png"):
        shutil.copyfile(ASSETS / name, OUT / name)
        print(f"copied {name}")


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    masthead()
    colophon()
    shared()
