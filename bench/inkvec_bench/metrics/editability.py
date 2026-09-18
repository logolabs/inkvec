"""Editability metrics.

No published vectorization benchmark scores these. Every one of them scores pixels,
which means a tracer that emits ten thousand anchors in a flat list ties with one that
emits a clean, grouped, primitive-aware document. Designers do not experience those as
equivalent, and neither should a benchmark.

The pair that matters most is (seam, overdraw). In a representation that stores each
region as an independent closed path, these trade against each other and cannot both
be driven to zero:

  * lay regions edge-to-edge (a cutout mosaic) and rounding disagreement between two
    copies of the same boundary opens hairline **seams**;
  * stack regions with generous overlap and the seams close, but geometry is drawn
    two or more times — **overdraw** — so every shared boundary now exists twice and
    editing it means editing both copies.

A planar map with genuinely shared edges escapes the trade entirely: seam ~ 0 at
overdraw ~ 1. That escape is the central architectural claim of this project, so the
benchmark measures the pair rather than either alone.
"""
from __future__ import annotations

import re

import numpy as np

from ..config import OPAQUE_ALPHA, TRANSPARENT_ALPHA
from ..svgmodel import DocInfo

try:
    from shapely.geometry import Polygon
    from shapely.ops import unary_union
    from shapely.validation import explain_validity

    _HAVE_SHAPELY = True
except ImportError:  # pragma: no cover
    _HAVE_SHAPELY = False

# Guard against pathological documents; geometric union is superlinear in ring count.
MAX_RINGS_FOR_GEOMETRY = 4000


def seam_metrics(ref_rgba: np.ndarray, cand_rgba: np.ndarray) -> dict[str, float]:
    """Fraction of the reference's opaque area where the candidate lets background through.

    This is the rendering-side signature of a hairline gap between two regions that
    should have shared a boundary. It is close to invisible at source resolution and
    grows at higher zoom, which is why the harness evaluates it at several scales.
    """
    ref_op = ref_rgba[..., 3] >= OPAQUE_ALPHA
    cand_gap = cand_rgba[..., 3] <= TRANSPARENT_ALPHA
    denom = int(ref_op.sum())
    if denom == 0:
        return {"seam_fraction": 0.0, "seam_pixels": 0.0}
    seam = int((ref_op & cand_gap).sum())
    return {"seam_fraction": seam / denom, "seam_pixels": float(seam)}


def coverage_metrics(doc: DocInfo) -> dict[str, float]:
    """Geometric overdraw and validity, computed from flattened rings."""
    out = {
        "overdraw_ratio": float("nan"),
        "self_intersections": float("nan"),
        "invalid_ring_fraction": float("nan"),
    }
    if not _HAVE_SHAPELY:
        return out

    rings = [r for e in doc.elements for r in e.rings if len(r) >= 3]
    if not rings:
        return out
    if len(rings) > MAX_RINGS_FOR_GEOMETRY:
        # Still report validity, which is linear; skip the union.
        bad = _count_invalid(rings)
        out["self_intersections"] = float(bad)
        out["invalid_ring_fraction"] = bad / len(rings)
        return out

    # Net filled area must respect the fill rule. A face with a hole is drawn as two
    # rings under `evenodd`, and summing |ring area| counts the hole as *more* ink
    # instead of less — which reports a correct partition as heavy overdraw. Even-odd
    # fill is exactly the symmetric difference of the rings, so compute that.
    polys = []
    invalid = 0
    per_element: list[list] = []
    for e in doc.elements:
        elem_polys = []
        for r in e.rings:
            if len(r) < 3:
                continue
            try:
                p = Polygon(r)
            except Exception:
                invalid += 1
                continue
            if not p.is_valid:
                invalid += 1
                p = p.buffer(0)
            if p.is_empty:
                continue
            elem_polys.append(p)
            polys.append(p)
        if elem_polys:
            per_element.append(elem_polys)

    if not polys:
        return out

    sum_area = 0.0
    for elem_polys in per_element:
        net = elem_polys[0]
        for q in elem_polys[1:]:
            try:
                net = net.symmetric_difference(q)
            except Exception:
                net = net.union(q)
        sum_area += float(abs(net.area))
    try:
        union_area = float(abs(unary_union(polys).area))
    except Exception:
        union_area = float("nan")

    out["overdraw_ratio"] = sum_area / union_area if union_area > 0 else float("nan")
    out["self_intersections"] = float(invalid)
    out["invalid_ring_fraction"] = invalid / len(rings)
    return out


def _count_invalid(rings) -> int:
    bad = 0
    for r in rings:
        try:
            if not Polygon(r).is_valid:
                bad += 1
        except Exception:
            bad += 1
    return bad


def structure_metrics(doc: DocInfo) -> dict[str, float]:
    """Document-level structure: anchor economy, primitive use, layer tree."""
    return {
        "n_elements": float(doc.n_elements),
        "n_anchors": float(doc.n_anchors),
        "n_params": float(doc.n_params),
        "boundary_length": float(doc.total_length),
        "anchor_density": float(doc.anchor_density),
        "primitive_fraction": float(doc.primitive_fraction),
        "gradient_fraction": float(doc.gradient_fraction),
        "n_groups": float(doc.n_groups),
        "named_group_fraction": float(doc.named_group_fraction),
        "max_group_depth": float(doc.max_depth),
    }


# --- edit locality ------------------------------------------------------------------

_NUM_RE = re.compile(r"-?\d*\.?\d+(?:[eE][-+]?\d+)?")


def edit_locality(
    svg: str,
    width: int,
    height: int,
    n_probes: int = 8,
    delta: float = 1.0,
    seed: int = 0,
) -> dict[str, float]:
    """Perturb single anchors and measure how far the consequences travel.

    Two things are being asked:

    ``edit_spread``  — after nudging one anchor, what fraction of the rendered image
                       changed? A local edit should stay local. A large number means
                       the representation couples distant geometry.

    ``edit_tear``    — did the nudge open a hole? In a mosaic of independent paths,
                       moving a boundary anchor tears a gap because only one side of
                       the shared boundary moved. In a planar map both faces move
                       together and nothing tears. This is the metric that most
                       directly distinguishes the two representations.

    Experimental, and expensive: it re-renders once per probe.
    """
    from ..render import render

    rng = np.random.default_rng(seed)
    try:
        base = render(svg, width, height)
    except Exception:
        return {"edit_spread": float("nan"), "edit_tear": float("nan"), "edit_probes": 0.0}

    base_bg = float((base[..., 3] <= TRANSPARENT_ALPHA).mean())
    total_px = base.shape[0] * base.shape[1]

    # Collect (span, numbers) for every path data attribute.
    targets: list[tuple[int, int, list[str]]] = []
    for m in re.finditer(r'\sd\s*=\s*"([^"]*)"', svg):
        nums = _NUM_RE.findall(m.group(1))
        if len(nums) >= 4:
            targets.append((m.start(1), m.end(1), nums))
    if not targets:
        return {"edit_spread": float("nan"), "edit_tear": float("nan"), "edit_probes": 0.0}

    spreads: list[float] = []
    tears: list[float] = []
    for _ in range(n_probes):
        s, e, nums = targets[rng.integers(len(targets))]
        d_old = svg[s:e]
        # Pick a coordinate pair and displace it.
        i = int(rng.integers(0, max(1, len(nums) - 1)))
        i -= i % 2
        try:
            new_nums = list(nums)
            new_nums[i] = f"{float(nums[i]) + delta:.4f}"
            if i + 1 < len(new_nums):
                new_nums[i + 1] = f"{float(nums[i + 1]) + delta:.4f}"
        except ValueError:
            continue

        it = iter(new_nums)
        d_new = _NUM_RE.sub(lambda _m: next(it), d_old)
        cand = svg[:s] + d_new + svg[e:]

        try:
            out = render(cand, width, height)
        except Exception:
            continue

        changed = float((np.abs(out - base).max(axis=-1) > 2 / 255).mean())
        bg_after = float((out[..., 3] <= TRANSPARENT_ALPHA).mean())
        spreads.append(changed)
        tears.append(max(0.0, bg_after - base_bg))

    if not spreads:
        return {"edit_spread": float("nan"), "edit_tear": float("nan"), "edit_probes": 0.0}
    return {
        "edit_spread": float(np.median(spreads)),
        "edit_tear": float(np.median(tears)),
        "edit_probes": float(len(spreads)),
    }
