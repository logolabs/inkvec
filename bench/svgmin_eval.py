"""Does `inkvec-svgmin` keep the picture while shrinking the file?

    python bench/svgmin_eval.py [--n 40] [--set screen] [--exe target/release/inkvec-svgmin.exe]
                                [--args "--tolerance 0.1"] [--ours]

For each corpus icon: minify the artist's SVG, render both at the judge size, and score the
rewrite against the original with dE00 (the gate's colour metric) and the largest pixel
difference. Report the description-length saving beside the fidelity cost, per family.

With `--ours`, do the same to the tracer's own output instead of the artist's file: how
much redundancy the emitter leaves behind.
"""
from __future__ import annotations

import argparse
import re
import shlex
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

import svgeval  # noqa: E402
from inkvec_bench import render  # noqa: E402
from inkvec_bench.metrics import color as mcolor  # noqa: E402

JUDGE = svgeval.JUDGE_SIZE


def rgb(svg: str) -> np.ndarray:
    img = render.render(svg, JUDGE, JUDGE)
    if img.shape[-1] == 4:
        img = img[..., :3] * img[..., 3:] + (1.0 - img[..., 3:])
    return img


def tool_counts(stderr: str) -> tuple[int, int, float, float]:
    """Segments and description length before and after, from the tool's own `--stats`
    line: it parses the path data properly, where counting command letters here would
    miss SVG's implicit command repetition (one `c` followed by twenty curves)."""
    m = re.search(r"segments (\d+) -> (\d+), params (\d+) -> (\d+)", stderr)
    if not m:
        raise RuntimeError(f"no stats in: {stderr[:200]}")
    return int(m.group(1)), int(m.group(2)), float(m.group(3)), float(m.group(4))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(ROOT / "target/release/inkvec-svgmin.exe"))
    ap.add_argument("--tracer", default=str(ROOT / "target/release/inkvec.exe"))
    ap.add_argument("--set", default="screen")
    ap.add_argument("--n", type=int, default=40)
    ap.add_argument("--args", default="")
    ap.add_argument("--ours", action="store_true", help="minify the tracer's output, not the artist's file")
    a = ap.parse_args()

    items = svgeval.load_sets()[a.set]
    step = max(1, len(items) // a.n)
    items = items[::step][: a.n]
    tmp = Path(tempfile.mkdtemp())
    extra = shlex.split(a.args)
    rows = []
    print(f"{'icon':38} {'segs':>9} {'params':>12} {'saved':>6} {'dE00':>7} {'maxpx':>6} {'ms':>5}")
    for it in items:
        png, gt = svgeval.item_paths(it)
        if a.ours:
            src_path = tmp / f"{it['stem']}.traced.svg"
            subprocess.run([a.tracer, str(png), "-o", str(src_path), "-q"], check=True)
        else:
            src_path = gt
        out_path = tmp / f"{it['stem']}.min.svg"
        t0 = time.perf_counter()
        p = subprocess.run([a.exe, str(src_path), "-o", str(out_path), "--stats", *extra],
                           capture_output=True, text=True)
        ms = (time.perf_counter() - t0) * 1e3
        if p.returncode != 0:
            print(f"{it['corpus'] + '/' + it['stem']:38} FAILED: {p.stderr.strip()[:80]}")
            continue
        src = src_path.read_text(encoding="utf-8")
        out = out_path.read_text(encoding="utf-8")
        s0, s1, p0, p1 = tool_counts(p.stderr)
        ref, img = rgb(src), rgb(out)
        de = float(mcolor.delta_e00(ref, img)["de00_mean"])
        maxpx = float(np.abs(ref - img).max() * 255.0)
        mse = float(np.mean((ref - img) ** 2))
        psnr = 10.0 * np.log10(1.0 / mse) if mse > 0 else 99.0
        saved = 100.0 * (1.0 - p1 / p0) if p0 else 0.0
        b0, b1 = len(src.encode("utf-8")), len(out.encode("utf-8"))
        rows.append(dict(corpus=it["corpus"], stem=it["stem"], s0=s0, s1=s1, p0=p0, p1=p1,
                         saved=saved, de=de, maxpx=maxpx, psnr=psnr, b0=b0, b1=b1, ms=ms))
        print(f"{it['corpus'] + '/' + it['stem']:38} {s0:4}->{s1:<4} {p0:5.0f}->{p1:<5.0f} {saved:5.1f}% {de:7.4f} {maxpx:6.1f} {psnr:5.1f}dB {ms:5.0f}")

    if not rows:
        return 1
    print()
    fams = sorted({r["corpus"] for r in rows})
    print(f"{'family':16} {'n':>3} {'params saved':>13} {'bytes saved':>12} {'mean dE00':>10} {'worst dE00':>11} "
          f"{'PSNR mean':>10} {'PSNR min':>9} {'ms':>5}")
    for fam in fams + ["ALL"]:
        rs = [r for r in rows if fam == "ALL" or r["corpus"] == fam]
        p0, p1 = sum(r["p0"] for r in rs), sum(r["p1"] for r in rs)
        b0, b1 = sum(r["b0"] for r in rs), sum(r["b1"] for r in rs)
        print(f"{fam:16} {len(rs):3} {100 * (1 - p1 / max(p0, 1)):12.1f}% {100 * (1 - b1 / max(b0, 1)):11.1f}% "
              f"{np.mean([r['de'] for r in rs]):10.4f} {max(r['de'] for r in rs):11.4f} "
              f"{np.mean([r['psnr'] for r in rs]):10.1f} {min(r['psnr'] for r in rs):9.1f} {np.mean([r['ms'] for r in rs]):5.0f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
