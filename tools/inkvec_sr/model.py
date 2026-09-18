"""Loading and running the packaged x4 upscaler.

The weights and architecture are hosted on Hugging Face at
`Logolabs/inkvec-sr-001` and downloaded on first use. A local override can be
provided with `--sr-package` for offline or development use.

Two details are carried over from the packaged CLI because getting them wrong
shows up in the traced output rather than in the raster:

  alpha     the network is RGB-only. Alpha rides through at Lanczos and the RGB
            is zeroed where alpha is ~0 before the model sees it, so transparent
            logos do not pick up a dark halo -- which downstream would become a
            spurious traced contour.
  feather   tiles are blended with a distance-to-edge weight, so a seam does not
            become an edge for the tracer to find.
"""
from __future__ import annotations

import os
import sys
import urllib.request
from dataclasses import dataclass
from pathlib import Path

import numpy as np

HF_REPO_ID = "Logolabs/inkvec-sr-001"
HF_BASE_URL = f"https://huggingface.co/{HF_REPO_ID}/resolve/main"

#: Files the loader needs from the HF repo.
_HF_FILES = ["logo_sr_x4.pt", "mambairv2_arch.py"]


class UpscalerUnavailable(RuntimeError):
    """Raised when the model cannot be loaded, with a reason worth printing."""


#: Fixed RNG seed for the routing sample. Any constant does; what matters is
#: that it is the same one every run.
ROUTING_SEED = 0x5641_4331


def _cache_dir() -> Path:
    """Return the local cache directory for downloaded SR model files."""
    xdg = os.environ.get("XDG_CACHE_HOME")
    if xdg:
        base = Path(xdg)
    elif sys.platform == "win32":
        base = Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData" / "Local"))
    else:
        base = Path.home() / ".cache"
    return base / "inkvec" / "sr"


def _ensure_package() -> Path:
    """Download the SR model package from Hugging Face if not already cached.

    Returns the directory containing ``logo_sr_x4.pt`` and ``mambairv2_arch.py``.
    Uses ``huggingface_hub`` when available (respects its cache), otherwise falls
    back to direct HTTPS download into ``~/.cache/inkvec/sr``.
    """
    # --- Try huggingface_hub first (cleaner, auth-aware, cache-deduped) -------
    try:
        from huggingface_hub import hf_hub_download

        paths = [
            Path(hf_hub_download(repo_id=HF_REPO_ID, filename=f))
            for f in _HF_FILES
        ]
        # All files land in the same snapshot directory.
        return paths[0].parent
    except Exception:  # noqa: BLE001
        pass

    # --- Fallback: plain HTTPS into a local cache directory -------------------
    dest = _cache_dir()
    dest.mkdir(parents=True, exist_ok=True)

    for fname in _HF_FILES:
        local = dest / fname
        if local.exists():
            continue
        url = f"{HF_BASE_URL}/{fname}"
        print(f"Downloading {fname} from {url} ...")
        tmp = local.with_suffix(".tmp")
        try:
            urllib.request.urlretrieve(url, tmp)
            tmp.replace(local)
        except Exception as e:
            tmp.unlink(missing_ok=True)
            raise UpscalerUnavailable(
                f"could not download {fname} from {url}: {e}\n"
                f"Install huggingface_hub (`pip install huggingface_hub`) for "
                f"better error handling, or use --sr-package to point at a "
                f"local copy."
            ) from e

    return dest


def _seeded(torch, fn, *a, **kw):
    """Run `fn` with the global RNG pinned, and put the RNG back afterwards.

    MambaIRv2's ASSM picks which of 128 prompt tokens each pixel belongs to with
    `F.gumbel_softmax(logits, hard=True)`, which samples Gumbel noise on *every
    call, including under eval and no_grad*. Left alone that makes the upscaler
    non-reproducible: three runs of one emoji differed by up to 12.45 levels per
    pixel. A tracer front end cannot have that -- the same PNG must give the same
    SVG -- and it would leave the Rust port with nothing to compare against.

    The obvious fix is the wrong one. Replacing the sample with its
    zero-temperature limit, a plain one-hot argmax, is deterministic but measured
    *worse*: dE00 0.5492 against 0.5364, worse on seven of eight images and
    outside the sampled range on all eight (`h29_routing_variance.py`). The
    weights were trained against the noisy routing, so the argmax route is off
    the distribution they were fitted to.

    Seeding keeps the trained distribution and still returns the same answer
    every time. The quality spread across draws is only +/- 0.001 dE00, so which
    draw is pinned does not matter -- only that one is.
    """
    state = torch.random.get_rng_state()
    cuda_states = torch.cuda.get_rng_state_all() if torch.cuda.is_available() else None
    try:
        torch.manual_seed(ROUTING_SEED)
        return fn(*a, **kw)
    finally:
        torch.random.set_rng_state(state)
        if cuda_states is not None:
            torch.cuda.set_rng_state_all(cuda_states)


@dataclass
class Upscaler:
    net: object
    scale: int
    device: object
    backend: str
    step: int | None = None
    source: str = ""
    #: Pin the routing RNG, so the same input gives the same output.
    deterministic: bool = True

    @property
    def label(self) -> str:
        return (f"MambaIRv2 x{self.scale} step {self.step} on {self.device} "
                f"[{self.backend}"
                f"{'' if self.deterministic else ', unseeded routing'}]")


def load(package: Path | None = None, device: str | None = None,
         deterministic: bool = True) -> Upscaler:
    """Load the upscaler, or explain why it cannot be loaded."""
    try:
        import torch
    except ImportError as e:  # noqa: PERF203
        raise UpscalerUnavailable(
            "PyTorch is not installed; the SR pre-pass needs it") from e

    if package is not None:
        # Explicit local override: check both the flat layout (HF-style) and the
        # legacy nested layout (out/sr/logo_sr_package/weights/...).
        pkg = Path(package)
        weights = pkg / "logo_sr_x4.pt"
        if not weights.exists():
            weights = pkg / "weights" / "logo_sr_x4.pt"
        if not weights.exists():
            raise UpscalerUnavailable(
                f"weights not found at {pkg}. Expected logo_sr_x4.pt or "
                f"weights/logo_sr_x4.pt inside the package directory.")
    else:
        pkg = _ensure_package()
        weights = pkg / "logo_sr_x4.pt"

    if device is None:
        # Only refuse when CPU was *inferred*. Asking for it is a valid choice.
        if not torch.cuda.is_available():
            raise UpscalerUnavailable(
                "no CUDA device. The scan runs on CPU but takes far longer per "
                "image; pass --device cpu explicitly if that is acceptable.")
        device = "cuda"

    from . import scan

    backend = scan.install()
    sys.path.insert(0, str(pkg))
    try:
        from mambairv2_arch import MambaIRv2  # noqa: PLC0415
    except Exception as e:  # noqa: BLE001
        raise UpscalerUnavailable(
            f"could not import MambaIRv2 from {pkg}: {e}") from e

    blob = torch.load(weights, map_location="cpu", weights_only=False)
    net = MambaIRv2(**blob["arch_kwargs"])
    missing, _ = net.load_state_dict(
        {k: v.float() for k, v in blob["state_dict"].items()}, strict=False)
    if missing:
        raise UpscalerUnavailable(
            f"checkpoint does not match architecture: {len(missing)} missing tensors")
    return Upscaler(net.to(device).eval(), int(blob["scale"]), device, backend,
                    blob.get("trained_step"), str(blob.get("source_run", "")),
                    deterministic)


def upscale(up: Upscaler, rgb: np.ndarray, tile: int = 256, overlap: int = 32,
            bf16: bool = True) -> np.ndarray:
    """rgb float HxWx3 in [0, 1] -> upscaled by `up.scale`, same range."""
    import torch

    with torch.no_grad():
        h, w, _ = rgb.shape
        s = up.scale
        pad = 16  # the model's window size; a partial window is not valid input
        out = np.zeros((h * s, w * s, 3), np.float32)
        acc = np.zeros((h * s, w * s, 1), np.float32)
        step = max(1, tile - overlap)
        ys = list(range(0, max(1, h - overlap), step)) or [0]
        xs = list(range(0, max(1, w - overlap), step)) or [0]
        for y in ys:
            for x in xs:
                y1, x1 = min(y + tile, h), min(x + tile, w)
                y0, x0 = max(0, y1 - tile), max(0, x1 - tile)
                patch = torch.from_numpy(
                    np.ascontiguousarray(rgb[y0:y1, x0:x1], np.float32)
                ).permute(2, 0, 1)[None].to(up.device)
                ph_, pw_ = patch.shape[-2:]
                py, px = (-ph_) % pad, (-pw_) % pad
                if py or px:
                    patch = torch.nn.functional.pad(patch, (0, px, 0, py),
                                                    mode="reflect")
                with torch.autocast("cuda", dtype=torch.bfloat16,
                                    enabled=bf16 and str(up.device) == "cuda"):
                    pred = (_seeded(torch, up.net, patch) if up.deterministic
                            else up.net(patch))
                pred = pred.float().clamp(0, 1)[..., : ph_ * s, : pw_ * s]
                pred = pred[0].permute(1, 2, 0).cpu().numpy()
                ph, pw = pred.shape[:2]
                # Feather, so a tile seam does not become a traced edge.
                wy = np.minimum(np.arange(ph), ph - 1 - np.arange(ph)) + 1.0
                wx = np.minimum(np.arange(pw), pw - 1 - np.arange(pw)) + 1.0
                wgt = np.minimum(wy[:, None], wx[None, :])[..., None].astype(np.float32)
                out[y0 * s:y0 * s + ph, x0 * s:x0 * s + pw] += pred * wgt
                acc[y0 * s:y0 * s + ph, x0 * s:x0 * s + pw] += wgt
        return out / np.maximum(acc, 1e-6)


def upscale_rgba(up: Upscaler, rgba: np.ndarray, **kw) -> np.ndarray:
    """HxWx4 in [0, 1] -> upscaled by `up.scale`, alpha carried at Lanczos."""
    from PIL import Image

    alpha, rgb = rgba[..., 3:4], rgba[..., :3]
    # The model sees colour, not colour x alpha, so a fully transparent pixel
    # does not drag its neighbours dark.
    rgb = np.where(alpha > 1e-4, rgb, 0.0)
    hi = upscale(up, rgb, **kw)
    s = up.scale
    a_up = Image.fromarray((alpha[..., 0] * 255).astype(np.uint8)).resize(
        (alpha.shape[1] * s, alpha.shape[0] * s), Image.LANCZOS)
    return np.dstack([np.clip(hi, 0, 1),
                      np.asarray(a_up, np.float32)[..., None] / 255.0])
