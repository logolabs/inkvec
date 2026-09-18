"""Score one build on a devset_v2 split, in parallel, and report per family + worst cases.

    python bench/full_eval.py <exe> [--set full|dev|held_a|held_b|screen|all] [--workers N]
    python bench/full_eval.py <exe> --set all --sample 0.1     # a reproducible tenth
    python bench/full_eval.py A.exe --compare B.exe ...   # paired: A = before, B = after

Writes bench/data/eval_<label>.json (per-icon dE00 / DISTS / params ratio / seconds) and
prints the per-family table and the worst icons by dE00. Uses the evolve evaluator's
scoring so the numbers are the loop's numbers (judged at 1024 px against the GT SVG).
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "bench"))
import svgeval  # noqa: E402


def table(ss: svgeval.SetScore, other: svgeval.SetScore | None = None) -> None:
    fam = ss.family_means()
    # Two axes, not one. dE00 and DISTS ask whether it looks right; self-residual,
    # turning and mirror ask whether it is built right, and a change can move those in
    # opposite directions - the sawtooth improved fidelity while tripling turning.
    print(f"{'family':16s} {'n':>4s} {'dE00':>7s} {'DISTS':>7s} {'ratio':>6s}"
          f" {'self':>7s} {'turn':>6s} {'mirror':>8s}" +
          (f" {'dE delta':>9s} {'b/w':>8s}" if other else ""))
    B = {i.stem: i for i in other.images} if other else {}
    for f in sorted(fam) + ["ALL"]:
        rows = [i for i in ss.images if f == "ALL" or i.corpus == f]
        de = np.mean([i.de00 for i in rows]); di = np.mean([i.dists for i in rows]); ra = np.mean([i.ratio for i in rows])
        if f == "ALL":
            de, di, ra = ss.de00, ss.dists, ss.ratio
        sr = np.mean([getattr(i, "self_res", 0.0) for i in rows])
        tu = np.mean([getattr(i, "turning", 0.0) for i in rows])
        mi = np.mean([getattr(i, "mirror", 0.0) for i in rows])
        line = (f"{f:16s} {len(rows):4d} {de:7.4f} {di:7.4f} {ra:6.2f}"
                f" {sr:7.4f} {tu:6.3f} {mi:8.5f}")
        if other:
            dd = np.array([B[i.stem].de00 - i.de00 for i in rows if i.stem in B])
            line += f" {dd.mean():+9.4f} {int((dd < -1e-4).sum()):3d}/{int((dd > 1e-4).sum()):<4d}"
        print(line)
    print(f"objective {ss.objective:.4f}" + (f" -> {other.objective:.4f}" if other else ""))
    if other:
        # Structure moves are reported separately and never folded into the objective:
        # they are a veto, not a term. A change that improves colour while raising turning
        # is the shape of every defect we shipped and failed to notice.
        a = {i.stem: i for i in ss.images}
        pairs = [(a[k], B[k]) for k in B if k in a]
        for name, get in (("self-residual", lambda i: getattr(i, "self_res", 0.0)),
                          ("turning", lambda i: getattr(i, "turning", 0.0)),
                          ("mirror", lambda i: getattr(i, "mirror", 0.0))):
            d = np.array([get(y) - get(x) for x, y in pairs])
            if not len(d):
                continue
            # Relative, because these quantities differ by orders of magnitude between
            # families and an absolute threshold would flag every run. One per cent of the
            # signal is the smallest move worth a second look.
            base = np.mean([get(x) for x, _ in pairs])
            flag = "  <-- worse" if d.mean() > 0.01 * max(base, 1e-9) else ""
            print(f"  {name:14s} {np.mean([get(x) for x, _ in pairs]):9.5f}"
                  f" -> {np.mean([get(y) for _, y in pairs]):9.5f}"
                  f"   {d.mean():+.5f}{flag}")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("exe")
    ap.add_argument("--compare", help="second build (the 'after'); the first is 'before'")
    ap.add_argument("--set", default="full",
                    choices=("dev", "held_a", "held_b", "full", "screen", "all"),
                    help="'all' is every corpus icon with a ground truth (1406), including "
                         "the 426 no set has ever scored")
    ap.add_argument("--sample", type=float, default=None, metavar="F",
                    help="score a stratified random fraction of the set, e.g. 0.1 for a "
                         "tenth; reproducible from --seed, and family proportions are kept")
    ap.add_argument("--seed", type=int, default=0, help="the draw --sample makes [default: 0]")
    ap.add_argument("--workers", type=int, default=max(1, (os.cpu_count() or 4) // 2))
    ap.add_argument("--label", default="full")
    ap.add_argument("--worst", type=int, default=30)
    ap.add_argument("--args", default="", help="extra tracer arguments for both builds, e.g. '--min-area 2'")
    a = ap.parse_args()
    items = svgeval.load_sets()[a.set]
    if a.sample is not None:
        whole = len(items)
        items = svgeval.sample(items, a.sample, a.seed)
        print(f"sampling {a.sample:.0%} of {a.set}: {len(items)} of {whole} icons "
              f"(seed {a.seed})", flush=True)
    out_dir = ROOT / "bench" / "data"
    extra = a.args.split()
    ss = svgeval.score_set(Path(a.exe).resolve(), "A", items, svgeval.WORK / f"_full_{a.label}_A", extra_args=extra, workers=a.workers)
    (out_dir / f"eval_{a.label}_A.json").write_text(json.dumps(ss.to_json(), indent=1), encoding="utf-8")
    other = None
    if a.compare:
        other = svgeval.score_set(Path(a.compare).resolve(), "B", items, svgeval.WORK / f"_full_{a.label}_B", extra_args=extra, workers=a.workers)
        (out_dir / f"eval_{a.label}_B.json").write_text(json.dumps(other.to_json(), indent=1), encoding="utf-8")
    print(f"\n{a.set}: {len(ss.images)} icons scored, {len(ss.failures)} failures")
    table(ss, other)
    ref = other or ss
    worst = sorted(ref.images, key=lambda i: -i.de00)[: a.worst]
    print(f"\nworst {a.worst} by dE00" + (" (after)" if other else "") + ":")
    for i in worst:
        print(f"  {i.stem:44s} {i.corpus:14s} dE {i.de00:.3f}  DISTS {i.dists:.4f}  ratio {i.ratio:.2f}")
    if ss.failures:
        print("failures:", ss.failures[:10])


if __name__ == "__main__":
    main()
