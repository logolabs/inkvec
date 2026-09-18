"""The pre-pass itself: upscale, halve, and put the colours back.

Measured during development. The short version is that the
upscaler removes about two thirds of a JPEG's error energy -- not because it was
trained to, it was not, but because it was fitted on flat-colour logo art and
ringing is not on that manifold, so it cannot represent the damage.

Three steps, and each is here for a measured reason.

**Upscale x4.** This is what actually cleans. Artefact gain -- what survives the
network relative to what went in -- is 0.36 across seven families.

**Box-downsample x2.** Not a denoiser, whatever it looks like. A 2x2 box has its
only transfer zero at Nyquist, and the DCT grid puts JPEG damage at f = 1/8 and
below, which it passes at 0.974 of amplitude; half the error survives and the
surviving half is the *more* coherent half. It is here because x2 is the useful
output scale, not because it cleans.

**Recolour.** The network costs about 0.6 dE00 on clean input too, so that cost
is not about JPEG. Split by region against bicubic as control, it is 0.60 on flat
interiors where bicubic scores 0.03, and 1.57 on edges where bicubic scores 8.63:
each resampler is right about the half the other is wrong about. So take geometry
from the network and colour from the source. What survives is the edge column,
which is a sub-pixel boundary disagreement and the tracer's problem, not this
module's.
"""
from __future__ import annotations

import numpy as np

#: Interior residual above which an input is treated as degraded. Measured at
#: 0.000 on an exact-coverage clean intake and 1.509 on JPEG q50, so anywhere in
#: between is safe; this sits low enough to catch milder damage.
DEGRADED_RESIDUAL = 0.5


def box_downsample(a: np.ndarray, factor: int = 2) -> np.ndarray:
    """Exact box average by an integer factor."""
    f = factor
    h, w = a.shape[0] // f * f, a.shape[1] // f * f
    b = a[:h, :w]
    return b.reshape(h // f, f, w // f, f, -1).mean(axis=(1, 3))


def _flat_mask(img: np.ndarray, tol: float) -> np.ndarray:
    """True where the 3x3 neighbourhood spans less than `tol`."""
    g = img.mean(axis=2)
    lo = hi = g
    for dy in (-1, 0, 1):
        for dx in (-1, 0, 1):
            r = np.roll(np.roll(g, dy, 0), dx, 1)
            lo, hi = np.minimum(lo, r), np.maximum(hi, r)
    flat = (hi - lo) < tol
    flat[:1, :] = flat[-1:, :] = flat[:, :1] = flat[:, -1:] = False
    return flat


def match_flats(hi: np.ndarray, src: np.ndarray, tol: float = 3.0 / 255.0
                ) -> np.ndarray:
    """Put the upscaler's flat regions back on the source's colours.

    One affine map per channel, fitted from the upscaled image to a bicubic
    upsample of the same input, over the pixels the bicubic image says are flat.
    Uses nothing but the input, so it is available at inference; the artist's
    file is never consulted. Both arrays are in [0, 1].
    """
    from PIL import Image

    bi = np.asarray(
        Image.fromarray((src.clip(0, 1) * 255).astype(np.uint8)).resize(
            (hi.shape[1], hi.shape[0]), Image.BICUBIC), np.float64) / 255.0
    flat = _flat_mask(bi, tol)
    if flat.mean() < 0.01:  # nothing flat enough to fit on; use everything
        flat = np.ones(bi.shape[:2], bool)

    out = hi.astype(np.float64).copy()
    for c in range(3):
        x, y = hi[..., c][flat], bi[..., c][flat]
        if x.size < 16 or x.std() < 1e-6:
            continue
        A = np.stack([x, np.ones_like(x)], axis=1)
        a, b = np.linalg.lstsq(A, y, rcond=None)[0]
        out[..., c] = hi[..., c] * a + b
    return out.clip(0, 1)


def prepass(up, rgba: np.ndarray, out_scale: int = 2, recolour: bool = True,
            **kw) -> np.ndarray:
    """RGBA in [0, 1] -> cleaned RGBA at `out_scale` times the input size."""
    from . import model

    hi = model.upscale_rgba(up, rgba, **kw)
    factor = up.scale // out_scale
    if factor > 1:
        hi = box_downsample(hi, factor)
    elif factor < 1:
        raise ValueError(f"cannot output x{out_scale} from an x{up.scale} model")
    if recolour:
        # Fit against the RGB the model actually saw, not the raw file. Where a
        # PNG is transparent its colour channels hold whatever the encoder left
        # there, and `upscale_rgba` zeroes those before the network sees them;
        # matching against the un-zeroed original would fit the affine map to
        # pixels that never entered the upscale.
        src = np.where(rgba[..., 3:4] > 1e-4, rgba[..., :3], 0.0)
        hi = np.dstack([match_flats(hi[..., :3], src), hi[..., 3:4]])
    return hi.clip(0, 1)


# --------------------------------------------------------------------- detection
def interior_residual(inp_rgb: np.ndarray, model_rgb: np.ndarray) -> float:
    """RMS disagreement with the input, in 8-bit levels, where the trace is flat.

    Both arrays are the same size, in [0, 1], composited on white. The traced
    model is piecewise flat by construction, so wherever it is locally constant
    the input should be too. On an undamaged raster this is 0.000; JPEG q50 puts
    it at 1.509, which is the widest margin of any reference-free signal tried.

    Note that the cheaper candidate -- an 8x8 block signature, which needs no
    trace at all -- was measured and does not work: 2.175 on clean against 2.133
    on JPEG q50 is no signal. This one costs a first trace and is the one that
    discriminates.
    """
    flat = _flat_mask(model_rgb, 1.5 / 255.0)
    if flat.sum() < 100:
        return float("nan")
    d = (model_rgb - inp_rgb)[flat] * 255.0
    return float(np.sqrt((d ** 2).mean() / 3))
