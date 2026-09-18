"""Paired DinoScore comparison between our engine and VTracer.

Standalone rather than folded into the main sweep because the question it answers is
narrow: for one fixed operating point per engine, on two real corpora, how do the two
compare *per image*. Medians hide that — two engines can sit at the same median while
one wins 80% of the images, and for a metric as saturated as DinoScore (everything
credible scores above 0.9) the paired count is far more informative than the centre of
the distribution.

Both backbones are scored in one pass. DINOv3 is the headline; DINOv2 is included only
because the published SVG-Bench table used it, and a DINOv3 number cannot be placed
next to a DINOv2 one.

Usage::

    python dino_compare.py [--limit N] [--corpora noto-emoji,twemoji]
"""
from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))

from inkvec_bench import render  # noqa: E402
from inkvec_bench.metrics import dino as m_dino  # noqa: E402

HERE = Path(__file__).resolve().parent
INKVEC_EXE = Path(r"M:\AI STORAGE\SVGIfication\target4\release\inkvec.exe")

# Our best measured operating point.
INKVEC_ARGS = ["--precision", "0.6", "--quiet"]

# VTracer's best-fidelity setting as measured in this harness.
VTRACER_CFG = dict(
    colormode="color", mode="spline", hierarchical="stacked", path_precision=3,
    corner_threshold=60, length_threshold=4.0, splice_threshold=45,
    color_precision=8, layer_difference=8, filter_speckle=2,
)

PX = 128
BATCH = 32


def trace_inkvec(png: Path, tmp: Path) -> str:
    out = tmp / (png.stem + ".ours.svg")
    r = subprocess.run(
        [str(INKVEC_EXE), str(png), "-o", str(out), *INKVEC_ARGS],
        capture_output=True, text=True, timeout=300,
    )
    if r.returncode != 0 or not out.exists():
        raise RuntimeError(f"inkvec rc={r.returncode}: {(r.stderr or r.stdout)[:200]}")
    return out.read_text(encoding="utf-8")


def trace_vtracer(png: Path, tmp: Path) -> str:
    import vtracer

    return vtracer.convert_raw_image_to_svg(png.read_bytes(), img_format="png", **VTRACER_CFG)


ENGINES = {"ours": trace_inkvec, "vtracer": trace_vtracer}


def embed_all(images: list[np.ndarray], backbone: str) -> np.ndarray | None:
    """Batched embedding — one forward pass per BATCH images rather than per image."""
    chunks = []
    for i in range(0, len(images), BATCH):
        f = m_dino.embed(images[i:i + BATCH], backbone=backbone)
        if f is None:
            return None
        chunks.append(f)
    return np.concatenate(chunks) if chunks else None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpora", default="noto-emoji,twemoji")
    ap.add_argument("--limit", type=int, default=0, help="cap images per corpus (0 = all)")
    ap.add_argument("--backbones", default="dinov3,dinov2")
    args = ap.parse_args()

    backbones = [b for b in args.backbones.split(",") if b]
    for b in backbones:
        err = m_dino.load_error(b)
        print(f"backbone {b:8s} {'OK' if not err else 'FAILED: ' + err}")
    backbones = [b for b in backbones if m_dino.available(b)]
    if not backbones:
        print("no DINO backbone available; nothing to measure", file=sys.stderr)
        return 1

    tmp = Path(tempfile.mkdtemp(prefix="dinocmp-"))
    results: dict[tuple[str, str, str], dict[str, float]] = {}

    for corpus in args.corpora.split(","):
        raster = HERE / "data" / "corpus_raster" / corpus / str(PX)
        pngs = sorted(raster.glob("*.png"))
        if args.limit:
            pngs = pngs[:args.limit]
        print(f"\n=== {corpus}: {len(pngs)} images ===")

        refs: list[np.ndarray] = []
        cands: dict[str, list[np.ndarray]] = {e: [] for e in ENGINES}
        names: list[str] = []
        failures: dict[str, int] = {e: 0 for e in ENGINES}

        for png in pngs:
            try:
                ref = render.composite(render.load_rgba(png))
            except Exception as e:
                print(f"  skip {png.name}: reference load failed: {e}")
                continue
            traced: dict[str, np.ndarray] = {}
            for name, fn in ENGINES.items():
                try:
                    svg = fn(png, tmp)
                    traced[name] = render.composite(render.render(svg, PX, PX))
                except Exception as e:
                    failures[name] += 1
                    print(f"  {name} failed on {png.name}: {type(e).__name__}: {e}")
                    break
            # Paired comparison only means anything when both engines produced a
            # result for the same image, so drop the image entirely otherwise.
            if len(traced) != len(ENGINES):
                continue
            refs.append(ref)
            names.append(png.stem)
            for name, img in traced.items():
                cands[name].append(img)

        print(f"  paired images: {len(refs)}  failures: {failures}")
        if not refs:
            continue

        for backbone in backbones:
            rf = embed_all(refs, backbone)
            scores = {}
            for name in ENGINES:
                cf = embed_all(cands[name], backbone)
                scores[name] = np.sum(rf * cf, axis=1).clip(-1, 1)
                results[(corpus, name, backbone)] = {
                    "median": float(np.median(scores[name])),
                    "mean": float(np.mean(scores[name])),
                    "p10": float(np.percentile(scores[name], 10)),
                    "min": float(np.min(scores[name])),
                    "n": len(refs),
                }
            a, b = scores["ours"], scores["vtracer"]
            d = a - b
            ties = int(np.sum(np.abs(d) < 1e-6))
            results[(corpus, "paired", backbone)] = {
                "ours_wins": int(np.sum(d > 1e-6)),
                "vtracer_wins": int(np.sum(d < -1e-6)),
                "ties": ties,
                "n": len(refs),
                "mean_delta": float(np.mean(d)),
                "median_delta": float(np.median(d)),
            }
            print(f"  [{backbone}] ours median {np.median(a):.4f} | "
                  f"vtracer median {np.median(b):.4f} | "
                  f"ours wins {int(np.sum(d > 1e-6))}/{len(refs)}")

    print("\n\n================ SUMMARY ================")
    for backbone in backbones:
        print(f"\n--- {backbone} ---")
        print(f"{'corpus':12s} {'engine':9s} {'n':>4s} {'median':>8s} {'mean':>8s} "
              f"{'p10':>8s} {'min':>8s}")
        for (corpus, name, bb), v in results.items():
            if bb != backbone or name == "paired":
                continue
            print(f"{corpus:12s} {name:9s} {v['n']:4d} {v['median']:8.4f} "
                  f"{v['mean']:8.4f} {v['p10']:8.4f} {v['min']:8.4f}")
        print("paired (per-image, ours vs vtracer):")
        for (corpus, name, bb), v in results.items():
            if bb != backbone or name != "paired":
                continue
            print(f"  {corpus:12s} ours {v['ours_wins']:3d} / vtracer {v['vtracer_wins']:3d}"
                  f" / ties {v['ties']:3d}  of {v['n']}   "
                  f"mean delta {v['mean_delta']:+.4f}  median delta {v['median_delta']:+.4f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
