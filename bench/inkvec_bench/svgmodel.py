"""Structural model of an SVG document.

This is where the editability metrics come from, and it is the part of the harness
with no equivalent in the published literature. Existing benchmarks score a
vectorizer on how closely its *pixels* match; none score whether the thing it emitted
is a document a designer can open and work with.

The distinctions that matter here:

* A ``<circle>`` carries three numbers and zero anchors. The same circle written as
  four cubic Beziers carries twenty-four numbers and four anchors, renders no better,
  and cannot be resized by dragging one handle. Both score identically on SSIM.
* Two adjacent regions that each carry their own copy of the shared boundary will
  either overlap (overdraw) or leave a hairline gap (seam). Both are invisible to
  pixel metrics at source resolution and obvious at 4x.
* Anchors are only meaningful per unit of boundary length. A complex logo legitimately
  needs more anchors than a simple one; anchor *density* is the comparable quantity.
"""
from __future__ import annotations

import io
import math
from dataclasses import dataclass, field
from typing import Iterable, Iterator

import numpy as np

try:
    import svgelements as se
except ImportError as e:  # pragma: no cover
    raise ImportError("inkvec-bench requires svgelements: pip install svgelements") from e


SVG_NS = "http://www.w3.org/2000/svg"
XLINK_NS = "http://www.w3.org/1999/xlink"

# Element kinds we treat as geometric primitives: shapes whose identity survives into
# an editor as a manipulable object rather than as a bag of anchor points.
PRIMITIVE_TYPES = {"Circle", "Ellipse", "Rect", "SimpleLine", "Polygon", "Polyline", "Use"}

# Numeric parameter counts for primitives, for AnchorFlow-comparable parameter budgets.
PRIMITIVE_PARAMS = {
    "Circle": 3,      # cx, cy, r
    "Ellipse": 4,     # cx, cy, rx, ry
    "Rect": 6,        # x, y, w, h, rx, ry
    "SimpleLine": 4,  # x1, y1, x2, y2
    "Use": 6,         # href + transform
}

_FLATTEN_SAMPLES_PER_SEG = 16


@dataclass
class ElementInfo:
    """One drawable element, with its geometry resolved into absolute coordinates."""

    kind: str                      # svgelements class name
    is_primitive: bool
    depth: int                     # nesting depth in the group tree
    group_id: str | None           # nearest ancestor group id, if any
    element_id: str | None
    fill_kind: str                 # "flat" | "gradient" | "none"
    anchors: int = 0               # on-curve points (0 for primitives)
    params: int = 0                # numeric parameters
    seg_types: dict[str, int] = field(default_factory=dict)
    length: float = 0.0            # approximate boundary length in user units
    rings: list[np.ndarray] = field(default_factory=list)  # flattened closed subpaths


@dataclass
class DocInfo:
    """Aggregate structural description of an SVG document."""

    elements: list[ElementInfo]
    width: float
    height: float
    n_groups: int
    n_named_groups: int
    max_depth: int
    parse_error: str | None = None

    # --- aggregates -----------------------------------------------------------
    @property
    def n_elements(self) -> int:
        return len(self.elements)

    @property
    def n_anchors(self) -> int:
        return sum(e.anchors for e in self.elements)

    @property
    def n_params(self) -> int:
        return sum(e.params for e in self.elements)

    @property
    def total_length(self) -> float:
        return sum(e.length for e in self.elements)

    @property
    def anchor_density(self) -> float:
        """Anchors per unit boundary length — the comparable form of "anchor count".

        A ``<circle>`` contributes length but no anchors, so expressing shapes as
        primitives lowers this directly. Lower is better.
        """
        L = self.total_length
        return self.n_anchors / L if L > 0 else 0.0

    @property
    def primitive_fraction(self) -> float:
        """Fraction of boundary length carried by primitives or by true arc segments.

        Length-weighted rather than element-counted, so one big ``<circle>`` is not
        outweighed by a hundred incidental specks.
        """
        L = self.total_length
        if L <= 0:
            return 0.0
        good = 0.0
        for e in self.elements:
            if e.is_primitive:
                good += e.length
            elif e.seg_types.get("Arc"):
                frac = e.seg_types["Arc"] / max(1, sum(e.seg_types.values()))
                good += e.length * frac
        return good / L

    @property
    def seg_type_counts(self) -> dict[str, int]:
        out: dict[str, int] = {}
        for e in self.elements:
            for k, v in e.seg_types.items():
                out[k] = out.get(k, 0) + v
        return out

    @property
    def gradient_fraction(self) -> float:
        L = self.total_length
        if L <= 0:
            return 0.0
        return sum(e.length for e in self.elements if e.fill_kind == "gradient") / L

    @property
    def named_group_fraction(self) -> float:
        return self.n_named_groups / self.n_groups if self.n_groups else 0.0


# --- parsing --------------------------------------------------------------------


def _fill_kind(shape) -> str:
    raw = None
    try:
        raw = (shape.values or {}).get("fill")
    except Exception:
        pass
    if raw is None:
        fill = getattr(shape, "fill", None)
        if fill is None:
            return "none"
        raw = str(fill)
    raw = str(raw).strip().lower()
    if raw.startswith("url("):
        return "gradient"
    if raw in ("none", "transparent", ""):
        return "none"
    return "flat"


def _flatten_segments(segments) -> tuple[list[np.ndarray], float, dict[str, int], int]:
    """Flatten segments into closed rings, accumulating length, types and anchor count."""
    rings: list[np.ndarray] = []
    cur: list[tuple[float, float]] = []
    seg_types: dict[str, int] = {}
    length = 0.0
    anchors = 0
    open_subpath_points = 0

    def close_ring():
        nonlocal cur, open_subpath_points, anchors
        if len(cur) >= 3:
            rings.append(np.asarray(cur, dtype=np.float64))
        # An open subpath has one more on-curve point than it has segments.
        if open_subpath_points:
            anchors += 1
        cur = []
        open_subpath_points = 0

    for seg in segments:
        name = type(seg).__name__
        if name == "Move":
            close_ring()
            try:
                p = seg.end
                cur.append((float(p.x), float(p.y)))
            except Exception:
                pass
            continue
        if name == "Close":
            seg_types["Close"] = seg_types.get("Close", 0) + 1
            # Closing point coincides with the subpath start; not a new anchor.
            if len(cur) >= 3:
                rings.append(np.asarray(cur, dtype=np.float64))
            cur = []
            open_subpath_points = 0
            continue

        seg_types[name] = seg_types.get(name, 0) + 1
        anchors += 1           # each segment terminates at one on-curve point
        open_subpath_points += 1

        n = 2 if name == "Line" else _FLATTEN_SAMPLES_PER_SEG
        pts = []
        for i in range(1, n + 1):
            try:
                p = seg.point(i / n)
                pts.append((float(p.x), float(p.y)))
            except Exception:
                break
        if pts:
            prev = cur[-1] if cur else pts[0]
            for q in pts:
                length += math.hypot(q[0] - prev[0], q[1] - prev[1])
                prev = q
            cur.extend(pts)

    close_ring()
    return rings, length, seg_types, anchors


def _walk(node, depth: int, group_id: str | None) -> Iterator[tuple[object, int, str | None]]:
    """Depth-first walk yielding (shape, depth, nearest_group_id)."""
    for child in node:
        cname = type(child).__name__
        if cname in ("Group", "SVG"):
            gid = getattr(child, "id", None) or group_id
            yield from _walk(child, depth + 1, gid)
        else:
            yield child, depth, group_id


def _defs_geometry(svg: str) -> dict[str, tuple[int, int, float, dict[str, int]]]:
    """Measure geometry declared inside ``<defs>``, keyed by id.

    svgelements does not emit defs content as drawable, so without this a document
    could hide arbitrary complexity behind ``<use>`` and score as though it were free.
    Reuse should be rewarded, but only for what it actually saves: N copies of
    boundary *length* for one copy of the *anchors*.
    """
    import xml.etree.ElementTree as ET

    out: dict[str, tuple[int, int, float, dict[str, int]]] = {}
    try:
        root = ET.fromstring(svg)
    except ET.ParseError:
        return out

    for defs in root.iter(f"{{{SVG_NS}}}defs"):
        for child in defs:
            eid = child.get("id")
            if not eid:
                continue
            tag = child.tag.split("}")[-1]
            if tag in ("linearGradient", "radialGradient", "pattern", "clipPath", "mask", "filter"):
                continue
            frag = (
                f'<svg xmlns="{SVG_NS}" viewBox="0 0 1000 1000">'
                + ET.tostring(child, encoding="unicode")
                + "</svg>"
            )
            try:
                sub = se.SVG.parse(io.StringIO(frag))
                shapes = [s for s in sub.elements() if type(s).__name__ not in ("SVG", "Group")]
                if not shapes:
                    continue
                segs = list(se.Path(shapes[0]).segments())
            except Exception:
                continue
            _rings, length, seg_types, anchors = _flatten_segments(segs)
            params = 0
            for name, count in seg_types.items():
                params += {"Line": 2, "QuadraticBezier": 4, "CubicBezier": 6, "Arc": 7}.get(name, 0) * count
            out[eid] = (anchors, params, length, seg_types)
    return out


def parse(svg: str) -> DocInfo:
    """Parse an SVG string into a structural model. Never raises on malformed input."""
    try:
        doc = se.SVG.parse(io.StringIO(svg))
    except Exception as e:
        return DocInfo([], 0.0, 0.0, 0, 0, 0, parse_error=f"{type(e).__name__}: {e}")

    defs_geom = _defs_geometry(svg)

    try:
        width = float(doc.width)
        height = float(doc.height)
    except Exception:
        width = height = 0.0

    n_groups = 0
    n_named = 0

    def count_groups(node):
        nonlocal n_groups, n_named
        for child in node:
            if type(child).__name__ in ("Group",):
                n_groups += 1
                vals = getattr(child, "values", {}) or {}
                label = (
                    getattr(child, "id", None)
                    or vals.get("inkscape:label")
                    or vals.get("aria-label")
                    or vals.get("data-name")
                )
                # A generated id like "path123" or "g4" is not a name.
                if label and not _is_autogenerated(str(label)):
                    n_named += 1
                count_groups(child)
            elif type(child).__name__ == "SVG":
                count_groups(child)

    count_groups(doc)

    elements: list[ElementInfo] = []
    max_depth = 0
    for shape, depth, gid in _walk(doc, 0, None):
        kind = type(shape).__name__
        if kind in ("Text", "Image", "Desc", "Title", "Use"):
            if kind == "Use":
                # A <use> costs a reference plus a transform, and contributes the
                # referenced boundary length without contributing its anchors again.
                vals = getattr(shape, "values", {}) or {}
                href = (
                    vals.get(f"{{{XLINK_NS}}}href")
                    or vals.get("xlink:href")
                    or vals.get("href")
                    or ""
                )
                ref = defs_geom.get(str(href).lstrip("#"))
                elements.append(
                    ElementInfo(
                        kind=kind, is_primitive=True, depth=depth, group_id=gid,
                        element_id=getattr(shape, "id", None), fill_kind=_fill_kind(shape),
                        anchors=0, params=PRIMITIVE_PARAMS["Use"],
                        length=ref[2] if ref else 0.0,
                    )
                )
                max_depth = max(max_depth, depth)
            continue

        try:
            segments = list(se.Path(shape).segments())
        except Exception:
            continue

        rings, length, seg_types, anchors = _flatten_segments(segments)
        if length <= 0 and not rings:
            continue

        is_prim = kind in PRIMITIVE_TYPES
        if is_prim:
            params = PRIMITIVE_PARAMS.get(kind, max(4, anchors * 2))
            anchors_out = 0
        else:
            params = 0
            for name, count in seg_types.items():
                if name == "Line":
                    params += 2 * count
                elif name == "QuadraticBezier":
                    params += 4 * count
                elif name == "CubicBezier":
                    params += 6 * count
                elif name == "Arc":
                    params += 7 * count
            anchors_out = anchors

        elements.append(
            ElementInfo(
                kind=kind, is_primitive=is_prim, depth=depth, group_id=gid,
                element_id=getattr(shape, "id", None), fill_kind=_fill_kind(shape),
                anchors=anchors_out, params=params, seg_types=seg_types,
                length=length, rings=rings,
            )
        )
        max_depth = max(max_depth, depth)

    # Count each referenced <defs> shape exactly once: its anchors are paid for one
    # time no matter how many <use> elements point at it. Its length is already
    # attributed to the use sites, so it adds none here.
    referenced = {e.kind for e in elements}
    if "Use" in referenced:
        for eid, (anchors, params, _length, seg_types) in defs_geom.items():
            elements.append(
                ElementInfo(
                    kind="Defs", is_primitive=False, depth=0, group_id=None,
                    element_id=eid, fill_kind="flat",
                    anchors=anchors, params=params, seg_types=dict(seg_types), length=0.0,
                )
            )

    return DocInfo(
        elements=elements, width=width, height=height,
        n_groups=n_groups, n_named_groups=n_named, max_depth=max_depth,
    )


def _is_autogenerated(label: str) -> bool:
    """Heuristic: ``path12``/``g4``/``layer1`` are ids, not names a human would give."""
    import re
    return bool(re.fullmatch(r"(path|g|layer|shape|rect|circle|ellipse|polygon|use)[-_]?\d*", label.strip().lower()))


def parse_file(path) -> DocInfo:
    from pathlib import Path
    return parse(Path(path).read_text(encoding="utf-8"))
