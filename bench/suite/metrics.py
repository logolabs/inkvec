"""What to measure, and why each one is here.

Two families, because the literature and a production decision want different things.

RESEARCH-STANDARD, so that a number here can be put beside a published one:

    MSE, SSIM, LPIPS(vgg), DINOScore   the set SVG-Bench established and SVGenius and
                                      VectorArk report. DINOScore is the one to trust when
                                      they disagree: pixel metrics on vector art are
                                      dominated by flat background and punish a sub-pixel
                                      offset far more than a viewer would, which is the
                                      stated reason SVG-Bench introduced it.

PRODUCTION, which no paper reports and every deployment needs:

    dE00            CIEDE2000 against the artist's own file. Stricter than any of the
                    above and the metric this project has always optimised.
    params ratio    numbers written against numbers the artist wrote. An engine can buy
                    fidelity with shapes for ever, and only this notices.
    bytes           what actually ships.
    seconds         p50 and p95 and the worst case, because a tracer with a five second
                    budget is judged by its tail and not its mean.
    valid           does the output parse, and does it draw anything.

Everything is computed from one pair of renders so that a metric cannot disagree with its
neighbour about what it is looking at.
"""
from __future__ import annotations

import functools
import sys
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))
from inkvec_bench import render, svgmodel  # noqa: E402
from inkvec_bench.metrics import color as mcolor  # noqa: E402


@dataclass
class Scores:
    de00: float = float("nan")
    mse: float = float("nan")
    ssim: float = float("nan")
    lpips: float = float("nan")
    dino: float = float("nan")
    params_ratio: float = float("nan")
    n_params: int = 0
    bytes: int = 0
    seconds: float = float("nan")
    valid: bool = False
    extra: dict = field(default_factory=dict)


@functools.lru_cache(maxsize=1)
def _torch_models():
    """LPIPS and DINOv2, loaded once. Both are optional: without them the research-standard
    columns are reported as missing rather than silently skipped."""
    try:
        import torch
        import lpips
        from transformers import AutoImageProcessor, AutoModel

        dev = "cuda" if torch.cuda.is_available() else "cpu"
        lp = lpips.LPIPS(net="vgg").to(dev).eval()
        dn = AutoModel.from_pretrained("facebook/dinov2-base").to(dev).eval()
        pr = AutoImageProcessor.from_pretrained("facebook/dinov2-base")
        return torch, dev, lp, dn, pr
    except Exception as e:  # noqa: BLE001
        print(f"  [metrics] LPIPS/DINO unavailable, those columns will be blank: {e}")
        return None


def render_white(svg: str, size: int) -> np.ndarray | None:
    """Render and composite over white, which is what every benchmark here assumes."""
    try:
        a = render.render(svg, size, size)
    except Exception:  # noqa: BLE001
        return None
    al = a[..., 3:4]
    return np.clip(a[..., :3] * al + (1.0 - al), 0, 1).astype(np.float32)


def score(gt_svg: str, cand_svg: str, seconds: float, judge: int = 512) -> Scores:
    s = Scores(seconds=seconds, bytes=len(cand_svg.encode()))
    ref = render_white(gt_svg, judge)
    got = render_white(cand_svg, judge)
    if ref is None or got is None:
        return s
    # An engine that emits a blank page should not score well on a mostly-blank corpus.
    if float(got.std()) < 1e-6 and float(ref.std()) > 1e-6:
        s.extra["blank"] = True
        return s
    s.valid = True
    s.de00 = float(mcolor.delta_e00(ref, got)["de00_mean"])
    s.mse = float(((ref - got) ** 2).mean())
    try:
        from skimage.metrics import structural_similarity

        s.ssim = float(structural_similarity(ref, got, channel_axis=2, data_range=1.0))
    except Exception:  # noqa: BLE001
        pass
    try:
        s.n_params = svgmodel.parse(cand_svg).n_params
        gt_n = svgmodel.parse(gt_svg).n_params
        s.params_ratio = s.n_params / max(1, gt_n)
    except Exception:  # noqa: BLE001
        pass

    tm = _torch_models()
    if tm is not None:
        torch, dev, lp, dn, pr = tm
        from PIL import Image

        a = torch.from_numpy(ref).permute(2, 0, 1)[None].to(dev)
        b = torch.from_numpy(got).permute(2, 0, 1)[None].to(dev)
        mean = torch.tensor([0.485, 0.456, 0.406], device=dev).view(1, 3, 1, 1)
        std = torch.tensor([0.229, 0.224, 0.225], device=dev).view(1, 3, 1, 1)
        with torch.no_grad():
            # SVGenius normalises with ImageNet statistics before LPIPS; matching their
            # harness matters more here than matching the LPIPS README.
            s.lpips = float(lp((a - mean) / std, (b - mean) / std).item())
            pi = pr(images=[Image.fromarray((ref * 255).astype(np.uint8)),
                            Image.fromarray((got * 255).astype(np.uint8))],
                    return_tensors="pt").to(dev)
            f = dn(**pi).last_hidden_state[:, 0]
            f = torch.nn.functional.normalize(f, dim=-1)
            s.dino = float((f[0] @ f[1]).item())
    return s


def aggregate(rows: list[Scores]) -> dict:
    """Means for the quality columns, and the tail for the one that is a budget."""
    ok = [r for r in rows if r.valid]
    if not ok:
        return {"n": 0, "n_valid": 0, "n_failed": len(rows)}
    sec = np.array([r.seconds for r in rows if np.isfinite(r.seconds)])

    def m(attr: str) -> float:
        v = np.array([getattr(r, attr) for r in ok], float)
        v = v[np.isfinite(v)]
        return float(v.mean()) if v.size else float("nan")

    return {
        "n": len(rows),
        "n_valid": len(ok),
        "n_failed": len(rows) - len(ok),
        "de00": m("de00"),
        "mse": m("mse"),
        "ssim": m("ssim"),
        "lpips": m("lpips"),
        "dino": m("dino"),
        "params_ratio": m("params_ratio"),
        "bytes": m("bytes"),
        "sec_p50": float(np.percentile(sec, 50)) if sec.size else float("nan"),
        "sec_p95": float(np.percentile(sec, 95)) if sec.size else float("nan"),
        "sec_max": float(sec.max()) if sec.size else float("nan"),
        "over_5s": int((sec > 5.0).sum()) if sec.size else 0,
    }
