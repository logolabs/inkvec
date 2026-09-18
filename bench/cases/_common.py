"""Machinery shared by the isolated cases: rasterise, trace, and measure.

A case here is deliberately not an icon. The 980-icon run answers "is it better on
average"; it cannot answer "is the edge in the right place", because every icon mixes a
dozen properties and the score sums them into one number. So each case holds exactly one
property still and measures it against geometry we authored ourselves — never against
what the tracer happens to do today. A case that fails is therefore a statement about the
tracer, not about the threshold.

Two conventions matter and are easy to get wrong:

* **Frames.** inkvec emits ``viewBox="-0.5 -0.5 W H"``: its user units are pixel *centres*,
  so its coordinate 19.9 is the same place as 20.4 in a truth SVG written on the pixel
  grid. svgelements applies the viewBox-to-viewport transform when it parses, which
  cancels the offset exactly, so geometry read through `shapes()` is in one common frame
  (image pixels, top-left of the image at the origin) for both documents. Do not "fix"
  the half pixel by hand.
* **Ink, not coverage.** Measuring a thin stroke by its alpha area is blind to the disease
  that actually eats thin strokes: the stroke survives geometrically but comes back as a
  pale blend of itself and the backdrop. So sizes are measured twice — once over all
  covered pixels, once over pixels that still carry the artist's colour. The pair names
  the disease; coverage alone does not.
"""
from __future__ import annotations

import io
import math
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))

import svgelements as se  # noqa: E402
from inkvec_bench import render  # noqa: E402
from inkvec_bench.metrics.color import delta_e00  # noqa: E402
from inkvec_bench.svgmodel import _flatten_segments, _walk  # noqa: E402

# The mirror residual already exists and is what regularity.py reports over the real
# corpus; a second copy would drift away from it.
from regularity import symmetry  # noqa: E402,F401  (re-exported to the cases)

# Same intake as the corpus (bench/build_corpus_v2.py): 8x8 samples give 64 coverage
# levels from geometry alone, where resvg's own AA quantises an edge to four. A case
# rendered any other way would be measuring the renderer, not the tracer.
SUPERSAMPLE = 8

# inkvec's default --precision is 0.1 px, so every emitted coordinate lands on a 0.1 grid
# and a perfect fit still shows ~0.025 px mean, 0.05 px worst-case placement error. Every
# geometric threshold in this suite is stated relative to that floor, and every case
# authors its geometry *off* that grid so nothing can pass by landing on it.

# Straight-alpha colour match tolerance, per channel, in [0, 1]. 0.06 is ~15/255: wider
# than 8-bit rounding and gradient dither, far narrower than any blend a tracer makes
# when it averages a stroke with its backdrop.
INK_TOL = 0.06


# --------------------------------------------------------------------------- model


@dataclass
class Check:
    """One assertion. `note` says where the number comes from, not what it does."""

    name: str
    value: float
    limit: float
    op: str  # "<" | "<=" | "==" | ">=" | ">"
    note: str

    @property
    def ok(self) -> bool:
        v, l = self.value, self.limit
        if not np.isfinite(v):
            return False
        return {"<": v < l, "<=": v <= l, "==": v == l, ">=": v >= l, ">": v > l}[self.op]

    @property
    def margin(self) -> float:
        """How close to the limit, as a ratio. Used to pick the line worth printing."""
        if self.op in ("<", "<="):
            return self.value / self.limit if self.limit else float("inf")
        if self.op in (">=", ">"):
            return self.limit / self.value if self.value else float("inf")
        return 0.0 if self.ok else float("inf")


@dataclass
class Case:
    """One property, one small input, one verdict."""

    name: str
    what: str  # the property, in a few words
    check: Callable[["Traced"], list[Check]]
    svg: str | None = None  # authored ground truth
    size: int = 128
    png_path: Path | None = None  # real-icon cases bring their own raster ...
    truth_path: Path | None = None  # ... and the artist's SVG next to it
    args: tuple[str, ...] = ()  # extra tracer flags, for a case about a flag


# ----------------------------------------------------------------------- rasterise


def rasterize(svg: str, size: int, ss: int = SUPERSAMPLE) -> bytes:
    """Exact-coverage raster, byte-identical in method to the corpus intake.

    Copied rather than imported: build_corpus_v2 pulls in `requests` and the corpus
    fetchers at import time, and a case suite that cannot run offline is not a case suite.
    """
    rgba = render.render(svg, size * ss, size * ss)  # straight RGBA float32 in [0, 1]
    a = rgba[..., 3:4]
    pre = np.concatenate([rgba[..., :3] * a, a], axis=-1)
    blk = pre.reshape(size, ss, size, ss, 4).mean(axis=(1, 3))
    alpha = blk[..., 3:4]
    rgb = np.where(alpha > 1e-6, blk[..., :3] / np.maximum(alpha, 1e-6), 0.0)
    out = np.concatenate([rgb, alpha], axis=-1)
    img = Image.fromarray((np.clip(out, 0, 1) * 255 + 0.5).astype(np.uint8), "RGBA")
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


def svg_doc(size: int, body: str) -> str:
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" '
            f'viewBox="0 0 {size} {size}">{body}</svg>')


# ------------------------------------------------------------------------- tracing


@dataclass
class Shape:
    kind: str
    fill: tuple[float, float, float] | None  # None for a gradient or no paint
    gradient: bool
    rings: list[np.ndarray]
    seg_types: dict[str, int]

    @property
    def n_segments(self) -> int:
        return sum(n for k, n in self.seg_types.items() if k != "Close")


def shapes(svg: str) -> list[Shape]:
    """Drawable shapes with fill and flattened rings, in image-pixel coordinates."""
    try:
        doc = se.SVG.parse(io.StringIO(svg))
    except Exception:
        return []
    out: list[Shape] = []
    for shape, _depth, _gid in _walk(doc, 0, None):
        kind = type(shape).__name__
        if kind in ("Text", "Image", "Desc", "Title", "Use"):
            continue
        try:
            segs = list(se.Path(shape).segments())
        except Exception:
            continue
        rings, length, seg_types, _anchors = _flatten_segments(segs)
        if length <= 0 and not rings:
            continue
        raw = str((getattr(shape, "values", {}) or {}).get("fill", "")).strip().lower()
        grad = raw.startswith("url(")
        rgb = None
        if not grad:
            try:
                c = se.Color(shape.fill) if shape.fill is not None else None
                if c is not None and c.value is not None:
                    rgb = (c.red / 255.0, c.green / 255.0, c.blue / 255.0)
            except Exception:
                rgb = None
        out.append(Shape(kind, rgb, grad, rings, seg_types))
    return out


class Traced:
    """One case, rasterised and traced, with every measurement cached."""

    def __init__(self, case: Case, exe: Path, work: Path):
        self.case = case
        self.size = case.size
        self._cache: dict = {}

        if case.png_path is not None:
            self.png = case.png_path
            self.truth_svg = case.truth_path.read_text(encoding="utf-8")
            self.size = render.load_rgba(self.png).shape[0]
        else:
            self.truth_svg = case.svg
            self.png = work / f"{case.name}.png"
            self.png.write_bytes(rasterize(self.truth_svg, case.size))

        out = work / f"{case.name}.svg"
        cmd = [str(exe), str(self.png), "-o", str(out), "--quiet", *case.args]
        proc = subprocess.run(cmd, capture_output=True)
        self.rc = proc.returncode
        self.stderr = proc.stderr.decode("utf-8", "replace")[-400:]
        self.out_svg = out.read_text(encoding="utf-8") if out.exists() else ""

    # --- renders ---------------------------------------------------------------
    def _render(self, key: str, svg: str, scale: int) -> np.ndarray:
        ck = (key, scale)
        if ck not in self._cache:
            n = self.size * scale
            self._cache[ck] = render.render(svg, n, n)
        return self._cache[ck]

    def truth_rgba(self, scale: int = 4) -> np.ndarray:
        return self._render("t", self.truth_svg, scale)

    def out_rgba(self, scale: int = 4) -> np.ndarray:
        return self._render("o", self.out_svg, scale)

    # --- geometry --------------------------------------------------------------
    @property
    def truth_shapes(self) -> list[Shape]:
        if "ts" not in self._cache:
            self._cache["ts"] = shapes(self.truth_svg)
        return self._cache["ts"]

    @property
    def out_shapes(self) -> list[Shape]:
        if "os" not in self._cache:
            self._cache["os"] = shapes(self.out_svg)
        return self._cache["os"]

    def out_rings(self) -> list[np.ndarray]:
        return [r for s in self.out_shapes for r in s.rings]

    def out_points(self) -> np.ndarray:
        rings = self.out_rings()
        return np.vstack(rings) if rings else np.zeros((0, 2))

    @property
    def out_segments(self) -> int:
        return sum(s.n_segments for s in self.out_shapes)

    @property
    def truth_segments(self) -> int:
        return sum(s.n_segments for s in self.truth_shapes)


# -------------------------------------------------------------------- measurements


def seg_distance(pts: np.ndarray, a: np.ndarray, b: np.ndarray) -> np.ndarray:
    ab = b - a
    l2 = float(ab @ ab)
    if l2 <= 0:
        return np.hypot(*(pts - a).T)
    t = np.clip((pts - a) @ ab / l2, 0.0, 1.0)
    return np.hypot(*(pts - (a + t[:, None] * ab)).T)


def dist_to_polygon(pts: np.ndarray, poly, closed: bool = True) -> np.ndarray:
    """Distance from each point to the nearest edge of a polyline / closed polygon."""
    P = np.asarray(poly, dtype=float)
    if len(pts) == 0 or len(P) < 2:
        return np.full(len(pts), np.inf)
    d = np.full(len(pts), np.inf)
    n = len(P) if closed else len(P) - 1
    for i in range(n):
        d = np.minimum(d, seg_distance(pts, P[i], P[(i + 1) % len(P)]))
    return d


def dist_to_output(t: Traced, p) -> float:
    """Distance from one point to the nearest emitted boundary.

    This is how corner sharpness and junction placement are read: a boundary that
    chamfers a corner leaves the true corner stranded that far outside it, while a sharp
    fit passes through the point and reads ~0.
    """
    q = np.asarray([p], dtype=float)
    best = np.inf
    for ring in t.out_rings():
        best = min(best, float(dist_to_polygon(q, ring)[0]))
    return best


def edge_displacement(t: Traced, poly, corner_radius: float = 2.0) -> float:
    """Mean distance from the emitted boundary to the truth's edges, corners excluded.

    Corners are excluded because a chamfered corner is a different defect with its own
    case; leaving it in would let a rounded corner masquerade as an edge that drifted.
    """
    pts = t.out_points()
    P = np.asarray(poly, dtype=float)
    if len(pts) == 0 or len(P) < 3:
        return float("nan")
    to_vertex = np.linalg.norm(pts[:, None, :] - P[None, :, :], axis=2).min(axis=1)
    pts = pts[to_vertex > corner_radius]
    if len(pts) == 0:
        return float("nan")
    return float(dist_to_polygon(pts, P).mean())


def turning(ring: np.ndarray) -> float:
    """Total absolute turning of a closed polyline, in radians.

    A convex quad reads 2*pi however many points it is sampled at, so this is a scale-
    and sampling-free count of how many times the boundary changes its mind. Teeth in a
    traced stroke show up here as a multiple of the truth, and nowhere else.
    """
    P = np.asarray(ring, dtype=float)
    if len(P) < 3:
        return 0.0
    d = np.diff(np.vstack([P, P[:1]]), axis=0)
    n = np.hypot(d[:, 0], d[:, 1])
    keep = n > 1e-9
    d, n = d[keep], n[keep]
    if len(d) < 3:
        return 0.0
    u = d / n[:, None]
    v = np.roll(u, -1, axis=0)
    cross = u[:, 0] * v[:, 1] - u[:, 1] * v[:, 0]
    dot = (u * v).sum(1)
    return float(np.abs(np.arctan2(cross, np.clip(dot, -1.0, 1.0))).sum())


def total_turning(rings) -> float:
    return float(sum(turning(r) for r in rings))


def ink_area(rgba: np.ndarray, rgb, scale: int, tol: float = INK_TOL) -> float:
    """Coverage, in source px^2, that still carries `rgb` in straight alpha."""
    a = rgba[..., 3]
    near = np.abs(rgba[..., :3] - np.asarray(rgb, dtype=np.float32)).max(-1) <= tol
    return float(a[near & (a > 0.02)].sum()) / (scale * scale)


def alpha_area(rgba: np.ndarray, scale: int) -> float:
    return float(rgba[..., 3].sum()) / (scale * scale)


def components(rgba: np.ndarray, thresh: float = 0.5) -> int:
    from scipy import ndimage

    return int(ndimage.label(rgba[..., 3] >= thresh)[1])


def holes(rgba: np.ndarray, thresh: float = 0.5) -> int:
    """Enclosed background regions — the counters of a glyph."""
    from scipy import ndimage

    bg = rgba[..., 3] < thresh
    lab, n = ndimage.label(bg)
    if n == 0:
        return 0
    border = set(lab[0, :]) | set(lab[-1, :]) | set(lab[:, 0]) | set(lab[:, -1])
    return sum(1 for i in range(1, n + 1) if i not in border)


def painted_over_clear(truth: np.ndarray, out: np.ndarray, scale: int) -> float:
    """Area, in source px^2, painted opaque where the artist left the canvas clear.

    Separates the two ways a counter disappears. Filled in with the surrounding ink, it
    shows up as a lost hole and nothing here; painted over with white, it shows up in
    both — and only the second survives being placed on a coloured ground.
    """
    return float(((truth[..., 3] < 0.1) & (out[..., 3] > 0.9)).sum()) / (scale * scale)


def de00(t: Traced, scale: int = 4, alpha_min: float = 0.5) -> float:
    """Mean CIEDE2000 over the truth's opaque area, composited on white."""
    a, b = t.truth_rgba(scale), t.out_rgba(scale)
    mask = a[..., 3] >= alpha_min
    if not mask.any():
        return float("nan")
    return delta_e00(render.composite(a), render.composite(b), mask)["de00_mean"]


def ribbon_polygon(x0, y0, x1, y1, w) -> list[tuple[float, float]]:
    """The four corners of a stroke, written as a filled polygon.

    Authored as a polygon rather than a `<line stroke-width=...>` so the truth's segment
    count and turning are exactly 4 and 2*pi — the numbers the emitted boundary is
    compared against.
    """
    dx, dy = x1 - x0, y1 - y0
    n = math.hypot(dx, dy)
    px, py = -dy / n * w / 2, dx / n * w / 2
    return [(x0 + px, y0 + py), (x1 + px, y1 + py), (x1 - px, y1 - py), (x0 - px, y0 - py)]


def poly_attr(pts) -> str:
    return " ".join(f"{x:.4f},{y:.4f}" for x, y in pts)
