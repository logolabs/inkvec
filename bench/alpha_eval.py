"""Score a trace on grounds other than white, which is the only place transparency shows.

    python bench/alpha_eval.py <exe> [--args " --cutout"] [--set screen] [--n 60]

`full_eval.py` composites everything over white before it measures, so a transparent region
that came back painted white scores perfectly and a hole scores the same. Every alpha
decision the tracer makes is therefore invisible to it — which is why `--cutout` is off by
default despite being the correct output.

This scores the same trace three ways:

  white   what full_eval sees
  dark    the same image over #141210, where paint that should be a hole shows up
  alpha   mean absolute difference of the alpha channels themselves

The artist's own file is the reference in all three. A trace that agrees with it on white
and disagrees on dark is painting where the artist left the page showing.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

from inkvec_bench import render  # noqa: E402

SIZE = 320
DARK = np.array([0.078, 0.071, 0.063], np.float32)  # the design system's --ll-bg-abyss


def rgba(svg: str) -> np.ndarray:
    """Render to straight RGBA in [0, 1]."""
    a = render.render(svg, SIZE, SIZE)
    if a.shape[-1] == 3:
        return np.concatenate([a, np.ones(a.shape[:2] + (1,), np.float32)], axis=2)
    return a


def over(img: np.ndarray, ground: np.ndarray) -> np.ndarray:
    al = img[..., 3:4]
    return np.clip(img[..., :3] * al + ground * (1.0 - al), 0.0, 1.0)


def score(trace: np.ndarray, truth: np.ndarray) -> dict[str, float]:
    white = np.ones(3, np.float32)
    return {
        "white": float(np.abs(over(trace, white) - over(truth, white)).mean()),
        "dark": float(np.abs(over(trace, DARK) - over(truth, DARK)).mean()),
        "alpha": float(np.abs(trace[..., 3] - truth[..., 3]).mean()),
    }


def alpha_content(truth: np.ndarray) -> float:
    """How much of this artwork is neither fully opaque nor fully clear — the part a
    white-only metric cannot see."""
    a = truth[..., 3]
    return float(((a > 0.02) & (a < 0.98)).mean())


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("exe")
    ap.add_argument("--args", default="", help="extra tracer arguments, e.g. ' --cutout'")
    ap.add_argument("--corpus", default="bench/data/corpus_raster")
    ap.add_argument("--truth", default="bench/data/corpus_svg")
    ap.add_argument("--tier", default="128ss")
    ap.add_argument("--n", type=int, default=60, help="icons per family")
    ap.add_argument("--families", default="lucide,material-icons,simple-icons,twemoji,noto-emoji")
    ap.add_argument("--json", default="")
    a = ap.parse_args()

    work = Path(tempfile.mkdtemp(prefix="alpha_eval_"))
    extra = a.args.split()
    rows = []
    for fam in a.families.split(","):
        pngs = sorted((ROOT / a.corpus / fam / a.tier).glob("*.png"))[: a.n]
        for png in pngs:
            gt = ROOT / a.truth / fam / f"{png.stem}.svg"
            if not gt.exists():
                continue
            out = work / f"{fam}_{png.stem}.svg"
            r = subprocess.run([a.exe, str(png), "-o", str(out), "-q", *extra],
                               capture_output=True, text=True)
            if r.returncode != 0 or not out.exists():
                continue
            truth = rgba(gt.read_text(encoding="utf-8"))
            trace = rgba(out.read_text(encoding="utf-8"))
            row = score(trace, truth)
            row.update(name=f"{fam}/{png.stem}", family=fam, translucent=alpha_content(truth))
            rows.append(row)

    if not rows:
        sys.exit("nothing scored")
    print(f"{'family':16s} {'n':>4s} {'white':>9s} {'dark':>9s} {'alpha':>9s} {'translucent':>12s}")
    for fam in dict.fromkeys(r["family"] for r in rows):
        f = [r for r in rows if r["family"] == fam]
        print(f"{fam:16s} {len(f):4d} {np.mean([r['white'] for r in f]):9.5f} "
              f"{np.mean([r['dark'] for r in f]):9.5f} {np.mean([r['alpha'] for r in f]):9.5f} "
              f"{np.mean([r['translucent'] for r in f]) * 100:11.1f}%")
    print(f"{'ALL':16s} {len(rows):4d} {np.mean([r['white'] for r in rows]):9.5f} "
          f"{np.mean([r['dark'] for r in rows]):9.5f} {np.mean([r['alpha'] for r in rows]):9.5f} "
          f"{np.mean([r['translucent'] for r in rows]) * 100:11.1f}%")
    worst = sorted(rows, key=lambda r: -(r["dark"] - r["white"]))[:10]
    print("\nlargest gap between the dark ground and white (paint where the page should show):")
    for r in worst:
        print(f"  {r['name']:44s} white {r['white']:.5f}  dark {r['dark']:.5f}  alpha {r['alpha']:.5f}")
    if a.json:
        Path(a.json).write_text(json.dumps(rows, indent=1), encoding="utf-8")


if __name__ == "__main__":
    main()
