"""Perceptual distances the gate can afford: pure NumPy, deterministic, no torch.

dE00 asks "is this pixel the right colour". It answers well over a whole icon and badly
over a small edit: a boundary half a pixel out of place barely moves it, and that is the
shape of every change a beautifier or a learned prior makes. These two metrics are
gradient- and wavelet-based, so a displaced or softened edge is exactly what they see.

* [`gmsd`] -- Gradient Magnitude Similarity Deviation (Xue et al., 2014). The standard
  deviation of a local gradient-similarity map: 0 for identical images, larger as edges
  move. Twenty lines, no parameters to choose beyond the published constant.
* [`haarpsi`] -- Haar Perceptual Similarity Index (Reisenhofer et al., 2018), returned
  here as a *distance* (1 - similarity) so that, like every other number the gate holds,
  lower is better.

Both take float images in [0, 1], shape (H, W, 3) or (H, W). Both are pure array
arithmetic with fixed constants: the same input gives the same answer on every platform,
which a ratcheting gate needs.
"""
from __future__ import annotations

import numpy as np

__all__ = ["gmsd", "haarpsi", "luma"]


def luma(img: np.ndarray) -> np.ndarray:
    """ITU-R BT.601 luma on a 0-255 scale, which is what both papers assume."""
    a = np.asarray(img, dtype=np.float64)
    if a.ndim == 2:
        y = a
    else:
        y = 0.299 * a[..., 0] + 0.587 * a[..., 1] + 0.114 * a[..., 2]
    return y * 255.0


def _conv_same(img: np.ndarray, kernel: np.ndarray) -> np.ndarray:
    """2-D correlation with symmetric padding, by shifted adds (no SciPy)."""
    kh, kw = kernel.shape
    ph, pw = kh // 2, kw // 2
    pad = np.pad(img, ((ph, ph), (pw, pw)), mode="symmetric")
    out = np.zeros_like(img, dtype=np.float64)
    h, w = img.shape
    for i in range(kh):
        for j in range(kw):
            k = kernel[i, j]
            if k != 0.0:
                out += k * pad[i:i + h, j:j + w]
    return out


def _pool2(y: np.ndarray) -> np.ndarray:
    """The 2x2 average and decimation both papers' reference code applies first."""
    h, w = y.shape[0] - y.shape[0] % 2, y.shape[1] - y.shape[1] % 2
    a = y[:h, :w]
    return 0.25 * (a[0::2, 0::2] + a[0::2, 1::2] + a[1::2, 0::2] + a[1::2, 1::2])


_PREWITT_X = np.array([[1.0, 0.0, -1.0]] * 3) / 3.0
_PREWITT_Y = _PREWITT_X.T


def gmsd(a: np.ndarray, b: np.ndarray, t: float = 170.0) -> float:
    """Gradient Magnitude Similarity Deviation. 0 = identical; ~0.35 = grossly different."""
    ya, yb = _pool2(luma(a)), _pool2(luma(b))
    ga = np.hypot(_conv_same(ya, _PREWITT_X), _conv_same(ya, _PREWITT_Y))
    gb = np.hypot(_conv_same(yb, _PREWITT_X), _conv_same(yb, _PREWITT_Y))
    gms = (2.0 * ga * gb + t) / (ga * ga + gb * gb + t)
    return float(np.std(gms))


def _haar_responses(y: np.ndarray, scales: int = 3) -> list[tuple[np.ndarray, np.ndarray]]:
    """High-pass Haar responses per scale, horizontal and vertical."""
    out = []
    for j in range(1, scales + 1):
        n = 2 ** j
        lo = np.ones(n) / n
        hi = np.concatenate([np.ones(n // 2), -np.ones(n // 2)]) * (2.0 / n)
        # Separable: high-pass along one axis, low-pass along the other.
        kx = np.outer(lo, hi)
        ky = np.outer(hi, lo)
        out.append((_conv_same(y, kx), _conv_same(y, ky)))
    return out


def haarpsi(a: np.ndarray, b: np.ndarray, c: float = 30.0, alpha: float = 4.2) -> float:
    """Haar Perceptual Similarity Index, as a distance: 0 = identical, 1 = unrelated.

    Luma only (the paper's chroma term changes little on flat-coloured artwork and costs
    two more convolutions per scale).
    """
    ya, yb = _pool2(luma(a)), _pool2(luma(b))
    ra, rb = _haar_responses(ya), _haar_responses(yb)
    num = np.zeros_like(ya)
    den = np.zeros_like(ya)
    for k in (0, 1):  # horizontal, then vertical
        # Similarity from the two finest scales, weighted by the coarsest response.
        s = np.zeros_like(ya)
        for j in (0, 1):
            fa, fb = np.abs(ra[j][k]), np.abs(rb[j][k])
            s += (2.0 * fa * fb + c) / (fa * fa + fb * fb + c)
        s *= 0.5
        w = np.maximum(np.abs(ra[2][k]), np.abs(rb[2][k]))
        num += 1.0 / (1.0 + np.exp(-alpha * s)) * w
        den += w
    if den.sum() <= 0.0:
        return 0.0
    mean_l = num.sum() / den.sum()
    # Invert the logistic, then square, as the paper defines the index.
    mean_l = min(max(mean_l, 1e-12), 1.0 - 1e-12)
    sim = (np.log(mean_l / (1.0 - mean_l)) / alpha) ** 2
    return float(1.0 - min(max(sim, 0.0), 1.0))
