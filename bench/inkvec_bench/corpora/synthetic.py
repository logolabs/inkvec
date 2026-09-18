"""Procedurally generated ground-truth corpus.

Emoji sets give realism; this gives *control*. Every file here is generated from a
known structural intent, so we can score things a rendered emoji cannot tell us:

* **Primitive recall** — this really was a ``<circle>``. Did the tracer return one?
* **Symmetry** — this really was 5-fold rotational. Was that recovered, or did five
  copies come back with five slightly different outlines?
* **Seam vs. overdraw** — the mosaic categories tile the plane with regions that
  *exactly* share boundary vertices. A planar-map tracer should reproduce that at
  overdraw ~ 1.0 with no seams. A path-list tracer must pick one failure or the other,
  which is precisely the trade this project claims to escape.
* **Sub-pixel features** — strokes deliberately thinner than one pixel at the small
  raster tiers, where thresholding tracers lose the feature entirely.

Everything is deterministic given a seed, and generation needs no network.
"""
from __future__ import annotations

import json
import math
from dataclasses import dataclass, asdict
from pathlib import Path

import numpy as np

CANVAS = 256  # user units; raster tiers scale this


@dataclass
class SyntheticSpec:
    name: str
    category: str
    n_primitives: int
    has_gradient: bool
    symmetry: str  # "none" | "mirror" | "rot{k}"
    notes: str = ""


def _svg(body: str) -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" '
        f'viewBox="0 0 {CANVAS} {CANVAS}" width="{CANVAS}" height="{CANVAS}">'
        f'<rect width="{CANVAS}" height="{CANVAS}" fill="#ffffff"/>{body}</svg>'
    )


def _hsl(i: int, n: int, light: int = 55) -> str:
    return f"hsl({int(360 * i / max(1, n))}, 62%, {light}%)"


# --- single primitives -------------------------------------------------------------


def gen_primitives(rng) -> list[tuple[str, str, SyntheticSpec]]:
    out = []
    c = CANVAS / 2
    out.append((
        "prim_circle",
        _svg(f'<circle cx="{c}" cy="{c}" r="{c * 0.72:.2f}" fill="#d94f3d"/>'),
        SyntheticSpec("prim_circle", "primitive", 1, False, "rot_inf", "exact circle"),
    ))
    out.append((
        "prim_ellipse",
        _svg(f'<ellipse cx="{c}" cy="{c}" rx="{c * 0.78:.2f}" ry="{c * 0.45:.2f}" fill="#2f6f9f"/>'),
        SyntheticSpec("prim_ellipse", "primitive", 1, False, "mirror", "exact ellipse"),
    ))
    out.append((
        "prim_roundrect",
        _svg(f'<rect x="28" y="52" width="200" height="152" rx="34" ry="34" fill="#3f9f6f"/>'),
        SyntheticSpec("prim_roundrect", "primitive", 1, False, "mirror", "rounded rect, r=34"),
    ))
    out.append((
        "prim_rect_rot",
        _svg(f'<rect x="60" y="80" width="136" height="96" fill="#8f5fbf" '
             f'transform="rotate(23 {c} {c})"/>'),
        SyntheticSpec("prim_rect_rot", "primitive", 1, False, "mirror", "rotated rect, 23deg"),
    ))
    # Regular star and polygon, emitted as <polygon> so the ground truth is a primitive.
    for k, name in ((5, "prim_star5"), (6, "prim_hex")):
        pts = []
        for i in range(k * 2 if "star" in name else k):
            if "star" in name:
                r = c * (0.82 if i % 2 == 0 else 0.36)
                a = -math.pi / 2 + i * math.pi / k
            else:
                r = c * 0.78
                a = -math.pi / 2 + i * 2 * math.pi / k
            pts.append(f"{c + r * math.cos(a):.2f},{c + r * math.sin(a):.2f}")
        out.append((
            name,
            _svg(f'<polygon points="{" ".join(pts)}" fill="#e0a020"/>'),
            SyntheticSpec(name, "primitive", 1, False, f"rot{k}", f"{k}-fold regular"),
        ))
    return out


# --- mosaics: regions sharing exact boundaries -------------------------------------


def gen_mosaics(rng) -> list[tuple[str, str, SyntheticSpec]]:
    """Partitions of the plane. Adjacent regions share boundary vertices exactly.

    This is the seam/overdraw probe. The ground truth has overdraw ratio 1.0 (no
    region is drawn twice) and zero seams. A tracer that stores each region as an
    independent path has to give up one or the other.
    """
    out = []
    c = CANVAS / 2

    # Pie: k sectors sharing radial edges.
    for k in (6, 12):
        parts = []
        R = c * 0.88
        for i in range(k):
            a0 = 2 * math.pi * i / k - math.pi / 2
            a1 = 2 * math.pi * (i + 1) / k - math.pi / 2
            x0, y0 = c + R * math.cos(a0), c + R * math.sin(a0)
            x1, y1 = c + R * math.cos(a1), c + R * math.sin(a1)
            large = 0
            parts.append(
                f'<path d="M{c:.3f},{c:.3f} L{x0:.3f},{y0:.3f} '
                f'A{R:.3f},{R:.3f} 0 {large} 1 {x1:.3f},{y1:.3f} Z" fill="{_hsl(i, k)}"/>'
            )
        out.append((
            f"mosaic_pie{k}",
            _svg("".join(parts)),
            SyntheticSpec(f"mosaic_pie{k}", "mosaic", k, False, f"rot{k}",
                          f"{k} sectors sharing radial boundaries exactly"),
        ))

    # Jittered grid: cells share vertices exactly.
    for n, tag in ((4, "grid4"), (6, "grid6")):
        pts = np.zeros((n + 1, n + 1, 2))
        for i in range(n + 1):
            for j in range(n + 1):
                x = CANVAS * i / n
                y = CANVAS * j / n
                if 0 < i < n:
                    x += rng.uniform(-1, 1) * CANVAS / n * 0.22
                if 0 < j < n:
                    y += rng.uniform(-1, 1) * CANVAS / n * 0.22
                pts[i, j] = (x, y)
        parts = []
        idx = 0
        for i in range(n):
            for j in range(n):
                quad = [pts[i, j], pts[i + 1, j], pts[i + 1, j + 1], pts[i, j + 1]]
                d = "M" + " L".join(f"{p[0]:.3f},{p[1]:.3f}" for p in quad) + " Z"
                parts.append(f'<path d="{d}" fill="{_hsl(idx, n * n)}"/>')
                idx += 1
        out.append((
            f"mosaic_{tag}",
            _svg("".join(parts)),
            SyntheticSpec(f"mosaic_{tag}", "mosaic", n * n, False, "none",
                          f"{n}x{n} jittered grid, shared vertices"),
        ))
    return out


# --- gradients ---------------------------------------------------------------------


def gen_gradients(rng) -> list[tuple[str, str, SyntheticSpec]]:
    c = CANVAS / 2
    out = []
    lin = (
        '<defs><linearGradient id="lg" x1="0" y1="0" x2="1" y2="1">'
        '<stop offset="0" stop-color="#2b6cb0"/><stop offset="1" stop-color="#f6ad55"/>'
        "</linearGradient></defs>"
        f'<rect x="24" y="24" width="208" height="208" rx="18" fill="url(#lg)"/>'
    )
    out.append((
        "gradient_linear",
        _svg(lin),
        SyntheticSpec("gradient_linear", "gradient", 1, True, "none",
                      "single linear gradient; banding tracers explode here"),
    ))
    rad = (
        '<defs><radialGradient id="rg" cx="0.4" cy="0.35" r="0.7">'
        '<stop offset="0" stop-color="#fffbe6"/><stop offset="1" stop-color="#b7791f"/>'
        "</radialGradient></defs>"
        f'<circle cx="{c}" cy="{c}" r="{c * 0.8:.2f}" fill="url(#rg)"/>'
    )
    out.append((
        "gradient_radial",
        _svg(rad),
        SyntheticSpec("gradient_radial", "gradient", 1, True, "none",
                      "single radial gradient on a primitive circle"),
    ))
    return out


# --- symmetry ----------------------------------------------------------------------


def gen_symmetry(rng) -> list[tuple[str, str, SyntheticSpec]]:
    """Compositions with exact symmetry, emitted via <use> so the ground truth reuses geometry."""
    out = []
    c = CANVAS / 2
    for k in (3, 5, 8):
        petal = (
            f'<path id="p" d="M{c:.2f},{c:.2f} '
            f'C{c - 26:.2f},{c - 58:.2f} {c - 20:.2f},{c - 100:.2f} {c:.2f},{c - 108:.2f} '
            f'C{c + 20:.2f},{c - 100:.2f} {c + 26:.2f},{c - 58:.2f} {c:.2f},{c:.2f} Z" '
            f'fill="#c53030"/>'
        )
        uses = "".join(
            f'<use xlink:href="#p" transform="rotate({360 * i / k:.3f} {c} {c})"/>'
            for i in range(1, k)
        )
        out.append((
            f"symmetry_rot{k}",
            _svg(f"<defs>{petal}</defs><use xlink:href=\"#p\"/>{uses}"),
            SyntheticSpec(f"symmetry_rot{k}", "symmetry", k, False, f"rot{k}",
                          f"{k}-fold rotational, one path reused via <use>"),
        ))

    mirror = (
        f'<defs><path id="h" d="M{c:.2f},210 L{c - 92:.2f},150 '
        f'C{c - 100:.2f},96 {c - 56:.2f},60 {c:.2f},96 Z" fill="#2c7a7b"/></defs>'
        f'<use xlink:href="#h"/>'
        f'<use xlink:href="#h" transform="translate({CANVAS},0) scale(-1,1)"/>'
    )
    out.append((
        "symmetry_mirror",
        _svg(mirror),
        SyntheticSpec("symmetry_mirror", "symmetry", 2, False, "mirror",
                      "exact mirror pair via <use> + scale(-1,1)"),
    ))
    return out


# --- hard cases --------------------------------------------------------------------


def gen_hard(rng) -> list[tuple[str, str, SyntheticSpec]]:
    out = []
    c = CANVAS / 2

    # Sub-pixel features: at the 32px and 64px tiers these strokes are thinner than a
    # pixel. Thresholding tracers drop them; coverage-aware ones should keep them.
    lines = "".join(
        f'<rect x="{20 + i * 22}" y="30" width="{0.6 + i * 0.55:.2f}" height="196" fill="#1a202c"/>'
        for i in range(10)
    )
    out.append((
        "thin_features",
        _svg(lines),
        SyntheticSpec("thin_features", "hard", 10, False, "none",
                      "strokes 0.6..5.6 user units wide; sub-pixel at small tiers"),
    ))

    # Concentric rings: many near-parallel boundaries at close spacing.
    rings = "".join(
        f'<circle cx="{c}" cy="{c}" r="{112 - i * 11:.1f}" fill="{_hsl(i, 10, 50 + (i % 2) * 22)}"/>'
        for i in range(10)
    )
    out.append((
        "rings_concentric",
        _svg(rings),
        SyntheticSpec("rings_concentric", "hard", 10, False, "rot_inf",
                      "10 stacked circles; tests boundary spacing and z-order"),
    ))

    # Overlapping stack with known occlusion, for layer-recovery scoring.
    stack = (
        f'<circle cx="96" cy="110" r="66" fill="#e53e3e"/>'
        f'<circle cx="160" cy="110" r="66" fill="#3182ce" fill-opacity="0.85"/>'
        f'<circle cx="128" cy="166" r="66" fill="#38a169" fill-opacity="0.85"/>'
    )
    out.append((
        "stack_overlap",
        _svg(stack),
        SyntheticSpec("stack_overlap", "hard", 3, False, "none",
                      "3 overlapping semi-transparent circles; occlusion + alpha"),
    ))

    # A logo-like composition: primitives, a gradient, symmetry and a counter-form.
    logo = (
        '<defs><linearGradient id="lg2" x1="0" y1="0" x2="0" y2="1">'
        '<stop offset="0" stop-color="#4c51bf"/><stop offset="1" stop-color="#2b6cb0"/>'
        "</linearGradient></defs>"
        f'<rect x="24" y="24" width="208" height="208" rx="46" fill="url(#lg2)"/>'
        f'<circle cx="{c}" cy="{c}" r="62" fill="#ffffff"/>'
        f'<circle cx="{c}" cy="{c}" r="30" fill="#4c51bf"/>'
        f'<rect x="120" y="36" width="16" height="40" rx="8" fill="#ffffff"/>'
    )
    out.append((
        "logo_like",
        _svg(logo),
        SyntheticSpec("logo_like", "logo", 4, True, "mirror",
                      "gradient panel, concentric counters, rounded corners"),
    ))
    return out


GENERATORS = (gen_primitives, gen_mosaics, gen_gradients, gen_symmetry, gen_hard)


def build(out_dir: Path, seed: int = 0) -> list[SyntheticSpec]:
    """Write the synthetic corpus and its manifest. Returns the specs written."""
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(seed)

    specs: list[SyntheticSpec] = []
    for gen in GENERATORS:
        for name, svg, spec in gen(rng):
            (out_dir / f"{name}.svg").write_text(svg, encoding="utf-8")
            specs.append(spec)

    (out_dir / "manifest.json").write_text(
        json.dumps([asdict(s) for s in specs], indent=2), encoding="utf-8"
    )
    return specs
