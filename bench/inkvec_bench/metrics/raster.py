"""Raster fidelity metrics, evaluated at multiple scales.

Multi-scale is not a refinement here, it is the point. A vector graphic is a
resolution-independent object, so scoring it only at the resolution of the input
raster rewards a tracer for over-fitting to that resolution's anti-aliasing. Jagged
boundaries and mis-placed sub-pixel edges are close to invisible at 1x and obvious at
4x, which is precisely the difference this project exists to close.
"""
from __future__ import annotations

from functools import lru_cache

import numpy as np
from skimage.metrics import peak_signal_noise_ratio, structural_similarity

from . import dino as m_dino


def psnr(a: np.ndarray, b: np.ndarray) -> float:
    if a.shape != b.shape:
        raise ValueError(f"shape mismatch {a.shape} vs {b.shape}")
    if np.allclose(a, b):
        return 100.0
    return float(peak_signal_noise_ratio(a, b, data_range=1.0))


def ssim(a: np.ndarray, b: np.ndarray) -> float:
    if a.shape != b.shape:
        raise ValueError(f"shape mismatch {a.shape} vs {b.shape}")
    win = min(7, min(a.shape[:2]))
    if win % 2 == 0:
        win -= 1
    if win < 3:
        return float("nan")
    return float(
        structural_similarity(a, b, channel_axis=-1, data_range=1.0, win_size=win)
    )


# --- perceptual (optional; require torch) -----------------------------------------


@lru_cache(maxsize=1)
def _lpips_model():
    try:
        import lpips  # type: ignore
        import torch  # noqa: F401
    except ImportError:
        return None
    try:
        return lpips.LPIPS(net="alex", verbose=False).to(_device()).eval()
    except Exception:
        return None


@lru_cache(maxsize=1)
def _dists_model():
    try:
        from DISTS_pytorch import DISTS  # type: ignore
        import torch  # noqa: F401
    except ImportError:
        return None
    try:
        return DISTS().to(_device()).eval()
    except Exception:
        return None


# LPIPS and DISTS are trained on small crops; feeding them a 4096px render is both
# far slower and less meaningful than evaluating at a bounded size. PSNR/SSIM/deltaE
# stay at full resolution, where they are exact.
PERCEPTUAL_MAX_SIDE = 512


@lru_cache(maxsize=1)
def _device():
    try:
        import torch

        return "cuda" if torch.cuda.is_available() else "cpu"
    except ImportError:
        return "cpu"


def _to_tensor(rgb: np.ndarray):
    import torch
    import torch.nn.functional as F

    t = torch.from_numpy(np.ascontiguousarray(rgb.transpose(2, 0, 1)))[None].float()
    h, w = t.shape[-2:]
    m = max(h, w)
    if m > PERCEPTUAL_MAX_SIDE:
        s = PERCEPTUAL_MAX_SIDE / m
        t = F.interpolate(
            t, size=(max(1, round(h * s)), max(1, round(w * s))),
            mode="area",
        )
    return t.to(_device())


def lpips_distance(a: np.ndarray, b: np.ndarray) -> float:
    """LPIPS (AlexNet). Lower is better. NaN when torch/lpips is unavailable."""
    model = _lpips_model()
    if model is None:
        return float("nan")
    import torch

    with torch.no_grad():
        # LPIPS expects inputs in [-1, 1].
        d = model(_to_tensor(a) * 2 - 1, _to_tensor(b) * 2 - 1)
    return float(d.item())


def dists_distance(a: np.ndarray, b: np.ndarray) -> float:
    """DISTS. Lower is better. NaN when DISTS-pytorch is unavailable.

    Preferred over LPIPS for this task: DISTS is explicitly texture-tolerant and
    structure-sensitive, which matches what we care about (boundary placement) and
    what we do not (imperceptible flat-fill drift).
    """
    model = _dists_model()
    if model is None:
        return float("nan")
    import torch

    with torch.no_grad():
        d = model(_to_tensor(a), _to_tensor(b))
    return float(d.item())


def perceptual_available() -> dict[str, bool]:
    return {
        "lpips": _lpips_model() is not None,
        "dists": _dists_model() is not None,
        "dino": m_dino.available(),
    }


def compare(a_rgb: np.ndarray, b_rgb: np.ndarray, perceptual: bool = True) -> dict[str, float]:
    """All raster metrics for one pair of composited RGB images in [0, 1].

    ``dino`` is the odd one out in this dict: higher is better, where ``lpips`` and
    ``dists`` are distances. It is NaN when the DINO checkpoint is unavailable.
    """
    out = {"psnr": psnr(a_rgb, b_rgb), "ssim": ssim(a_rgb, b_rgb)}
    if perceptual:
        out["lpips"] = lpips_distance(a_rgb, b_rgb)
        out["dists"] = dists_distance(a_rgb, b_rgb)
        out["dino"] = m_dino.dino_score(a_rgb, b_rgb)
    return out
