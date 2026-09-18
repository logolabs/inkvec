"""Prove two builds write the same bytes, and say how much faster the second is.

    python bench/identical.py <before.exe> <after.exe> [--n 40] [--args " --cutout"]

An optimisation that changes the output is a different change, judged by the corpus. This
is for the ones that must not: same SVG, byte for byte, in less time.
"""
from __future__ import annotations

import argparse
import hashlib
import random
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def trace(exe: str, png: Path, out: Path, extra: list[str]) -> tuple[str, float]:
    t = time.perf_counter()
    r = subprocess.run([exe, str(png), "-o", str(out), "-q", *extra], capture_output=True)
    dt = time.perf_counter() - t
    if r.returncode != 0 or not out.exists():
        return "", dt
    return hashlib.sha1(out.read_bytes()).hexdigest(), dt


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("before")
    ap.add_argument("after")
    ap.add_argument("--n", type=int, default=8, help="icons per family")
    ap.add_argument("--args", default="")
    ap.add_argument("--families", default="lucide,material-icons,simple-icons,twemoji,noto-emoji")
    ap.add_argument("--extra-images", default="gradio_app/samples")
    a = ap.parse_args()

    extra = a.args.split()
    work = Path(tempfile.mkdtemp(prefix="identical_"))
    random.seed(11)
    pngs: list[Path] = []
    for fam in a.families.split(","):
        d = ROOT / "bench/data/corpus_raster" / fam / "128ss"
        if d.is_dir():
            pool = sorted(d.glob("*.png"))
            pngs += random.sample(pool, min(a.n, len(pool)))
    if a.extra_images:
        pngs += sorted((ROOT / a.extra_images).glob("*.png"))

    same = differ = failed = 0
    tb = ta = 0.0
    for png in pngs:
        hb, db = trace(a.before, png, work / "b.svg", extra)
        ha, da = trace(a.after, png, work / "a.svg", extra)
        tb += db
        ta += da
        if not hb or not ha:
            failed += 1
        elif hb == ha:
            same += 1
        else:
            differ += 1
            if differ <= 5:
                print(f"  differs: {png.parent.parent.name}/{png.stem}")
    n = len(pngs)
    print(f"\n{n} images: {same} identical, {differ} different, {failed} failed")
    print(f"before {tb:6.2f} s   after {ta:6.2f} s   {tb / max(ta, 1e-9):.2f}x")
    sys.exit(1 if differ or failed else 0)


if __name__ == "__main__":
    main()
