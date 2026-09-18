"""Structural comparison against a ground-truth SVG.

Only available for the synthetic and icon corpora, where we rendered the raster from a
known SVG and therefore know what the answer was supposed to look like structurally,
not just chromatically. This is what lets us ask the question pixel metrics cannot:
*the input really was a circle — did the tracer recover a circle, or 40 anchors?*
"""
from __future__ import annotations

import numpy as np

from ..svgmodel import DocInfo, ElementInfo


def _bbox(e: ElementInfo) -> tuple[float, float, float, float] | None:
    if not e.rings:
        return None
    pts = np.vstack(e.rings)
    return float(pts[:, 0].min()), float(pts[:, 1].min()), float(pts[:, 0].max()), float(pts[:, 1].max())


def _iou(a, b) -> float:
    ax0, ay0, ax1, ay1 = a
    bx0, by0, bx1, by1 = b
    ix0, iy0 = max(ax0, bx0), max(ay0, by0)
    ix1, iy1 = min(ax1, bx1), min(ay1, by1)
    iw, ih = max(0.0, ix1 - ix0), max(0.0, iy1 - iy0)
    inter = iw * ih
    ua = (ax1 - ax0) * (ay1 - ay0) + (bx1 - bx0) * (by1 - by0) - inter
    return inter / ua if ua > 0 else 0.0


def primitive_recall(gt: DocInfo, cand: DocInfo, iou_threshold: float = 0.5) -> dict[str, float]:
    """Of the primitives the source actually contained, how many came back as primitives?

    Greedy bbox-IoU matching, scale-normalized so a candidate rendered in a different
    user-unit system still matches. A recovered ``<circle>`` counts; four cubics in
    the same place do not.
    """
    gt_prims = [e for e in gt.elements if e.is_primitive and e.rings]
    if not gt_prims:
        return {"primitive_recall": float("nan"), "n_gt_primitives": 0.0}

    # Normalize candidate coordinates into the ground truth's unit space.
    sx = gt.width / cand.width if cand.width else 1.0
    sy = gt.height / cand.height if cand.height else 1.0

    cand_boxes: list[tuple[tuple[float, float, float, float], bool, str]] = []
    for e in cand.elements:
        bb = _bbox(e)
        if bb is None:
            continue
        cand_boxes.append(((bb[0] * sx, bb[1] * sy, bb[2] * sx, bb[3] * sy),
                           e.is_primitive or bool(e.seg_types.get("Arc")), e.kind))

    used = set()
    hits = 0
    for g in gt_prims:
        gb = _bbox(g)
        if gb is None:
            continue
        best, best_i = 0.0, -1
        for i, (cb, is_prim, _kind) in enumerate(cand_boxes):
            if i in used:
                continue
            v = _iou(gb, cb)
            if v > best:
                best, best_i = v, i
        if best_i >= 0 and best >= iou_threshold:
            used.add(best_i)
            if cand_boxes[best_i][1]:
                hits += 1

    return {"primitive_recall": hits / len(gt_prims), "n_gt_primitives": float(len(gt_prims))}


def compare_structure(gt: DocInfo, cand: DocInfo) -> dict[str, float]:
    """Ratios against ground truth. 1.0 means the tracer matched the source's economy."""
    def ratio(c: float, g: float) -> float:
        return c / g if g > 0 else float("nan")

    out = {
        "gt_anchors": float(gt.n_anchors),
        "gt_params": float(gt.n_params),
        "gt_elements": float(gt.n_elements),
        "anchor_ratio": ratio(cand.n_anchors, gt.n_anchors),
        # AnchorFlow reports this quantity directly: 61.2 params vs VTracer's 206.4.
        "param_ratio": ratio(cand.n_params, gt.n_params),
        "element_ratio": ratio(cand.n_elements, gt.n_elements),
        "length_ratio": ratio(cand.total_length, gt.total_length),
    }
    out.update(primitive_recall(gt, cand))
    return out
