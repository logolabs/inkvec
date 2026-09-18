"""The regression gate: score the 246-icon screen set and fail on a worse number.

    python bench/ci_gate.py --exe target/release/inkvec [--workers N]
    python bench/ci_gate.py --exe ... --write-baseline    # manual re-baseline after deliberate change
    python bench/ci_gate.py --exe ... --bypass-gate "Strong justification approved by user"

Three numbers are gated, each against `bench/gate/baseline.json`:

* **dE00** (colour error vs the artist's file)  may rise at most 1 %
* **turning** (anchor turning per unit length)  may rise at most 1 %
* **ratio** (parameters vs the artist's)         may rise at most 5 %

Quality gains are absorbed automatically: whenever a metric improves beyond the current
baseline, `baseline.json` is updated downwards on the spot so that ground gained is
never surrendered.

If a metric regresses, the gate FAILS. The gate can only be bypassed if the user agrees
and provides a strong, explicit justification via `--bypass-gate "<justification>"`.

The screen set's 246 rasters and truths (1.6 MB) are committed under bench/data so this
runs from a bare checkout. Everything else under bench/data stays ignored.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

import svgeval  # noqa: E402

BASELINE = ROOT / "bench" / "gate" / "baseline.json"
LIMITS = {"de00": 0.01, "turning": 0.01, "ratio": 0.05}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--exe", required=True)
    ap.add_argument("--workers", type=int, default=max(1, (os.cpu_count() or 4) // 2))
    ap.add_argument("--write-baseline", "--update", action="store_true", help="Explicitly re-baseline current numbers into baseline.json")
    ap.add_argument("--bypass-gate", type=str, default=None, metavar="JUSTIFICATION",
                    help="Bypass statistical regression failure if user agreed and provides a strong justification")
    a = ap.parse_args()

    if a.bypass_gate is not None:
        justification = a.bypass_gate.strip()
        if len(justification) < 10:
            print("ERROR: --bypass-gate requires a substantial justification (at least 10 characters) explaining user approval.", file=sys.stderr)
            return 1

    exe = Path(a.exe).resolve()
    items = svgeval.load_sets()["screen"]
    work = ROOT / "bench" / "data" / "_gate"
    ss = svgeval.score_set(exe, "gate", items, work, workers=a.workers, use_cache=False)
    n = len(ss.images)
    mean = lambda k: float(sum(getattr(i, k, 0.0) for i in ss.images) / max(1, n))  # noqa: E731
    now = {
        "de00": ss.de00,
        "turning": mean("turning"),
        "ratio": ss.ratio,
        "self_res": mean("self_res"),
        "n": n,
    }
    print(f"gate: {n} icons scored")
    for k in ("de00", "turning", "ratio", "self_res"):
        print(f"  {k:10s} {now[k]:.5f}")
    if n < len(items) - 2:
        print(f"FAIL: only {n} of {len(items)} icons scored")
        return 1

    if a.write_baseline:
        if a.bypass_gate:
            now["_bypass_justification"] = a.bypass_gate.strip()
        BASELINE.parent.mkdir(parents=True, exist_ok=True)
        BASELINE.write_text(json.dumps(now, indent=2), encoding="utf-8")
        print(f"wrote {BASELINE}")
        return 0

    if not BASELINE.exists():
        print("no baseline; run with --write-baseline")
        return 1

    base = json.loads(BASELINE.read_text(encoding="utf-8"))
    failures: list[str] = []
    gains: list[str] = []
    tightened = dict(base)

    for k, lim in LIMITS.items():
        b, v = base[k], now[k]
        allowed = b * (1 + lim)
        if v > allowed:
            failures.append(f"{k:10s} {b:.5f} -> {v:.5f}  (limit {allowed:.5f}) WORSE (+{(v/b - 1)*100:.2f}%)")
            print(f"  {k:10s} {b:.5f} -> {v:.5f}  (limit {allowed:.5f}) WORSE")
        elif v < b * 0.9999:  # measurably better
            gains.append(f"{k:10s} {b:.5f} -> {v:.5f}  (-{(1 - v/b)*100:.2f}%)")
            tightened[k] = v
            print(f"  {k:10s} {b:.5f} -> {v:.5f}  (limit {allowed:.5f}) ok [BETTER]")
        else:
            print(f"  {k:10s} {b:.5f} -> {v:.5f}  (limit {allowed:.5f}) ok")

    if failures:
        if a.bypass_gate:
            print("\n======================================================================")
            print("  *** STATISTICAL QUALITY GATE BYPASSED BY USER AGREEMENT ***")
            print(f"  Justification: {a.bypass_gate.strip()}")
            print("======================================================================")
            for f in failures:
                print(f"  bypassed failure: {f}")
            if a.write_baseline:
                now["_bypass_justification"] = a.bypass_gate.strip()
                BASELINE.parent.mkdir(parents=True, exist_ok=True)
                BASELINE.write_text(json.dumps(now, indent=2), encoding="utf-8")
                print(f"\nwrote {BASELINE} with bypass justification")
            return 0
        else:
            print("\nstatistical quality gate FAILED:\n")
            for f in failures:
                print(f"  {f}")
            print("\nQuality regressed beyond allowed bounds.")
            print("This can only be bypassed if the user explicitly agrees and provides strong justification:")
            print('  python bench/ci_gate.py --exe ... --bypass-gate "<strong justification approved by user>"')
            return 1

    if gains:
        tightened["n"] = now["n"]
        tightened["self_res"] = now["self_res"]
        tightened.pop("_bypass_justification", None)
        BASELINE.parent.mkdir(parents=True, exist_ok=True)
        BASELINE.write_text(json.dumps(tightened, indent=2), encoding="utf-8")
        print("\nstatistical ratchet passed, baseline auto-tightened to match:")
        for g in gains:
            print(f"  better  {g}")
    else:
        print("\nstatistical quality gate PASSED (within baseline limits)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
