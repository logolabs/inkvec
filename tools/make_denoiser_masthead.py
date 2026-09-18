"""Apply the approved white LogoLabs lockup to the Denoiser model-card masthead."""
import io
from pathlib import Path
from urllib.request import urlopen

from huggingface_hub import hf_hub_download
from PIL import Image, ImageDraw, ImageFont
import resvg_py

ROOT = Path(__file__).resolve().parents[1]
MODEL = "Logolabs/inkvec-denoiser-001"
BASE_REVISION = "bbb988646d10c19bca1d7d37333e0ba8512853aa"
MARK = Path(r"T:\logolabs-design\public\img\logo-icon.svg")
OUT = ROOT / "docs" / "assets" / "denoiser-masthead.png"
PLAYFAIR_URL = "https://github.com/google/fonts/raw/main/ofl/playfairdisplay/PlayfairDisplay%5Bwght%5D.ttf"
WHITE = (250, 248, 245, 255)


def white_mark(height: int) -> Image.Image:
    svg = MARK.read_text(encoding="utf-8").replace("currentColor", "#faf8f5")
    raw = resvg_py.svg_to_bytes(svg_string=svg, height=height * 4)
    image = Image.open(io.BytesIO(raw)).convert("RGBA")
    width = round(image.width * height / image.height)
    return image.resize((width, height), Image.Resampling.LANCZOS)


base = Image.open(hf_hub_download(MODEL, "assets/masthead.png", revision=BASE_REVISION)).convert("RGBA")
mark = white_mark(36)
font = ImageFont.truetype(io.BytesIO(urlopen(PLAYFAIR_URL, timeout=30).read()), 36)
try:
    font.set_variation_by_name("SemiBold")
except OSError:
    pass

# Center the brand first. The original masthead's product information begins
# below, so the mark reads as an owner lockup rather than a competing title.
draw = ImageDraw.Draw(base)
text_box = draw.textbbox((0, 0), "LogoLabs", font=font)
brand_width = mark.width + 14 + text_box[2] - text_box[0]
x, y = (base.width - brand_width) // 2, 13
base.alpha_composite(mark, (x, y))
draw.text((x + mark.width + 14, y - text_box[1] - 2), "LogoLabs", font=font, fill=WHITE)

# Shorten the release pill now that the owner is stated above it.
badge = (535, 50, 1065, 76)
draw.rounded_rectangle(badge, radius=13, fill=(201, 117, 74, 255))
label_font = ImageFont.truetype("C:/Windows/Fonts/segoeuib.ttf", 12)
label = "RESEARCH RELEASE   ·   OPSET 17 ONNX   ·   19.7M PARAMS"
label_box = draw.textbbox((0, 0), label, font=label_font)
draw.text(((base.width - (label_box[2] - label_box[0])) // 2, 56), label,
          font=label_font, fill=(250, 218, 196, 255))
base.save(OUT, "PNG", optimize=True)
print(f"Wrote {OUT} ({OUT.stat().st_size:,} bytes)")
