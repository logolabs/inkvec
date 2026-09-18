"""SVG rasterization.

Backed by resvg (via ``resvg-py``), deliberately: it is the same renderer family the
Rust core will use, so harness measurements and engine self-checks agree. Anti-aliasing
quality matters here — a renderer that snaps edges to pixels would hide exactly the
differences this benchmark exists to measure.
"""
from __future__ import annotations

import io
import re
import xml.etree.ElementTree as ET
from functools import lru_cache

import numpy as np
from PIL import Image

SVG_NS = "http://www.w3.org/2000/svg"


class RenderError(RuntimeError):
    pass


@lru_cache(maxsize=1)
def _backend():
    try:
        import resvg_py  # type: ignore
    except ImportError as e:  # pragma: no cover
        raise RenderError(
            "No SVG renderer available. Install with: pip install resvg-py\n"
            "(cairosvg is not used: it needs a native cairo DLL that is absent on "
            "typical Windows installs.)"
        ) from e
    return resvg_py


_LEN_RE = re.compile(r"^\s*([0-9.eE+-]+)\s*(px)?\s*$")


def _parse_len(value: str | None) -> float | None:
    if not value:
        return None
    m = _LEN_RE.match(value)
    return float(m.group(1)) if m else None


def normalize_svg(svg: str) -> tuple[str, tuple[float, float]]:
    """Ensure the SVG has a viewBox, so requesting a render size scales rather than pads.

    Tracer output frequently carries ``width``/``height`` but no ``viewBox`` (VTracer
    does this). Without a viewBox the meaning of a requested output size is
    renderer-dependent. We inject one and return the intrinsic user-unit size.
    """
    try:
        root = ET.fromstring(svg)
    except ET.ParseError as e:
        raise RenderError(f"unparseable SVG: {e}") from e

    vb = root.get("viewBox")
    if vb:
        parts = [float(p) for p in re.split(r"[,\s]+", vb.strip()) if p]
        if len(parts) == 4:
            return svg, (parts[2], parts[3])

    w = _parse_len(root.get("width"))
    h = _parse_len(root.get("height"))
    if w and h:
        root.set("viewBox", f"0 0 {w} {h}")
        return ET.tostring(root, encoding="unicode"), (w, h)

    raise RenderError("SVG has neither viewBox nor usable width/height")


def fit_viewbox(svg: str, width: int, height: int, margin: float = 0.0) -> str:
    """Expand the viewBox (centred) to the requested aspect so the backend renders exactly
    ``width x height`` at a uniform scale.

    Without this a 1984x400 logo asked for at 512x512 came back 512x104 and was then
    stretched 4.9x vertically with Lanczos: horizontal edges six pixels soft, ringing
    beside them, letters five times too tall. The tracer then found a phantom ink in the
    soft ramps and the lettering came out wrong (2026-09-05). A square viewBox at a
    square request is untouched, so the curated corpus is unchanged.
    """
    m0 = re.search(r'viewBox="([^"]+)"', svg)
    if not m0:
        return svg
    try:
        x, y, w, h = [float(v) for v in re.split(r"[ ,]+", m0.group(1).strip())]
    except ValueError:
        return svg
    if w <= 0 or h <= 0 or width <= 0 or height <= 0:
        return svg
    if margin > 0:
        # Breathing room on every side, as a fraction of the larger extent, so no
        # artwork is flush with the raster edge.
        m = margin * max(w, h)
        x, y, w, h = x - m, y - m, w + 2 * m, h + 2 * m
    want, have = width / height, w / h
    if abs(want - have) < 1e-6:
        return svg[: m0.start(1)] + f"{x:g} {y:g} {w:g} {h:g}" + svg[m0.end(1):] if margin > 0 else svg
    if want > have:
        nw = h * want
        x -= (nw - w) / 2
        w = nw
    else:
        nh = w / want
        y -= (nh - h) / 2
        h = nh
    return svg[: m0.start(1)] + f"{x:g} {y:g} {w:g} {h:g}" + svg[m0.end(1):]


def render(svg: str, width: int, height: int, margin: float = 0.0) -> np.ndarray:
    """Rasterize to an ``(H, W, 4)`` float32 RGBA array in [0, 1], straight (unpremultiplied)."""
    svg, _ = normalize_svg(svg)
    svg = fit_viewbox(svg, int(width), int(height), margin)
    out = _backend().svg_to_bytes(svg_string=svg, width=int(width), height=int(height))
    if isinstance(out, list):
        out = bytes(out)
    img = Image.open(io.BytesIO(out)).convert("RGBA")
    if img.size != (width, height):
        img = img.resize((width, height), Image.LANCZOS)
    return np.asarray(img, dtype=np.float32) / 255.0


def render_to_png(svg: str, width: int, height: int) -> bytes:
    svg, _ = normalize_svg(svg)
    svg = fit_viewbox(svg, int(width), int(height))
    out = _backend().svg_to_bytes(svg_string=svg, width=int(width), height=int(height))
    return bytes(out) if isinstance(out, list) else out


def load_rgba(path) -> np.ndarray:
    """Load a PNG as ``(H, W, 4)`` float32 RGBA in [0, 1]."""
    return np.asarray(Image.open(path).convert("RGBA"), dtype=np.float32) / 255.0


def save_rgba(arr: np.ndarray, path) -> None:
    Image.fromarray((np.clip(arr, 0, 1) * 255).astype(np.uint8), mode="RGBA").save(path)


def composite(rgba: np.ndarray, bg: tuple[float, float, float] = (1.0, 1.0, 1.0)) -> np.ndarray:
    """Composite straight RGBA over a solid background, returning ``(H, W, 3)`` float32."""
    a = rgba[..., 3:4]
    return rgba[..., :3] * a + np.asarray(bg, dtype=np.float32) * (1.0 - a)


def show_through(rgba: np.ndarray) -> np.ndarray:
    """Per-pixel background visibility in [0, 1] — i.e. ``1 - alpha``.

    Rendering the same SVG over two different backgrounds and differencing is the
    robust way to detect seams and gaps; this is the cheap equivalent when the
    renderer gives us a real alpha channel.
    """
    return 1.0 - rgba[..., 3]
