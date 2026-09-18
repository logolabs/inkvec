"""The vectorisers under test, behind one interface.

Each engine takes a PNG path and returns the SVG it produced and how long it took. An
engine that cannot run on this machine says so once and is skipped, rather than silently
scoring zero and flattering everything else.

Two kinds are missing on purpose. Vectorizer.AI charges per image and uploads the corpus,
so it stays behind an explicit opt-in and is never part of a default run. Vector Magic and
Adobe Image Trace have no automatable interface here; their numbers, and those of the
neural vectorisers whose weights are unreleased, belong in `published.py` as quoted figures
with a citation, not as measurements pretending to be ours.
"""
from __future__ import annotations

import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

import numpy as np

ROOT = Path(__file__).resolve().parents[2]


@dataclass
class Result:
    svg: str
    seconds: float
    ok: bool = True
    note: str = ""


Engine = Callable[[Path], Result]


# --------------------------------------------------------------------------------- inkvec
def _inkvec_exe() -> Path:
    return ROOT / "target/release/inkvec.exe"


def inkvec(extra: tuple[str, ...] = ()) -> Engine:
    exe = _inkvec_exe()

    def run(png: Path) -> Result:
        out = png.parent / "_inkvec_out.svg"
        t = time.perf_counter()
        r = subprocess.run([str(exe), str(png), "-o", str(out), "--quiet", *extra],
                           capture_output=True)
        sec = time.perf_counter() - t
        if r.returncode != 0 or not out.exists():
            return Result("", sec, False, (r.stderr or b"").decode(errors="replace")[:200])
        return Result(out.read_text(encoding="utf-8"), sec)

    return run


# -------------------------------------------------------------------------------- vtracer
# The configuration the project has used for every VTracer comparison to date. Kept here so
# that a change to it is a visible change to the benchmark rather than to a call site.
VTRACER_CFG = dict(
    colormode="color", mode="spline", hierarchical="stacked", path_precision=3,
    corner_threshold=60, length_threshold=4.0, splice_threshold=45,
    color_precision=8, layer_difference=8, filter_speckle=2,
)


def vtracer() -> Engine:
    import vtracer as vt

    def run(png: Path) -> Result:
        t = time.perf_counter()
        try:
            svg = vt.convert_raw_image_to_svg(png.read_bytes(), img_format="png",
                                              **VTRACER_CFG)
        except Exception as e:  # noqa: BLE001
            return Result("", time.perf_counter() - t, False, str(e)[:200])
        return Result(svg, time.perf_counter() - t)

    return run


# -------------------------------------------------------------------------------- potrace
def potrace(threshold: float = 0.5) -> Engine:
    """The classical baseline. Bilevel by construction, so it is scored only where that is
    a fair thing to ask -- it cannot represent a multi-colour icon at all, and a table that
    let it compete on one would be measuring the wrong thing."""
    import potrace as pt
    from PIL import Image

    def run(png: Path) -> Result:
        t = time.perf_counter()
        try:
            im = Image.open(png).convert("RGBA")
            bg = Image.new("RGBA", im.size, (255, 255, 255, 255))
            g = np.asarray(Image.alpha_composite(bg, im).convert("L"), np.uint8)
            # Pass the grey levels, not a boolean mask. `Bitmap.__init__` thresholds and
            # then calls `invert()`, so handing it "dark is true" traces the background:
            # the first run of this suite scored potrace at dE00 98.6 with a negative SSIM,
            # which is a filled canvas rather than a bad tracer.
            bmp = pt.Bitmap(g, blacklevel=threshold)
            path = bmp.trace()
            w, h = im.size
            d: list[str] = []
            for curve in path:
                s = curve.start_point
                d.append(f"M{s.x:.3f},{s.y:.3f}")
                for seg in curve:
                    if seg.is_corner:
                        d.append(f"L{seg.c.x:.3f},{seg.c.y:.3f}"
                                 f"L{seg.end_point.x:.3f},{seg.end_point.y:.3f}")
                    else:
                        d.append(f"C{seg.c1.x:.3f},{seg.c1.y:.3f}"
                                 f" {seg.c2.x:.3f},{seg.c2.y:.3f}"
                                 f" {seg.end_point.x:.3f},{seg.end_point.y:.3f}")
                d.append("Z")
            svg = (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}"'
                   f' width="{w}" height="{h}">'
                   f'<rect width="{w}" height="{h}" fill="#ffffff"/>'
                   f'<path d="{"".join(d)}" fill="#000000" fill-rule="evenodd"/></svg>')
        except Exception as e:  # noqa: BLE001
            return Result("", time.perf_counter() - t, False, str(e)[:200])
        return Result(svg, time.perf_counter() - t)

    return run


# ---------------------------------------------------------------------------- the registry
def available(names: list[str] | None = None) -> dict[str, Engine]:
    """Build the engines that can actually run here, reporting the ones that cannot."""
    out: dict[str, Engine] = {}
    want = names or ["inkvec", "vtracer", "potrace"]
    for name in want:
        try:
            if name == "inkvec":
                if not _inkvec_exe().exists():
                    raise FileNotFoundError("target/release/inkvec.exe not built")
                out[name] = inkvec()
            elif name == "vtracer":
                out[name] = vtracer()
            elif name == "potrace":
                out[name] = potrace()
            else:
                raise KeyError(f"unknown engine {name}")
        except Exception as e:  # noqa: BLE001
            print(f"  [engines] {name} unavailable: {e}")
    return out


#: Engines that only make sense on two-colour art.
BILEVEL_ONLY = {"potrace"}
