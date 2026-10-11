"""Perceptual colour error.

RGB distance is not perceptual distance, which is the reason VTracer's
``color_precision`` (significant bits per RGB channel) is a blunt instrument: it
splits colours a viewer cannot tell apart while merging ones they can. We score in
CIELAB with CIEDE2000 so that a palette decision is judged the way an eye judges it.

Reported as mean and p95: the mean says whether the palette is right on average, the
p95 catches the handful of regions given badly wrong colours — which is what a viewer
actually notices.
"""
from __future__ import annotations

from functools import lru_cache

import numpy as np
from skimage.color import deltaE_ciede2000, rgb2lab


# CIEDE2000 is expensive per pixel, and mean/p95 are distributional statistics: a
# quarter-million samples pins them far tighter than the differences we care about.
# Subsampling here costs nothing in fidelity and roughly 4x in speed at 1024px.
MAX_DELTA_E_SAMPLES = 250_000


@lru_cache(maxsize=4)
def _sample(n: int, k: int) -> np.ndarray:
    """The `k` of `n` pixels `delta_e00` reads: always the same ones for the same `n`."""
    idx = np.random.default_rng(0).choice(n, k, replace=False)
    idx.flags.writeable = False
    return idx


def delta_e00(a_rgb: np.ndarray, b_rgb: np.ndarray, mask: np.ndarray | None = None) -> dict[str, float]:
    """CIEDE2000 between two RGB images in [0, 1], optionally restricted to a mask.

    Only the sampled pixels whose colours differ are converted and compared: a pixel the
    same in both images has a CIEDE2000 of exactly 0 (the formula's every term is a
    difference of equal numbers), and the conversion is per pixel, so the array of
    differences, and every statistic of it, is the one the whole sample would give."""
    a = np.clip(a_rgb, 0, 1).reshape(-1, 3)
    b = np.clip(b_rgb, 0, 1).reshape(-1, 3)
    if mask is not None:
        m = mask.reshape(-1)
        a, b = a[m], b[m]
    if a.shape[0] == 0:
        return {"de00_mean": float("nan"), "de00_p95": float("nan"), "de00_max": float("nan")}
    if a.shape[0] > MAX_DELTA_E_SAMPLES:
        idx = _sample(a.shape[0], MAX_DELTA_E_SAMPLES)
        a, b = a[idx], b[idx]

    diff = np.flatnonzero((a != b).any(axis=1))
    # The dtype the whole sample's differences would have (skimage keeps float32 as float32).
    probe = deltaE_ciede2000(rgb2lab(a[:1].reshape(-1, 1, 3)), rgb2lab(b[:1].reshape(-1, 1, 3)))
    de = np.zeros(a.shape[0], dtype=probe.dtype)
    if diff.size:
        lab_a = rgb2lab(a[diff].reshape(-1, 1, 3))
        lab_b = rgb2lab(b[diff].reshape(-1, 1, 3))
        de[diff] = deltaE_ciede2000(lab_a, lab_b).reshape(-1)
    de = de[np.isfinite(de)]
    if de.size == 0:
        return {"de00_mean": float("nan"), "de00_p95": float("nan"), "de00_max": float("nan")}
    return {
        "de00_mean": float(de.mean()),
        "de00_p95": float(np.percentile(de, 95)),
        "de00_max": float(de.max()),
    }


def palette_size(rgba: np.ndarray, alpha_min: float = 0.9, tol: float = 2.0) -> int:
    """Number of perceptually distinct colours among opaque pixels.

    Counted by greedy CIEDE2000 clustering with a just-noticeable-difference
    threshold, so it measures what a viewer would call "how many colours is this",
    not how many distinct RGB triples the file happens to contain. Banding a smooth
    gradient into layers inflates this; fitting it as a gradient does not.
    """
    opaque = rgba[..., 3] >= alpha_min
    if not opaque.any():
        return 0
    px = rgba[..., :3][opaque].reshape(-1, 3)
    # Subsample for tractability; palette size is a coarse statistic.
    if px.shape[0] > 20000:
        idx = np.random.default_rng(0).choice(px.shape[0], 20000, replace=False)
        px = px[idx]
    uniq = np.unique((px * 255).astype(np.uint8), axis=0).astype(np.float32) / 255.0
    if uniq.shape[0] > 4000:
        return int(uniq.shape[0])  # already far past any sane palette; skip clustering
    lab = rgb2lab(uniq.reshape(-1, 1, 3)).reshape(-1, 3)
    centers: list[np.ndarray] = []
    for c in lab:
        if not centers:
            centers.append(c)
            continue
        C = np.asarray(centers)
        d = deltaE_ciede2000(
            C.reshape(-1, 1, 3), np.repeat(c.reshape(1, 1, 3), C.shape[0], axis=0)
        ).ravel()
        if d.min() > tol:
            centers.append(c)
    return len(centers)
