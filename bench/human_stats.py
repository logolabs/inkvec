"""How far the traces are from the artists' files in how they are *drawn*: the design
battery (`inkvec_bench/design.py`) over the screen set, per family, with the oddities, the
statistics' correlations, and a paired before/after comparison to tune a build or a flag.

    python bench/human_stats.py --exe target/release/inkvec                 # quality-512ssop, quality-web
    python bench/human_stats.py --exe target/release/inkvec --extra-args "--editability" \\
        --base-args ""                                                      # does the flag help?
    python bench/human_stats.py --exe new/inkvec --base-exe old/inkvec      # a change, paired
    python bench/human_stats.py --exe ... --conditions quality-512ssop --sample 0.25 --oddities 5
    python bench/human_stats.py --from-report gate/report.json --correlate  # a gate run's numbers

What it prints
--------------
Per condition and family: each statistic's mean on the artist's files and on the traces, and
the mean **divergence** (the per-icon distance between the two; `design.STATS` says how each
is taken). Then the **oddities**: per statistic, the icons farthest from their artist. With
`--base-exe` / `--base-args`, a second arm is traced and every statistic is compared icon by
icon: the family-macro mean divergence before and after, its relative change with a paired,
family-stratified bootstrap interval (`gate_stats.compare`, as the gate does), and how many
icons moved closer to or further from the artist; the last line is the geometric mean of the
after/before ratios over the statistics, one number to tune against (below 1: closer).
`--correlate` prints the Spearman matrix of the per-icon divergences, pooled over the
conditions, with the gate's own axes when they are known (`--from-report`, or `--gate-axes`,
which scores them), and the redundancy pruning that chose `design.KEPT`.

Tracing only: no renders, so a full condition costs the tracer's time plus about 10 ms of
statistics per icon. Traces are kept under the cache (`<cache>/traces/<build>-<args>/<tier>/`),
keyed by the executable's bytes and the arguments, so a re-run of the same build is free;
`--fresh` retraces.

The gate (`bench/ci_gate.py`) computes the same battery on every icon it traces and puts it in
its `--report-json` and `--artifact-dir` (`human-<platform>.json`); it is reported there, not
gated.
"""
from __future__ import annotations

import argparse
import functools
import hashlib
import json
import math
import os
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

import gate_stats
import svgeval
from inkvec_bench import design

DEFAULT_CONDITIONS = ("quality-512ssop", "quality-web")
#: The gate's axes that join the correlation matrix when known.
GATE_AXES = ("de00", "geom", "turning_gap", "ratio_gap")
#: Two statistics whose per-icon divergences correlate beyond this (|Spearman rho|) carry
#: one signal: the later one in `design.STATS` order is dropped (`prune`).
REDUNDANT_RHO = 0.7


# --------------------------------------------------------------------------- aggregation
def finite(x) -> bool:
    return x is not None and isinstance(x, (int, float)) and math.isfinite(x)


def family_mean(per_icon: dict, stat: str, field: str) -> dict[str, float]:
    """{family: mean of `field` ("div", "trace" or "artist") of `stat`} over the icons where
    it is defined, and "macro": the mean of those family means."""
    fam: dict[str, list] = defaultdict(list)
    for k, h in per_icon.items():
        v = h.get(field, {}).get(stat)
        if finite(v):
            fam[k.split("/", 1)[0]].append(v)
    out = {f: float(np.mean(v)) for f, v in sorted(fam.items())}
    out["macro"] = float(np.mean(list(out.values()))) if out else float("nan")
    return out


def document(results: dict[str, dict], stats=None) -> dict:
    """The battery of a run as JSON: per condition the family and macro means of each
    statistic (divergence, trace, artist) and per icon [trace value, divergence]; the
    artist's values once. `results` is {condition: {family/stem: {"human": ...}}}."""
    stats = list(stats or design.KEPT)
    out: dict = {"stats": stats, "version": design.VERSION, "conditions": {}, "artist": {}}
    for cond, rows in results.items():
        per = {k: r["human"] for k, r in rows.items() if r.get("human")}
        out["conditions"][cond] = {
            "summary": {s: {f: family_mean(per, s, f) for f in ("div", "trace", "artist")}
                        for s in stats},
            "per_icon": {k: {s: [h["trace"].get(s), h["div"].get(s)] for s in stats}
                         for k, h in sorted(per.items())},
        }
        for k, h in per.items():
            out["artist"].setdefault(k, {s: h["artist"].get(s) for s in stats})
    out["artist"] = dict(sorted(out["artist"].items()))
    return clean(out)


def clean(obj):
    """`obj` with every non-finite float as None, so it is strict JSON."""
    if isinstance(obj, dict):
        return {k: clean(v) for k, v in obj.items()}
    if isinstance(obj, (list, tuple)):
        return [clean(v) for v in obj]
    if isinstance(obj, float) and not math.isfinite(obj):
        return None
    return obj


SHORT = {"quality": "q", "fast": "f"}


def print_summary(doc: dict, file=None) -> None:
    """The gate's compact console summary: each statistic's family-macro mean divergence,
    one column per condition (nothing when no icon has the battery)."""
    file = file or sys.stdout
    conds = [c for c in doc["conditions"] if doc["conditions"][c]["per_icon"]]
    if not conds:
        return
    heads = ["-".join([SHORT.get(c.split("-", 1)[0], c), c.split("-", 1)[-1]]) for c in conds]
    print("\nhuman statistics: distance from the artist's file, family-macro mean (reported, "
          "not gated; definitions in bench/inkvec_bench/design.py, detail in "
          "bench/human_stats.py)", file=file)
    print(f"  {'statistic':18s}" + "".join(f"{h:>11s}" for h in heads), file=file)
    for s in doc["stats"]:
        vals = [doc["conditions"][c]["summary"][s]["div"].get("macro") for c in conds]
        print(f"  {s:18s}" + "".join(f"{v:11.3f}" if finite(v) else f"{'-':>11s}" for v in vals),
              file=file)


# --------------------------------------------------------------------------- correlation
def spearman(x: np.ndarray, y: np.ndarray) -> float:
    """Spearman's rho over the pairs where both are finite (average ranks for ties)."""
    from scipy.stats import rankdata
    m = np.isfinite(x) & np.isfinite(y)
    if m.sum() < 10:
        return float("nan")
    rx, ry = rankdata(x[m]), rankdata(y[m])
    if rx.std() == 0 or ry.std() == 0:
        return float("nan")
    return float(np.corrcoef(rx, ry)[0, 1])


def matrix(columns: dict[str, np.ndarray]) -> tuple[list[str], np.ndarray]:
    names = list(columns)
    r = np.full((len(names), len(names)), np.nan)
    for i, a in enumerate(names):
        for j, b in enumerate(names):
            r[i, j] = 1.0 if i == j else spearman(columns[a], columns[b])
    return names, r


def prune(names: list[str], rho: np.ndarray, fixed: tuple[str, ...] = ()) -> tuple[list[str], dict]:
    """Greedy redundancy pruning: walk `names` in order (the gate's axes in `fixed` first,
    always kept) and keep a statistic unless its |rho| with one already kept exceeds
    `REDUNDANT_RHO`. Returns (kept, {dropped: (the kept statistic it duplicates, rho)})."""
    order = [n for n in fixed if n in names] + [n for n in names if n not in fixed]
    kept: list[str] = []
    dropped: dict = {}
    for n in order:
        i = names.index(n)
        worst = max(((abs(rho[i, names.index(k)]), k) for k in kept
                     if np.isfinite(rho[i, names.index(k)])), default=(0.0, None))
        if n not in fixed and worst[0] > REDUNDANT_RHO:
            dropped[n] = (worst[1], float(rho[i, names.index(worst[1])]))
        else:
            kept.append(n)
    return kept, dropped


def print_matrix(names: list[str], rho: np.ndarray, file=None) -> None:
    file = file or sys.stdout
    w = 6
    print(" " * 19 + "".join(f"{i:>{w}d}" for i in range(len(names))), file=file)
    for i, n in enumerate(names):
        cells = "".join(f"{rho[i, j]:{w}.2f}" if np.isfinite(rho[i, j]) else f"{'':>{w}s}"
                        for j in range(len(names)))
        print(f"{i:2d} {n:16s}" + cells, file=file)


# --------------------------------------------------------------------------- tracing
@functools.lru_cache(maxsize=8)
def _exe_key(path: str, mtime_ns: int, size: int) -> str:
    return svgeval.exe_key(Path(path))


def trace_dir(exe: Path, args: tuple[str, ...], tier: str) -> Path:
    """Where the traces of build `exe` with tracer flags `args` at `tier` are kept."""
    st = exe.stat()
    tag = hashlib.sha1(" ".join(args).encode()).hexdigest()[:8]
    return svgeval.CACHE / "traces" / f"{_exe_key(str(exe), st.st_mtime_ns, st.st_size)}-{tag}" / tier


def one(job: tuple) -> tuple[str, dict]:
    """Trace one icon (or read its kept trace) and take the battery; with `gate`, also the
    gate's own signals. Top-level for the process pool."""
    exe, it, args, cond_name, fresh, gate = job
    import ci_gate
    cond = ci_gate.BY_NAME[cond_name]
    tier = cond.tier
    svgeval.set_tier(tier)
    key = f"{it['corpus']}/{it['stem']}"
    png, gt = svgeval.item_paths(it)
    out = trace_dir(exe, args, tier) / f"{it['corpus']}__{it['stem']}.svg"
    if fresh or not out.exists():
        out.parent.mkdir(parents=True, exist_ok=True)
        fail, _, _, _ = svgeval._trace(svgeval.Job(exe, it, out.parent, args))
        if fail:
            return key, {"fail": fail["fail"]}
    svg = out.read_text(encoding="utf-8")
    from PIL import Image
    with Image.open(png) as im:
        px = im.size[0]
    row = {"human": svgeval.human_stats(svg, gt, px)}
    if gate:
        sig = svgeval.score_svg(svg, it, png, gt)
        if "fail" not in sig:
            row.update(sig)
            ci_gate.with_gaps({key: row}, cond)
    return key, row


def run_arm(exe: Path, args: tuple[str, ...], conds: list[str], items: list[dict],
            workers: int, fresh: bool, gate: bool) -> dict[str, dict]:
    import ci_gate
    out: dict[str, dict] = {}
    with svgeval.scoring_pool(workers) as pool:
        for c in conds:
            full_args = tuple(ci_gate.BY_NAME[c].args) + args
            jobs = [(exe, it, full_args, c, fresh, gate) for it in items]
            rows = dict(pool.map(one, jobs, chunksize=2))
            bad = [f"{k}: {r['fail']}" for k, r in rows.items() if "fail" in r]
            for b in bad[:5]:
                print(f"  {c}: {b}", file=sys.stderr)
            out[c] = {k: r for k, r in rows.items() if "fail" not in r}
    return out


# --------------------------------------------------------------------------- reports
def print_families(doc: dict, stats: list[str]) -> None:
    for cond, d in doc["conditions"].items():
        fams = [f for f in d["summary"][stats[0]]["div"] if f != "macro"]
        print(f"\n== {cond}: artist / trace (divergence), mean per family; a mix reads its "
              "share of cubics, a distribution its median, a palette its size")
        print(f"  {'statistic':18s}" + "".join(f"{f[:14]:>24s}" for f in fams + ["macro"]))
        for s in stats:
            cells = []
            for f in fams + ["macro"]:
                a = d["summary"][s]["artist"].get(f)
                t = d["summary"][s]["trace"].get(f)
                v = d["summary"][s]["div"].get(f)
                cells.append(f"{a:7.2f}/{t:7.2f} ({v:5.2f})" if all(finite(x) for x in (a, t, v))
                             else f"{'-':>24s}")
            print(f"  {s:18s}" + "".join(f"{c:>24s}" for c in cells))


def print_oddities(doc: dict, stats: list[str], n: int) -> None:
    """Per statistic, the `n` icons farthest from their artist; then overall, the icons with
    the highest mean divergence rank over the statistics (ties share their rank; artefact
    signals left out)."""
    from scipy.stats import rankdata
    for cond, d in doc["conditions"].items():
        per = d["per_icon"]
        print(f"\n== {cond}: oddities, the {n} icons farthest from their artist per statistic "
              "(divergence; trace value vs artist value)")
        ranks: dict[str, list] = defaultdict(list)
        for s in stats:
            rows = [(v[s][1], k, v[s][0], doc["artist"].get(k, {}).get(s))
                    for k, v in per.items() if s in v and finite(v[s][1])]
            if not rows:
                continue
            rows.sort(key=lambda r: (-r[0], r[1]))
            print(f"  {s:18s} " + "; ".join(f"{k} {dv:.2f} ({t:.2f} vs {a:.2f})"
                                            if finite(t) and finite(a) else f"{k} {dv:.2f}"
                                            for dv, k, t, a in rows[:n]))
            if s in design.ARTEFACTS:
                continue
            r = rankdata([x[0] for x in rows]) - 1
            for (_, k, _, _), rk in zip(rows, r):
                ranks[k].append(rk / max(1, len(rows) - 1))
        overall = sorted(((float(np.mean(v)), k) for k, v in ranks.items()), reverse=True)
        print("  overall, mean divergence rank over the statistics (1 = farthest on all): "
              + "; ".join(f"{k} {m:.2f}" for m, k in overall[:n]))


def print_ab(after: dict, before: dict, stats: list[str]) -> dict:
    """Paired comparison of two arms, per condition and statistic; returns the numbers."""
    res: dict = {}
    for cond, a_rows in after.items():
        b_rows = before.get(cond, {})
        print(f"\n== {cond}: before -> after, family-macro mean divergence from the artist "
              "(lower is closer)")
        print(f"  {'statistic':18s} {'before':>9s} {'after':>9s} {'change':>8s} "
              f"{'95% CI':>17s}  closer further")
        ratios = []
        res[cond] = {}
        for s in stats:
            base, cur, fam = {}, {}, {}
            for k in a_rows.keys() & b_rows.keys():
                x = b_rows[k].get("human", {}).get("div", {}).get(s)
                y = a_rows[k].get("human", {}).get("div", {}).get(s)
                if finite(x) and finite(y):
                    base[k], cur[k], fam[k] = x, y, k.split("/", 1)[0]
            if len(base) < 4:
                continue
            c = gate_stats.compare(base, cur, fam, "macro")
            if c.base > 0 and c.cur > 0 and s not in design.ARTEFACTS:
                ratios.append(c.cur / c.base)
            res[cond][s] = {"before": c.base, "after": c.cur, "rel": c.rel, "p025": c.p025,
                            "p975": c.p975, "closer": c.better, "further": c.worse}
            sig = "*" if c.p975 < 0 or c.p025 > 0 else " "
            print(f"  {s:18s} {c.base:9.4f} {c.cur:9.4f} {c.rel * 100:+7.1f}% "
                  f"[{c.p025 * 100:+6.1f}, {c.p975 * 100:+6.1f}]{sig} {c.better:6d} {c.worse:7d}")
        if ratios:
            g = float(np.exp(np.mean(np.log(ratios))))
            res[cond]["composite"] = g
            print(f"  composite (geometric mean of after/before over {len(ratios)} statistics, "
                  "artefact signals left out): "
                  f"{g:.4f} ({(g - 1) * 100:+.1f}%; below 1 is closer to the artist)")
    print("  (* the 95 % interval excludes zero)")
    return res


def correlate(results: dict[str, dict], stats: list[str], with_gate: bool) -> None:
    keys = [(c, k) for c, rows in results.items() for k in rows]
    cols: dict[str, np.ndarray] = {}
    if with_gate:
        for ax in GATE_AXES:
            v = np.array([results[c][k].get(ax, np.nan) for c, k in keys], dtype=float)
            if np.isfinite(v).sum() >= 10:
                cols[ax] = v
    for s in stats:
        cols[s] = np.array([results[c][k].get("human", {}).get("div", {}).get(s, np.nan)
                            for c, k in keys], dtype=float)
        cols[s][~np.isfinite(cols[s])] = np.nan
    names, rho = matrix(cols)
    fixed = tuple(ax for ax in GATE_AXES if ax in cols)
    print(f"\n== Spearman correlation of the per-icon divergences, pooled over "
          f"{', '.join(results)} ({len(keys)} icon-conditions; pairwise complete)")
    print_matrix(names, rho)
    kept, dropped = prune(names, rho, fixed)
    print(f"\nredundancy pruning at |rho| > {REDUNDANT_RHO} (gate axes kept first, then "
          "design.STATS order):")
    for n, (k, r) in dropped.items():
        print(f"  drop {n:18s} rho {r:+.2f} with {k}")
    print("  keep " + ", ".join(n for n in kept if n not in fixed))
    big = [(abs(rho[i, j]), names[i], names[j]) for i in range(len(names)) for j in range(i)
           if np.isfinite(rho[i, j]) and names[i] in kept and names[j] in kept]
    big.sort(reverse=True)
    print("  largest |rho| among the kept: " + ", ".join(f"{a}~{b} {r:.2f}" for r, a, b in big[:6]))
    # The same within families: each column replaced by its rank within the icon's family
    # and condition, so a family's style (lucide on its grid, noto-emoji off it) cannot
    # make two statistics look alike.
    groups = np.array([f"{c}|{k.split('/', 1)[0]}" for c, k in keys])
    within = {n: _within_ranks(v, groups) for n, v in cols.items()}
    _, rho_w = matrix(within)
    print("  within families (ranks within family and condition): "
          + ", ".join(f"{n}~{k} {rho_w[names.index(n), names.index(k)]:+.2f}"
                      for n, (k, _) in dropped.items()))
    big_w = [(abs(rho_w[i, j]), names[i], names[j]) for i in range(len(names)) for j in range(i)
             if np.isfinite(rho_w[i, j]) and names[i] in kept and names[j] in kept]
    big_w.sort(reverse=True)
    print("  largest within-family |rho| among the kept: "
          + ", ".join(f"{a}~{b} {r:.2f}" for r, a, b in big_w[:6]))


def _within_ranks(v: np.ndarray, groups: np.ndarray) -> np.ndarray:
    """Each finite value's rank within its group, scaled to [0, 1] (NaN stays NaN)."""
    from scipy.stats import rankdata
    out = np.full(v.shape, np.nan)
    for g in np.unique(groups):
        m = (groups == g) & np.isfinite(v)
        if m.sum() > 1:
            out[m] = (rankdata(v[m]) - 1) / (m.sum() - 1)
    return out


# --------------------------------------------------------------------------- main
#: Options whose value is itself a list of tracer flags, so starts with "-".
FLAG_OPTIONS = ("--extra-args", "--base-args")


def flag_values(argv: list[str]) -> list[str]:
    """`argv` with `--extra-args --editability` written `--extra-args=--editability`: argparse
    would read a value that starts with "-" as an option of its own."""
    out, i = [], 0
    while i < len(argv):
        if argv[i] in FLAG_OPTIONS and i + 1 < len(argv):
            out.append(f"{argv[i]}={argv[i + 1]}")
            i += 2
        else:
            out.append(argv[i])
            i += 1
    return out


def load_report(path: Path) -> dict[str, dict]:
    """A gate report's per-icon rows: {condition: {key: {axes..., "human": {...}}}}."""
    rep = json.loads(path.read_text(encoding="utf-8"))
    hum = rep.get("human") or {}
    out: dict[str, dict] = {}
    for cond, d in hum.get("conditions", {}).items():
        rows = {}
        for k, v in d["per_icon"].items():
            rows[k] = {"human": {"trace": {s: x[0] for s, x in v.items()},
                                 "div": {s: x[1] for s, x in v.items()},
                                 "artist": hum.get("artist", {}).get(k, {})}}
            rows[k].update(rep.get("per_icon", {}).get(cond, {}).get(k, {}))
        out[cond] = rows
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--exe", type=Path, help="the build to read (the 'after' arm)")
    ap.add_argument("--extra-args", default="", help="tracer flags for --exe, e.g. \"--editability\"")
    ap.add_argument("--base-exe", type=Path, default=None,
                    help="a second build to compare with (the 'before' arm; default: --exe)")
    ap.add_argument("--base-args", default=None,
                    help="tracer flags for the 'before' arm (\"\" for none); with --base-exe or "
                         "this, the two arms are compared icon by icon")
    ap.add_argument("--conditions", default=",".join(DEFAULT_CONDITIONS),
                    help="gate conditions to trace (bench/ci_gate.py CONDITIONS)")
    ap.add_argument("--sample", type=float, default=None, metavar="FRACTION")
    ap.add_argument("--workers", type=int, default=2)
    ap.add_argument("--fresh", action="store_true", help="retrace even when a kept trace exists")
    ap.add_argument("--from-report", type=Path, nargs="+", default=None,
                    help="read gate --report-json files instead of tracing")
    ap.add_argument("--all-stats", action="store_true",
                    help="every candidate statistic (design.STATS), not only design.KEPT")
    ap.add_argument("--families", action="store_true", help="the per-family table")
    ap.add_argument("--oddities", type=int, default=3, metavar="N")
    ap.add_argument("--correlate", action="store_true", help="the Spearman matrix and pruning")
    ap.add_argument("--gate-axes", action="store_true",
                    help="also score the gate's axes (renders; slower) for --correlate")
    ap.add_argument("--json", type=Path, default=None, help="write the numbers here")
    a = ap.parse_args(flag_values(sys.argv[1:]))
    os.environ.setdefault("INKVEC_SKIP_DISTS", "1")
    stats = list(design.STATS) if a.all_stats or a.correlate else list(design.KEPT)

    import shlex
    if a.from_report:
        after = {}
        for path in a.from_report:
            after.update(load_report(path))
        before = None
    else:
        if a.exe is None:
            ap.error("--exe or --from-report is required")
        conds = [c.strip() for c in a.conditions.split(",") if c.strip()]
        items = svgeval.load_sets()["screen"]
        if a.sample is not None:
            items = svgeval.sample(items, a.sample)
        exe = a.exe.resolve()
        args = tuple(shlex.split(a.extra_args))
        print(f"human_stats: {len(items)} icons x {len(conds)} conditions, {exe.name} "
              f"{' '.join(args)}", flush=True)
        after = run_arm(exe, args, conds, items, a.workers, a.fresh, a.gate_axes)
        before = None
        if a.base_exe is not None or a.base_args is not None:
            bexe = (a.base_exe or a.exe).resolve()
            bargs = tuple(shlex.split(a.base_args or ""))
            print(f"before: {bexe.name} {' '.join(bargs)}", flush=True)
            before = run_arm(bexe, bargs, conds, items, a.workers, a.fresh, a.gate_axes)

    doc = document(after, stats)
    if a.families or not before:
        print_families(doc, stats)
    if a.oddities:
        print_oddities(doc, stats, a.oddities)
    out = {"after": doc}
    if before:
        out["before"] = document(before, stats)
        out["paired"] = print_ab(after, before, stats)
    if a.correlate:
        correlate(after, stats, with_gate=a.gate_axes or a.from_report is not None)
    if a.json:
        a.json.parent.mkdir(parents=True, exist_ok=True)
        a.json.write_text(json.dumps(out, indent=1), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
