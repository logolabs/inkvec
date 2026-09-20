"""The minifier's regression gate.

    python bench/svgmin_gate.py --exe target/release/inkvec-svgmin
    python bench/svgmin_gate.py --exe ... --write-baseline   # after a deliberate change
    python bench/svgmin_gate.py --exe ... --bypass-gate "Strong justification, approved"

This is **not** shaped like `bench/ci_gate.py`, and the difference is the point.

The tracer's gate ratchets colour error downwards: for a tracer, less error is strictly
better, so ground gained is never given back. The minifier is the other kind of tool. Its
whole job is to *spend* fidelity, up to the tolerance it promises, to buy description
length. A downward ratchet on dE00 would fail the build every time the minifier got
better at what it is for.

So the two numbers that are in tension are gated against each other, not each on its own:

* **bytes saved** may not fall by more than `SAVINGS_SLACK`, ever;
* **mean dE00** may not rise by more than `FIDELITY_SLACK` *unless* bytes saved rose by
  at least `SAVINGS_SLACK` -- fidelity may be traded for size, but only at a price;
* and a run that improves one without spending the other ratchets the baseline.

Two things are not traded at all, because they are promises rather than measurements:

* **`--bytes-only` must not move the picture.** That path removes no segment and moves no
  point; it only spells the numbers more cheaply. If it renders even one pixel
  differently, something is silently redrawing the file and no byte count excuses it.
* **worst dE00** must stay under `WORST_CEILING`, a backstop well below the tracer's own
  error against these same files (0.148). The real contract is geometric -- no point moves
  more than three times the tolerance -- and that is enforced per segment inside the
  fitter; this is the outside check that it still holds.
"""
from __future__ import annotations

import argparse
import json
import platform
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

import svgeval  # noqa: E402
from inkvec_bench import render  # noqa: E402
from inkvec_bench.metrics import color as mcolor  # noqa: E402

BASELINE = ROOT / "bench" / "gate" / "svgmin.json"

# A percentage point of the description, and a tenth of the colour error.
SAVINGS_SLACK = 0.01
FIDELITY_SLACK = 0.10
# Four times below the error the tracer itself makes against these files.
WORST_CEILING = 0.037
# One step of an 8-bit channel: below this, nothing moved.
BYTES_ONLY_MAX_PX = 1.0


def rgb(svg: str) -> np.ndarray:
    """The picture as straight RGB over white, so alpha differences show up as colour.

    `render.render` already hands back floats in 0..1; dividing by 255 again quietly
    scales every colour into the near-black corner, where dE00 reads as almost nothing
    whatever the two pictures do.
    """
    img = render.render(svg, svgeval.JUDGE_SIZE, svgeval.JUDGE_SIZE)
    if img.shape[-1] == 4:
        img = img[..., :3] * img[..., 3:] + (1.0 - img[..., 3:])
    return img


def measure(exe: str, n: int) -> dict:
    items = svgeval.load_sets()["screen"]
    step = max(1, len(items) // n)
    items = items[::step][:n]
    tmp = Path(tempfile.mkdtemp())
    totals = {"b0": 0, "b1": 0, "p0": 0.0, "p1": 0.0}
    de00s: list[float] = []
    worst_bytes_only = 0.0
    worst_file = ""
    for it in items:
        _, gt = svgeval.item_paths(it)
        src = gt.read_text(encoding="utf-8")
        out_path = tmp / f"{it['stem']}.min.svg"
        raw_path = tmp / f"{it['stem']}.raw.svg"
        subprocess.run([exe, str(gt), "-o", str(out_path)], check=True, capture_output=True)
        subprocess.run(
            [exe, str(gt), "-o", str(raw_path), "--bytes-only"], check=True, capture_output=True
        )
        out = out_path.read_text(encoding="utf-8")
        raw = raw_path.read_text(encoding="utf-8")

        totals["b0"] += len(src.encode("utf-8"))
        totals["b1"] += len(out.encode("utf-8"))
        before, after = rgb(src), rgb(out)
        de00s.append(float(mcolor.delta_e00(before, after)["de00_mean"]))
        # The promise `--bytes-only` makes, checked against a renderer rather than against
        # our own idea of what the bytes mean.
        moved = float(np.abs(before - rgb(raw)).max() * 255.0)
        if moved > worst_bytes_only:
            worst_bytes_only, worst_file = moved, f"{it['corpus']}/{it['stem']}"
    return {
        "bytes_saved": 1.0 - totals["b1"] / max(totals["b0"], 1),
        "de00_mean": float(np.mean(de00s)),
        "de00_worst": float(np.max(de00s)),
        "bytes_only_max_px": worst_bytes_only,
        "bytes_only_worst_file": worst_file,
        "n": len(items),
    }


def provenance(exe: str) -> dict:
    def out(*args: str) -> str | None:
        try:
            return subprocess.run(args, cwd=ROOT, text=True, capture_output=True, check=True).stdout.strip()
        except (OSError, subprocess.CalledProcessError):
            return None

    return {
        "platform": platform.platform(),
        "rustc": out("rustc", "--version"),
        "source_revision": out("git", "rev-parse", "HEAD"),
        "exe": exe,
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(ROOT / "target/release/inkvec-svgmin.exe"))
    ap.add_argument("--n", type=int, default=40)
    ap.add_argument("--write-baseline", "--update", action="store_true")
    ap.add_argument("--bypass-gate", default="")
    a = ap.parse_args()

    now = measure(a.exe, a.n)
    print(
        f"bytes saved {now['bytes_saved']:.1%}   dE00 mean {now['de00_mean']:.4f} "
        f"worst {now['de00_worst']:.4f}   --bytes-only moved {now['bytes_only_max_px']:.2f}/255"
    )

    failures: list[str] = []
    # The promises, which are never traded and never ratcheted.
    if now["bytes_only_max_px"] > BYTES_ONLY_MAX_PX:
        failures.append(
            f"--bytes-only moved the picture by {now['bytes_only_max_px']:.2f}/255 on "
            f"{now['bytes_only_worst_file']}; that path may only respell the numbers"
        )
    if now["de00_worst"] > WORST_CEILING:
        failures.append(
            f"worst dE00 {now['de00_worst']:.4f} is over the {WORST_CEILING} ceiling"
        )

    if a.write_baseline or not BASELINE.exists():
        if failures:
            print("\n".join(f"FAIL {f}" for f in failures))
            return 1
        BASELINE.parent.mkdir(parents=True, exist_ok=True)
        BASELINE.write_text(
            json.dumps({**now, "_provenance": provenance(a.exe)}, indent=2) + "\n",
            encoding="utf-8",
        )
        print(f"baseline written to {BASELINE.relative_to(ROOT)}")
        return 0

    base = json.loads(BASELINE.read_text(encoding="utf-8"))
    gained = now["bytes_saved"] - base["bytes_saved"]
    # Positive when the picture got closer to the original.
    kept = base["de00_mean"] - now["de00_mean"]
    paid_for = gained >= SAVINGS_SLACK

    if gained < -SAVINGS_SLACK:
        failures.append(
            f"bytes saved fell {-gained:.1%}, from {base['bytes_saved']:.1%} to "
            f"{now['bytes_saved']:.1%}"
        )
    if kept < -FIDELITY_SLACK * base["de00_mean"] and not paid_for:
        failures.append(
            f"mean dE00 rose {-kept / base['de00_mean']:.0%}, from {base['de00_mean']:.4f} to "
            f"{now['de00_mean']:.4f}, and bytes saved did not rise to pay for it"
        )

    if failures:
        print("\nsvgmin gate FAILED:")
        for f in failures:
            print(f"  {f}")
        if a.bypass_gate:
            print(f"\nbypassed: {a.bypass_gate}")
            return 0
        print('\n  python bench/svgmin_gate.py --exe ... --bypass-gate "<justification>"')
        print("  python bench/svgmin_gate.py --exe ... --write-baseline")
        return 1

    # Ground gained is kept: a run that is better on one axis without spending the other
    # becomes the number to beat.
    if gained > 0 and kept >= 0:
        BASELINE.write_text(
            json.dumps({**now, "_provenance": provenance(a.exe)}, indent=2) + "\n",
            encoding="utf-8",
        )
        print(f"\nratchet: baseline tightened to {now['bytes_saved']:.1%} / {now['de00_mean']:.4f}")
    else:
        print("\nsvgmin gate PASSED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
