"""Probe B, fixed: does the 0.1.1 -> 0.1.2 pipeline reorder change quality on
DEGRADED (JPEG) input when both binaries can actually restore?

0.1.2 runs cap / intake-normalisation / oversample detection BEFORE the restorer;
0.1.1 ran them after. Trace each stem's JPEG-q50 intake with --restore auto on
both restore-capable builds and judge at 1024 against the GT SVG.

The old bench/probe_paths.py probe B composited transparent pixels as BLACK when
making the JPEG (PIL convert("RGB") on RGBA); here we composite on WHITE first.

Both exes print their restore decision on stderr without --quiet, so we run with
stats on and record the note lines per run.
"""
from __future__ import annotations

import re
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

import numpy as np  # noqa: E402
from PIL import Image  # noqa: E402

from inkvec_bench import render  # noqa: E402
from inkvec_bench.metrics import color as mcolor  # noqa: E402
import svgeval  # noqa: E402

OLD = ROOT / "out" / "src-011" / "target-restore" / "release" / "inkvec.exe"
NEW = ROOT / "target-restore-cli" / "release" / "inkvec.exe"
WORK = ROOT / "out" / "probe_restore"
J = 1024
STEMS = ["cat", "rule_folder", "E30B", "emoji_u1f36a", "1f338", "appium"]
NOTE = re.compile(r"^  (restore|sr|intake)\b.*$", re.M)


def white_jpeg(png: Path, jpg: Path) -> None:
    src = Image.open(png).convert("RGBA")
    bg = Image.new("RGBA", src.size, (255, 255, 255, 255))
    Image.alpha_composite(bg, src).convert("RGB").save(jpg, quality=50)


def run(label: str, exe: Path, jpg: Path, out: Path) -> tuple[float, str]:
    t0 = time.time()
    r = subprocess.run([str(exe), str(jpg), "-o", str(out), "--restore", "auto"],
                       capture_output=True, timeout=900)
    dt = time.time() - t0
    if r.returncode != 0:
        raise RuntimeError(f"{label} failed on {jpg.name} "
                           f"(rc={r.returncode}): {r.stderr.decode(errors='replace')[:400]}")
    notes = NOTE.findall(r.stderr.decode(errors="replace"))
    return dt, " | ".join(n.strip() for n in notes) if notes else "(no notes)"


def dE(svg_path: Path, ref: np.ndarray) -> float:
    b = render.composite(render.render(svg_path.read_text(encoding="utf-8"), J, J))
    return float(mcolor.delta_e00(ref, b)["de00_mean"])


def main() -> None:
    WORK.mkdir(parents=True, exist_ok=True)
    for exe in (OLD, NEW):
        if not exe.exists():
            raise SystemExit(f"missing build: {exe}")
    items = {it["stem"]: it for it in svgeval.load_sets()["full"]}
    stems = [s for s in STEMS if s in items]
    print(f"probing {len(stems)} stems: {stems}", flush=True)

    results: dict[str, tuple[float, float]] = {}
    for stem in stems:
        it = items[stem]
        png, gt = svgeval.item_paths(it)
        ref = render.composite(render.render(gt.read_text(encoding="utf-8"), J, J))
        jpg = WORK / f"{stem}_q50.jpg"
        white_jpeg(png, jpg)

        row: list[float] = []
        notes: list[str] = []
        for label, exe in (("0.1.1", OLD), ("0.1.2", NEW)):
            out = WORK / f"{stem}_{label.replace('.', '')}.svg"
            dt, note = run(label, exe, jpg, out)
            row.append(dE(out, ref))
            notes.append(f"    {label} {dt:6.1f}s  {note}")
        results[stem] = (row[0], row[1])
        print(f"{stem:16s} 0.1.1 {row[0]:7.4f}  0.1.2 {row[1]:7.4f}  "
              f"delta {row[1] - row[0]:+.4f}", flush=True)
        print("\n".join(notes), flush=True)

    print(f"\n{'stem':16s} {'0.1.1':>8s} {'0.1.2':>8s} {'delta':>8s}")
    d0 = d1 = 0.0
    for stem, (a, b) in results.items():
        print(f"{stem:16s} {a:8.4f} {b:8.4f} {b - a:+8.4f}")
        d0 += a
        d1 += b
    n = max(len(results), 1)
    print(f"{'mean':16s} {d0 / n:8.4f} {d1 / n:8.4f} {(d1 - d0) / n:+8.4f}")


if __name__ == "__main__":
    main()
