"""Score one build across every resolution tier, and compare two builds tier by tier.

Most of this project's history was measured at 128 px, because for a long time that was the
only tier the corpus had. That made a whole class of change untestable: anything whose
effect depends on how many pixels carry the same drawing -- resolution invariance, the
noise estimate, the fitter's price per parameter -- could only be argued about.

The corpus now renders the same 246 ground truths at 256, 512 and 1024 as well, by the same
supersampled intake, so a difference between tiers is the resolution and nothing else. This
runs a build against all of them and prints one table.

    python bench/tiers.py <exe>                     # one build, every tier
    python bench/tiers.py <exe> --compare <exe2>    # before/after, every tier
    python bench/tiers.py <exe> --tiers 128ss,512ss --sample 0.25

A change that claims to be behaviour-neutral should reproduce every number in every row.
A change that claims to help should say which rows it expected to move, before it is run.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

ALL_TIERS = ("128ss", "256ss", "512ss", "1024ss")

METRICS = ("de00", "dists", "ratio", "objective")


def run_tier(exe: str, tier: str, args: argparse.Namespace, label: str) -> dict | None:
    """One `full_eval` run pinned to one tier, returned as its metrics dict.

    Shelled out rather than imported because `svgeval.tier()` resolves once per process and
    caches: two tiers in one interpreter would silently score the second with the first's
    rasters, which is exactly the bug its own doc comment records.
    """
    import json

    env = {**os.environ, "INKVEC_TIER": tier}
    # The score cache keys on the tier already, but a salt keeps a comparison run from
    # colliding with whatever else has been measured today.
    env["INKVEC_CACHE_SALT"] = f"{args.salt}-{tier}"
    cmd = [
        sys.executable, str(ROOT / "bench" / "full_eval.py"), exe,
        "--set", args.set, "--workers", str(args.workers), "--label", f"{label}_{tier}",
    ]
    if args.sample is not None:
        cmd += ["--sample", str(args.sample), "--seed", str(args.seed)]
    if args.args:
        # `--args=<value>`, not two elements: a value beginning with "-" is read as another
        # option otherwise, and argparse rejects it as a missing argument.
        cmd += [f"--args={args.args}"]

    r = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True)
    out = ROOT / "bench" / "data" / f"eval_{label}_{tier}_A.json"
    if not out.exists():
        print(f"   ! {tier}: no result written")
        if r.stderr:
            print("     " + r.stderr.strip().splitlines()[-1][:160])
        return None
    return json.loads(out.read_text(encoding="utf-8"))


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("exe")
    ap.add_argument("--compare", help="second build; the first is 'before'")
    ap.add_argument("--tiers", default=",".join(ALL_TIERS))
    ap.add_argument("--set", default="screen")
    ap.add_argument("--sample", type=float, default=None)
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--args", default="", help="extra tracer flags for both builds")
    ap.add_argument("--salt", default="tiers", help="cache salt prefix")
    ap.add_argument("--label", default="tier")
    a = ap.parse_args()

    tiers = [t.strip() for t in a.tiers.split(",") if t.strip()]

    missing = [
        t for t in tiers
        if not any((ROOT / "bench" / "data" / "corpus_raster").glob(f"*/{t}/*.png"))
    ]
    if missing:
        print(f"no rasters for {', '.join(missing)} -- build them first "
              f"(bench/build_tiers.py)")
        return 1

    print(f"set {a.set}"
          + (f", {a.sample:.0%} sample (seed {a.seed})" if a.sample is not None else "")
          + f", tiers {', '.join(tiers)}\n")

    head = f"{'tier':>8s} {'n':>5s}" + "".join(f"{m:>11s}" for m in METRICS)
    if a.compare:
        head += f"{'objective D':>13s}"
    print(head)

    rows = []
    for t in tiers:
        before = run_tier(a.exe, t, a, f"{a.label}_before" if a.compare else a.label)
        if before is None:
            continue
        line = f"{t:>8s} {before['n']:5d}" + "".join(f"{before[m]:11.4f}" for m in METRICS)
        after = None
        if a.compare:
            after = run_tier(a.compare, t, a, f"{a.label}_after")
            if after is not None:
                d = after["objective"] - before["objective"]
                mark = "same" if abs(d) < 1e-9 else f"{d:+.4f}"
                line += f"{mark:>13s}"
        print(line)
        rows.append((t, before, after))

    if a.compare and rows:
        print()
        for t, before, after in rows:
            if after is None:
                continue
            moved = [m for m in METRICS if abs(after[m] - before[m]) > 1e-9]
            if not moved:
                print(f"  {t}: every metric identical")
            else:
                print(f"  {t}: moved " + ", ".join(
                    f"{m} {before[m]:.4f}->{after[m]:.4f}" for m in moved))
    return 0


if __name__ == "__main__":
    sys.exit(main())
