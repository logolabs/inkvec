"""Adversarial boundary perturbation — the robustness protocol.

AnchorFlow's most damaging result about existing tracers is not a fidelity number, it
is a *stability* number. Perturb the input boundaries slightly and measure how much the
output's parameter count grows:

    AnchorFlow   +2.9%
    AdaVec      +20.7%
    VTracer    +106.7%

VTracer more than doubles because boundary-following tracing inherits contour noise
directly as redundant anchors: every wobble the segmentation invents becomes a real
anchor in the output. That is the failure this project's sub-pixel front end and node
budget are supposed to prevent, so the harness has to be able to reproduce the
experiment. Anchor growth under perturbation is the headline robustness metric.

Three perturbation families, because they stress different parts of a pipeline:

``noise``   — per-pixel colour noise; stresses the region-clustering stage.
``jpeg``    — real JPEG round-trip; the single most common real-world degradation.
``boundary``— displaces region boundaries with smooth low-frequency warp; the closest
              analogue to AnchorFlow's "imperfect segmentation mask" setting.
"""
from __future__ import annotations

import io
from pathlib import Path

import numpy as np
from PIL import Image
from scipy.ndimage import gaussian_filter, map_coordinates


def add_noise(rgba: np.ndarray, sigma: float = 0.02, seed: int = 0) -> np.ndarray:
    rng = np.random.default_rng(seed)
    out = rgba.copy()
    out[..., :3] = np.clip(out[..., :3] + rng.normal(0, sigma, out[..., :3].shape), 0, 1)
    return out


def jpeg_roundtrip(rgba: np.ndarray, quality: int = 60) -> np.ndarray:
    """Composite on white, JPEG-encode, decode. Alpha is lost, as it is in real life."""
    rgb = rgba[..., :3] * rgba[..., 3:4] + (1.0 - rgba[..., 3:4])
    img = Image.fromarray((np.clip(rgb, 0, 1) * 255).astype(np.uint8), mode="RGB")
    buf = io.BytesIO()
    img.save(buf, format="JPEG", quality=quality)
    buf.seek(0)
    out = np.asarray(Image.open(buf).convert("RGB"), dtype=np.float32) / 255.0
    return np.dstack([out, np.ones(out.shape[:2], dtype=np.float32)])


def warp_boundaries(
    rgba: np.ndarray, amplitude: float = 0.75, scale: float = 8.0, seed: int = 0
) -> np.ndarray:
    """Smooth low-frequency spatial warp, displacing boundaries by ~``amplitude`` px.

    Deliberately *smooth*: a well-behaved tracer should follow a gently displaced
    boundary with the same number of anchors it used before. Growth here means the
    tracer is spending anchors on the perturbation rather than on the shape.
    """
    rng = np.random.default_rng(seed)
    h, w = rgba.shape[:2]
    dx = gaussian_filter(rng.normal(0, 1, (h, w)), scale)
    dy = gaussian_filter(rng.normal(0, 1, (h, w)), scale)
    for d in (dx, dy):
        s = d.std()
        if s > 0:
            d *= amplitude / s

    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    coords = np.array([yy + dy, xx + dx])
    out = np.empty_like(rgba)
    for c in range(rgba.shape[2]):
        out[..., c] = map_coordinates(rgba[..., c], coords, order=1, mode="nearest")
    return np.clip(out, 0, 1)


PERTURBATIONS = {
    "noise": lambda a, seed: add_noise(a, 0.02, seed),
    "jpeg": lambda a, seed: jpeg_roundtrip(a, 60),
    "boundary": lambda a, seed: warp_boundaries(a, 0.75, 8.0, seed),
}


def build_perturbed(
    png_paths: list[Path], out_dir: Path, kinds: tuple[str, ...] = tuple(PERTURBATIONS), seed: int = 0
) -> dict[str, list[Path]]:
    """Write perturbed copies of each input. Returns {kind: [paths]}."""
    from ..render import load_rgba, save_rgba

    out_dir = Path(out_dir)
    result: dict[str, list[Path]] = {k: [] for k in kinds}
    for kind in kinds:
        d = out_dir / kind
        d.mkdir(parents=True, exist_ok=True)
        fn = PERTURBATIONS[kind]
        for p in png_paths:
            try:
                arr = load_rgba(p)
                dest = d / p.name
                save_rgba(fn(arr, seed), dest)
                result[kind].append(dest)
            except Exception as e:
                print(f"  ! perturb {kind} failed for {p.name}: {e}")
    return result
