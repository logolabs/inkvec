"""Probe the two behavior paths that changed between 0.1.1 and 0.1.2.

The paired screen-set compare showed the two builds are byte-identical on clean
input, so any real quality delta must live where the pipeline reordered:

  A. large input  -- 0.1.1 silently ignored --max-dim when the restore probe
     early-returned; 0.1.2 caps at decode time (2048) and retargets the display
     size. Probe: trace a 4096-px render of the GT with both builds, plus the
     new build uncapped (--max-dim 0) to isolate the cap from everything else.
  B. restored input -- 0.1.2 runs cap/intake-norm/oversample detection BEFORE
     the restorer, where 0.1.1 ran it after. Probe: JPEG q50 the intake, trace
     with --restore auto on both, judge against the same reference.
"""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

import numpy as np  # noqa: E402
from PIL import Image  # noqa: E402

from inkvec_bench import render  # noqa: E402
from inkvec_bench.metrics import color as mcolor  # noqa: E402
import svgeval  # noqa: E402

# Restore-capable builds (built with --features restore-model; the plain builds here
# cannot restore and would hard-error on probe B). 0.1.1 links ORT dynamically, so
# onnxruntime.dll sits next to each exe.
OLD = ROOT / "out" / "src-011" / "target-restore" / "release" / "inkvec.exe"
NEW = ROOT / "target-restore-cli" / "release" / "inkvec.exe"
WORK = ROOT / "out" / "probe"
J = 1024
STEMS = ["cat", "rule_folder", "E30B", "emoji_u1f36a"]


def run(exe: Path, png: Path, out: Path, extra: list[str]) -> None:
    r = subprocess.run([str(exe), str(png), "-o", str(out), "--quiet", *extra],
                       capture_output=True, timeout=900)
    if r.returncode != 0:
        raise RuntimeError(f"{exe.name} failed on {png.name}: "
                           f"{r.stderr.decode(errors='replace')[:400]}")


def dE(svg_path: Path, ref: np.ndarray) -> float:
    b = render.composite(render.render(svg_path.read_text(encoding="utf-8"), J, J))
    return float(mcolor.delta_e00(ref, b)["de00_mean"])


def main() -> None:
    WORK.mkdir(parents=True, exist_ok=True)
    items = {it["stem"]: it for it in svgeval.load_sets()["full"]}
    stems = [s for s in STEMS if s in items]
    print(f"probing {len(stems)} stems: {stems}")

    print("\n== A. 4096-px input (cap path) ==")
    print(f"{'stem':18s} {'0.1.1':>7s} {'0.1.2':>7s} {'0.1.2 nocap':>11s}")
    for stem in stems:
        it = items[stem]
        _, gt = svgeval.item_paths(it)
        gt_text = gt.read_text(encoding="utf-8")
        ref = render.composite(render.render(gt_text, J, J))
        big = render.render(gt_text, 4096, 4096)
        png = WORK / f"{stem}_4096.png"
        Image.fromarray((np.clip(big, 0, 1) * 255).astype(np.uint8)).save(png)

        row = []
        for label, exe, extra in [("old", OLD, []), ("new", NEW, []),
                                  ("nocap", NEW, ["--max-dim", "0"])]:
            out = WORK / f"{stem}_A_{label}.svg"
            run(exe, png, out, extra)
            row.append(dE(out, ref))
        print(f"{stem:18s} {row[0]:7.4f} {row[1]:7.4f} {row[2]:11.4f}")

    print("\n== B. JPEG q50 + --restore auto (reorder path) ==")
    print(f"{'stem':18s} {'0.1.1':>7s} {'0.1.2':>7s}")
    for stem in stems:
        it = items[stem]
        png, gt = svgeval.item_paths(it)
        ref = render.composite(render.render(gt.read_text(encoding="utf-8"), J, J))
        jpg = WORK / f"{stem}_q50.jpg"
        # Composite on WHITE: a plain convert("RGB") turns transparent pixels black.
        # (Superseded by bench/probe_restore.py, which also records the restore notes.)
        src = Image.open(png).convert("RGBA")
        bg = Image.new("RGBA", src.size, (255, 255, 255, 255))
        Image.alpha_composite(bg, src).convert("RGB").save(jpg, quality=50)

        row = []
        for label, exe in [("old", OLD), ("new", NEW)]:
            out = WORK / f"{stem}_B_{label}.svg"
            run(exe, jpg, out, ["--restore", "auto"])
            row.append(dE(out, ref))
        print(f"{stem:18s} {row[0]:7.4f} {row[1]:7.4f}")


if __name__ == "__main__":
    main()
