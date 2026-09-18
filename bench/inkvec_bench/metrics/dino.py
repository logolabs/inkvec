"""DinoScore — cosine similarity between self-supervised ViT embeddings.

Introduced by the StarVector paper (SVG-Bench) as the perceptual headline metric for
image-to-SVG, and the one on which learned methods claim their advantage over
tracers. Higher is better; 1.0 is identical.

The metric is deliberately *semantic* rather than pixel-exact: DINO features are
trained to be invariant to the kind of low-level variation (sub-pixel edge placement,
mild colour drift) that PSNR punishes hardest. That makes it a useful complement to
the rest of the suite and a poor substitute for it — a tracer can gain DinoScore by
producing something that "reads as" the same glyph while losing SSIM, and vice versa.

Two backbones are selectable:

``dinov3``
    The default, and what this harness reports. ``facebook/dinov3-vitb16-pretrain-lvd1689m``.
    Gated on HuggingFace: the weights need a licence acceptance on the model page plus
    ``huggingface-cli login``. When they cannot be fetched the score degrades to NaN
    rather than silently falling back to another backbone, because a DINOv3 number and
    a DINOv2 number are not interchangeable.

``dinov2``
    ``facebook/dinov2-base`` — ungated, and the backbone the published SVG-Bench table
    used. Kept selectable purely so our numbers can be placed next to theirs; anything
    reported as a headline should come from the default.

Each backbone is loaded with its own ``AutoImageProcessor`` rather than a hand-rolled
resize, so the preprocessing matches what a reference implementation would do. Note
these differ: DINOv2's processor resizes the shortest edge to 256 and centre-crops 224
(so a little border is discarded), DINOv3's resizes straight to 224x224. That is the
published behaviour of each and is left alone.
"""
from __future__ import annotations

from functools import lru_cache

import numpy as np

# Backbone id -> HuggingFace repo. ``AutoModel`` handles both architectures.
BACKBONES = {
    "dinov3": "facebook/dinov3-vitb16-pretrain-lvd1689m",
    "dinov2": "facebook/dinov2-base",
}

#: Backbone used when a caller does not name one. DINOv3 is the current generation and
#: what we score against going forward; see the note above about comparability.
DEFAULT_BACKBONE = "dinov3"


@lru_cache(maxsize=1)
def _device() -> str:
    try:
        import torch

        return "cuda" if torch.cuda.is_available() else "cpu"
    except ImportError:
        return "cpu"


@lru_cache(maxsize=len(BACKBONES))
def _model(backbone: str):
    """``(processor, model)`` for a backbone, or ``None`` if it cannot be loaded.

    Cached at module level: the weights are ~90M parameters and loading them per call
    would dominate the runtime of a sweep. Every failure mode — transformers missing,
    no network, gated repo refusing an unauthenticated download — collapses to ``None``
    so that the rest of the harness keeps running.
    """
    repo = BACKBONES.get(backbone)
    if repo is None:
        raise ValueError(f"unknown DINO backbone {backbone!r}; known: {sorted(BACKBONES)}")
    try:
        import torch  # noqa: F401
        from transformers import AutoImageProcessor, AutoModel
    except ImportError:
        return None
    try:
        processor = AutoImageProcessor.from_pretrained(repo)
        model = AutoModel.from_pretrained(repo).to(_device()).eval()
    except Exception:
        return None
    return processor, model


def load_error(backbone: str = DEFAULT_BACKBONE) -> str:
    """Empty string if the backbone loads, else a description of why it does not.

    Separate from ``_model`` so that a caller wanting to *report* a failure can get the
    exception text, while the scoring path stays quiet and returns NaN.
    """
    repo = BACKBONES.get(backbone)
    if repo is None:
        return f"unknown backbone {backbone!r}"
    if _model(backbone) is not None:
        return ""
    try:
        import torch  # noqa: F401
        from transformers import AutoImageProcessor, AutoModel
    except ImportError as e:
        return f"missing dependency: {e}"
    # Reproduce the load outside the swallowing cache to recover the real message —
    # a gated repo reports a 401/403 here rather than anything obvious upstream.
    try:
        AutoImageProcessor.from_pretrained(repo)
        AutoModel.from_pretrained(repo)
    except Exception as e:
        return f"{type(e).__name__}: {e}"
    return "unknown load failure"


def available(backbone: str = DEFAULT_BACKBONE) -> bool:
    return _model(backbone) is not None


def _as_uint8(rgb: np.ndarray) -> np.ndarray:
    """``(H, W, 3)`` float in [0, 1] -> uint8, which is what the processor expects."""
    if rgb.ndim != 3 or rgb.shape[2] != 3:
        raise ValueError(f"expected (H, W, 3) RGB, got {rgb.shape}")
    return (np.clip(rgb, 0.0, 1.0) * 255.0 + 0.5).astype(np.uint8)


def embed(images: list[np.ndarray], backbone: str = DEFAULT_BACKBONE):
    """L2-normalised CLS embeddings for a batch of composited RGB images in [0, 1].

    Returns an ``(N, D)`` float32 numpy array, or ``None`` when the backbone is
    unavailable. Exposed separately from :func:`dino_score` because a sweep scores many
    candidates against a handful of references, and re-embedding a reference per pair
    is pure waste.
    """
    loaded = _model(backbone)
    if loaded is None:
        return None
    processor, model = loaded
    import torch

    batch = processor(images=[_as_uint8(im) for im in images], return_tensors="pt")
    batch = {k: v.to(_device()) for k, v in batch.items()}
    with torch.no_grad():
        out = model(**batch)
    # ``pooler_output`` is the CLS token for both DINOv2 and DINOv3 — the canonical
    # whole-image DINO descriptor, and what SVG-Bench compares.
    feats = getattr(out, "pooler_output", None)
    if feats is None:
        feats = out.last_hidden_state[:, 0]
    feats = torch.nn.functional.normalize(feats.float(), dim=-1)
    return feats.cpu().numpy()


def dino_score(a_rgb: np.ndarray, b_rgb: np.ndarray, backbone: str = DEFAULT_BACKBONE) -> float:
    """DinoScore for one pair of composited RGB images in [0, 1]. Higher is better.

    NaN when the backbone cannot be loaded, so a missing checkpoint degrades one column
    of the report instead of failing the run.
    """
    feats = embed([a_rgb, b_rgb], backbone=backbone)
    if feats is None:
        return float("nan")
    # Both rows are unit-norm, so the dot product is the cosine similarity. Clipped
    # because float error can put an identical pair a hair above 1.0.
    return float(np.clip(float(feats[0] @ feats[1]), -1.0, 1.0))
