"""Run the benchmark suite.

    python bench/suite/run.py --split screen --limit 40
    python bench/suite/run.py --split full --engines inkvec vtracer
    python bench/suite/run.py --degrade clean jpeg-q50 blur-1.0 --limit 25
    python bench/suite/run.py --svgenius C:/Users/stefa/svgb --tier hard --limit 40

Writes `bench/data/suite_<label>.json` and prints a table. Every engine sees byte-identical
input, every metric is computed from one pair of renders, and a failed trace is counted as
a failure rather than dropped -- an engine that crashes on a tenth of the corpus should not
come out looking like one that traced it badly.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import datasets as ds  # noqa: E402
import engines as eng  # noqa: E402
import metrics as mt  # noqa: E402
import published  # noqa: E402


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--split", default="screen", help="internal devset_v2 split")
    ap.add_argument("--svgenius", help="path to an SVGenius clone; runs that instead")
    ap.add_argument("--tier", default="hard", choices=["easy", "medium", "hard"])
    ap.add_argument("--engines", nargs="*", default=None)
    ap.add_argument("--degrade", nargs="*", default=["clean"],
                    help=f"any of {sorted(ds.DEGRADATIONS)}")
    ap.add_argument("--limit", type=int, default=None)
    ap.add_argument("--judge", type=int, default=512)
    ap.add_argument("--label", default=None)
    a = ap.parse_args()

    for d in a.degrade:
        if d not in ds.DEGRADATIONS:
            raise SystemExit(f"unknown degradation {d!r}; have {sorted(ds.DEGRADATIONS)}")

    if a.svgenius:
        items = list(ds.svgenius(Path(a.svgenius), a.tier, a.limit))
        source = f"svgenius-{a.tier}"
    else:
        items = list(ds.internal(a.split, a.limit))
        source = a.split
    if not items:
        raise SystemExit("no items found")

    engines = eng.available(a.engines)
    if not engines:
        raise SystemExit("no engines available")
    label = a.label or f"{source}_{'-'.join(a.degrade)}"
    cache = ROOT / "out/suite_inputs"

    print(f"suite: {len(items)} items x {len(engines)} engines x {len(a.degrade)} inputs"
          f"  (judged at {a.judge} px)\n")

    results: dict = {"source": source, "judge": a.judge, "n_items": len(items),
                     "rows": {}}
    for deg in a.degrade:
        for ename, engine in engines.items():
            rows: list[mt.Scores] = []
            for it in items:
                # A bilevel engine is not asked to vectorise a multi-colour icon; that
                # would be measuring the wrong thing and calling it a defeat.
                if ename in eng.BILEVEL_ONLY and not it.bilevel:
                    continue
                src = ds.apply_degradation(it, deg, cache)
                res = engine(src)
                if not res.ok:
                    rows.append(mt.Scores(seconds=res.seconds, valid=False,
                                          extra={"error": res.note}))
                    continue
                rows.append(mt.score(it.gt_svg.read_text(encoding="utf-8", errors="ignore"),
                                     res.svg, res.seconds, a.judge))
            agg = mt.aggregate(rows)
            results["rows"][f"{deg}|{ename}"] = agg
            print(f"  {deg:12s} {ename:9s} n={agg.get('n', 0):4d}"
                  f" ok={agg.get('n_valid', 0):4d}", flush=True)

    hdr = (f"\n{'input':13s} {'engine':9s} {'n':>4s} {'fail':>5s} {'dE00':>8s} {'LPIPS':>8s}"
           f" {'DINO':>7s} {'SSIM':>7s} {'params':>7s} {'KB':>6s} {'p50':>6s} {'p95':>6s}"
           f" {'max':>6s} {'>5s':>4s}")
    print(hdr)
    print("-" * len(hdr))
    for key, r in results["rows"].items():
        deg, ename = key.split("|")
        if not r.get("n_valid"):
            print(f"{deg:13s} {ename:9s} {r.get('n', 0):4d} {r.get('n_failed', 0):5d}"
                  f"   all failed")
            continue
        print(f"{deg:13s} {ename:9s} {r['n']:4d} {r['n_failed']:5d} {r['de00']:8.4f}"
              f" {r['lpips']:8.4f} {r['dino']:7.4f} {r['ssim']:7.4f}"
              f" {r['params_ratio']:7.2f} {r['bytes'] / 1024:6.1f}"
              f" {r['sec_p50']:6.2f} {r['sec_p95']:6.2f} {r['sec_max']:6.2f}"
              f" {r['over_5s']:4d}")

    if a.svgenius:
        published.print_reference(f"svgenius-{a.tier}")

    out = ROOT / "bench/data" / f"suite_{label}.json"
    out.write_text(json.dumps(results, indent=1), encoding="utf-8")
    print(f"\nwrote {out.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
