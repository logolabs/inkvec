"""The regression gate: score the 246-icon screen set under six conditions, and fail when a
change cannot show it is not worse than the recorded baseline.

    python bench/ci_gate.py --exe target/release/inkvec [--workers N]
    python bench/ci_gate.py --exe ... --conditions quality-512ss,fast-512ss   # a subset
    python bench/ci_gate.py --exe ... --write-baseline        # record this build (a decision)
    python bench/ci_gate.py --exe ... --bypass-gate "Strong justification approved by user"
    python bench/ci_gate.py --exe ... --report-json out/gate.json --artifact-dir out/gate

Conditions
----------
Each condition is a tracer mode and an intake tier of the same 246 icons (`CONDITIONS`):

    quality-128ss   quality-512ss   quality-512ssop
    fast-128ss      fast-512ss      fast-512ssop

`128ss` / `512ss` are the committed 8x-supersampled rasters at 128 and 512 px; `512ssop` is
the 512 px raster flattened onto white, an opaque logo on a white page (`svgeval.item_paths`
derives it). Every condition is judged at 1024 px against the artist's file.

Why six and not one. The gate used to score Quality at 128 px only, 98 % transparent; users
send 512-2048 px, mostly opaque. On the 0.2.4 boundary-solve rewrite the 128 px gate read
-8.5 % dE00 where 512 px read -0.9 % (indistinguishable from zero), per-icon deltas at 128
and 512 px were uncorrelated (Spearman +0.14), and material-icons went from -47 % at 128 px
to +14 % at 512 px: a regression the old gate could not see (r2-eval, 2026-10-02). 128 px
stays as the continuity row; Fast mode had no fidelity gate at all.

Axes and the decision
---------------------
Per condition, three axes are gated against the per-icon baseline of the same condition:

* **dE00**    colour error against the artist's render, family-macro mean, margin `MARGINS`
* **turning** anchor turning per unit length, plain mean
* **ratio**   parameters against the artist's file, family-macro mean

and `self_res` is reported. For each axis, `bench/gate_stats.py` computes the relative
change of the aggregate with a paired, family-stratified bootstrap interval (Koehn 2004)
and takes a non-inferiority verdict (Lakens 2017): the change passes when the one-sided
95 % upper bound is below the margin (or nothing changed at all), and fails when the bound
reaches the margin, whether it is demonstrably worse or merely inconclusive. The report
shows the minimum detectable effect next to each verdict (Card et al. 2020), so a failure
that only says "this set cannot resolve the margin for so broad a change" reads as that.

Icons whose SVG bytes match the baseline's (SHA-256) take the baseline's numbers exactly,
so a byte-identical build is "identical" on every axis whatever platform scores it.

Baselines
---------
Per platform, because the tracer's output differs between platforms (Linux and Windows
SVGs differ on 7 of the 12 contract cases that have both hashes): one file per platform tag
under `bench/gate/baselines/<tag>.json` (`linux-x86_64`, `windows-x86_64`, `macos-arm64`),
holding every condition's per-icon numbers and SVG hashes, the scorer version and the
provenance. CI runs on Linux and needs `linux-x86_64.json`; `--artifact-dir` writes this
run's numbers in exactly that format, so a Linux baseline is adopted by committing the file
the CI job uploads.

**Re-baselining is a decision.** `--write-baseline` records the current build. A baseline is
also tightened by itself, locally and never in CI, when a run passes every gated axis and
is *demonstrably* better on at least one (the one-sided upper bound below zero: the
"better" label), which is the Ladder's rule (Blum & Hardt 2015) of moving the recorded best
only on a real gain, never on a 0.01 % wobble. In CI a demonstrable gain is reported as a
stale baseline instead, with the artifact to commit.

Legacy mode. Until a platform file exists, the gate falls back to the 2026-09-19 scalar
baseline `bench/gate/baseline.json` for quality-128ss with the old rule (the set numbers of
dE00, turning and ratio may rise 1 %, 1 % and 5 %, compared as point estimates), scores the
other conditions for information, and writes the would-be platform baseline to
`--artifact-dir`.

A failing gate can only be bypassed if the user agrees and gives a strong justification via
`--bypass-gate "<justification>"`.

The screen set's rasters and truths are committed under bench/data so this runs from a bare
checkout; only generated output (the `_gate` work folder, the cache) is ignored.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

import gate_stats  # noqa: E402
import svgeval  # noqa: E402

BASELINE = ROOT / "bench" / "gate" / "baseline.json"      # legacy scalar baseline (09-19)
BASELINE_DIR = ROOT / "bench" / "gate" / "baselines"      # per-platform, per-icon baselines
LEGACY_LIMITS = {"de00": 0.01, "turning": 0.01, "ratio": 0.05}
LEGACY_CONDITION = "quality-128ss"
#: Layout of the per-platform baseline files; bumped on an incompatible change.
BASELINE_FORMAT = 1


@dataclass(frozen=True)
class Condition:
    """One way of running the tracer over the screen set."""

    name: str
    tier: str                      # raster tier under corpus_raster (or `<tier>op`)
    args: tuple[str, ...] = ()     # tracer flags


CONDITIONS = (
    Condition("quality-128ss", "128ss"),
    Condition("quality-512ss", "512ss"),
    Condition("quality-512ssop", "512ssop"),
    Condition("fast-128ss", "128ss", ("--mode", "fast")),
    Condition("fast-512ss", "512ss", ("--mode", "fast")),
    Condition("fast-512ssop", "512ssop", ("--mode", "fast")),
)
BY_NAME = {c.name: c for c in CONDITIONS}

#: How each axis is aggregated over the set (see gate_stats.compare).
AGGREGATE = {"de00": "macro", "turning": "micro", "ratio": "macro", "self_res": "micro"}
#: The relative rise each gated axis may show at its one-sided 95 % upper bound. Chosen on
#: the replay of the 0.2.4 decisions through this rule (REPORT of agent w2-gate,
#: 2026-10-02): at 2 % every recorded 0.2.4 change passes at every condition, while at 1 %
#: the boundary-solve rewrite, a gain at 128 px and at 1024 px opaque, reads "inconclusive"
#: at 512 px (dE00 upper bound +1.37 %, 90 % interval width 5 %). Compared with the old
#: rule (point estimate within 1 %), 2 % at the upper bound is stricter for broad edits
#: (standard error ~1.5 %: the point must be below about -0.4 %) and about 0.5 point looser
#: for narrow ones (standard error ~0.3 %: the point must be below about 1.5 %).
MARGINS = {"de00": 0.02, "turning": 0.02, "ratio": 0.05}
GATED_AXES = tuple(MARGINS)
REPORTED_AXES = GATED_AXES + ("self_res",)


# --------------------------------------------------------------------------- provenance
def command_output(*args: str) -> str | None:
    """Best-effort provenance: a missing local tool must not hide a score."""
    try:
        result = subprocess.run(args, cwd=ROOT, text=True, capture_output=True, check=True)
    except (OSError, subprocess.CalledProcessError):
        return None
    return result.stdout.strip() or None


def provenance(exe: Path) -> dict[str, str]:
    """Identify the exact tracer and source snapshot behind a recorded baseline."""
    return {
        "executable_sha256": hashlib.sha256(exe.read_bytes()).hexdigest(),
        "platform": platform.platform(),
        "rustc": command_output("rustc", "--version") or "unavailable",
        "source_revision": command_output("git", "rev-parse", "HEAD") or "unavailable",
    }


def platform_tag() -> str:
    """`<os>-<arch>` naming the baseline file this machine's output is compared with:
    linux-x86_64, windows-x86_64, macos-arm64, ... (AMD64 and x86_64 are the same arch)."""
    osname = {"win32": "windows", "darwin": "macos"}.get(sys.platform, sys.platform)
    if osname.startswith("linux"):
        osname = "linux"
    arch = platform.machine().lower()
    arch = {"amd64": "x86_64", "x64": "x86_64", "aarch64": "arm64"}.get(arch, arch)
    return f"{osname}-{arch}"


def shown(path: Path) -> str:
    """A path for the console: relative to the repository when it is inside it."""
    return str(path.relative_to(ROOT)) if path.is_relative_to(ROOT) else str(path)


def in_ci() -> bool:
    return os.environ.get("CI", "").lower() in ("1", "true") or "GITHUB_ACTIONS" in os.environ


# --------------------------------------------------------------------------- scoring
def score_condition(exe: Path, cond: Condition, items: list[dict], workers: int) -> tuple[dict, list[str]]:
    """Trace and score every icon under one condition.

    Returns ({"family/stem": {"corpus", "de00", "turning", "ratio", "self_res", "sha256"}},
    failures). The score cache is off: the gate must measure this build, not remember one.
    """
    svgeval.set_tier(cond.tier)
    work = ROOT / "bench" / "data" / "_gate" / cond.name
    t0 = time.time()
    ss = svgeval.score_set(exe, cond.name, items, work, extra_args=cond.args,
                           workers=workers, use_cache=False)
    rows = {f"{i.corpus}/{i.stem}": {"corpus": i.corpus, "de00": i.de00, "turning": i.turning,
                                     "ratio": i.ratio, "self_res": i.self_res, "sha256": i.sha256}
            for i in ss.images}
    print(f"  {cond.name}: {len(rows)} icons in {time.time() - t0:.0f} s"
          + (f", {len(ss.failures)} failed" if ss.failures else ""), flush=True)
    return rows, ss.failures


def summarise(rows: dict) -> dict:
    """The set-level numbers of one condition: each axis's aggregate, plus n."""
    fam = {k: r["corpus"] for k, r in rows.items()}
    out = {}
    for ax in REPORTED_AXES:
        vals = {k: r[ax] for k, r in rows.items()}
        c = gate_stats.compare(vals, vals, fam, AGGREGATE[ax])
        out[ax] = c.base
    out["n"] = len(rows)
    return out


# --------------------------------------------------------------------------- baselines
def pack_rows(rows: dict) -> dict:
    """Per-icon rows as compact lists, [de00, turning, ratio, self_res, sha256], for the file."""
    return {k: [r["de00"], r["turning"], r["ratio"], r["self_res"], r["sha256"]]
            for k, r in sorted(rows.items())}


def unpack_rows(packed: dict) -> dict:
    return {k: {"corpus": k.split("/", 1)[0], "de00": v[0], "turning": v[1], "ratio": v[2],
                "self_res": v[3], "sha256": v[4]} for k, v in packed.items()}


def baseline_document(exe: Path, results: dict) -> dict:
    """A per-platform baseline: every scored condition's summary and per-icon rows."""
    return {
        "_note": ("Per-icon regression-gate baseline written by bench/ci_gate.py. Recording "
                  "a new one is a decision: say why in the commit that does it."),
        "format": BASELINE_FORMAT,
        "scorer_version": svgeval.SCORER_VERSION,
        "platform": platform_tag(),
        "_provenance": provenance(exe),
        "conditions": {
            name: {"tier": BY_NAME[name].tier, "args": list(BY_NAME[name].args),
                   "summary": summarise(rows), "icons": pack_rows(rows)}
            for name, rows in results.items()
        },
    }


def write_json(path: Path, doc: dict) -> None:
    """Write `doc` as JSON with LF line ends on every platform (the repository is LF)."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(doc, indent=1, sort_keys=False) + "\n", encoding="utf-8", newline="\n")


# --------------------------------------------------------------------------- comparison
def compare_condition(base_rows: dict, cur_rows: dict, margins: dict) -> dict:
    """Every reported axis of one condition, compared icon by icon.

    Icons whose SVG is byte-identical to the baseline's take the baseline's values, so
    platform-level float noise in the scorer can never turn an unchanged icon into a delta.
    Returns {axis: (Comparison, Verdict)}; self_res is never gated (infinite margin)."""
    fam = {k: r["corpus"] for k, r in base_rows.items()}
    cur = {}
    for k, r in cur_rows.items():
        b = base_rows.get(k)
        cur[k] = b if b is not None and b["sha256"] and b["sha256"] == r["sha256"] else r
    out = {}
    for ax in REPORTED_AXES:
        c = gate_stats.compare({k: r[ax] for k, r in base_rows.items()},
                               {k: r[ax] for k, r in cur.items()}, fam, AGGREGATE[ax])
        out[ax] = (c, gate_stats.decide(c, margins.get(ax, float("inf"))))
    return out


def legacy_check(base: dict, now: dict) -> tuple[list[str], list[str], dict]:
    """The 2026-09-19 rule, kept verbatim for the fallback: each axis's set number may rise
    by LEGACY_LIMITS of the baseline; anything 0.01 % better tightens the baseline."""
    failures, gains = [], []
    tightened = dict(base)
    for k, lim in LEGACY_LIMITS.items():
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
    return failures, gains, tightened


# --------------------------------------------------------------------------- report
def fmt_row(cond: str, ax: str, c: gate_stats.Comparison, v: gate_stats.Verdict) -> str:
    """One line of the console report: levels, change, interval, bound against margin,
    MDE, how many icons moved, verdict (upper case when it fails the gate)."""
    gated = ax in GATED_AXES
    margin = f"{v.margin * 100:.0f}%" if gated else "-"
    return (f"  {cond:16s} {ax:9s} {c.base:9.5f} -> {c.cur:9.5f}  {c.rel * 100:+7.2f}%  "
            f"95% CI [{c.p025 * 100:+6.2f}, {c.p975 * 100:+6.2f}]  upper {c.p95 * 100:+6.2f}% "
            f"vs {margin:>3s}  MDE {c.mde * 100:5.2f}%  "
            f"changed {c.changed:3d} (better {c.better}, worse {c.worse})  "
            f"{v.label.upper() if gated and not v.passed else v.label if gated else 'reported'}")


def markdown_summary(verdicts: dict, legacy: bool) -> str:
    lines = ["## Regression gate", "",
             "Legacy mode: quality-128ss judged by the 2026-09-19 scalar baseline; the other "
             "conditions are informational." if legacy else
             "Paired, family-stratified bootstrap against the per-icon baseline; non-inferiority "
             "at the one-sided 95 % upper bound.", "",
             "| condition | axis | baseline | now | change | 95 % CI | upper bound | margin | MDE | changed | verdict |",
             "|---|---|---|---|---|---|---|---|---|---|---|"]
    for cond, axes in verdicts.items():
        for ax, (c, v) in axes.items():
            m = f"{v.margin * 100:.0f} %" if ax in GATED_AXES and v.margin != float("inf") else "-"
            lines.append(f"| {cond} | {ax} | {c.base:.5f} | {c.cur:.5f} | {c.rel * 100:+.2f} % | "
                         f"[{c.p025 * 100:+.2f}, {c.p975 * 100:+.2f}] % | {c.p95 * 100:+.2f} % | {m} | "
                         f"{c.mde * 100:.2f} % | {c.changed} | {v.label} |")
    return "\n".join(lines) + "\n"


def levels_markdown(doc: dict, failures: list[str]) -> str:
    """Legacy mode's job summary: each condition's set numbers, no comparison."""
    lines = ["## Regression gate (legacy mode)", "",
             f"No per-icon baseline for this platform: {LEGACY_CONDITION} was judged by the "
             "2026-09-19 scalar baseline, the rest are levels only. Commit the uploaded "
             "`gate-baseline` artifact as `bench/gate/baselines/<platform>.json` to switch to "
             "the paired non-inferiority rule.", "",
             "| condition | " + " | ".join(REPORTED_AXES) + " | n |",
             "|---|" + "---|" * (len(REPORTED_AXES) + 1)]
    for name, c in doc["conditions"].items():
        s = c["summary"]
        lines.append(f"| {name} | " + " | ".join(f"{s[ax]:.5f}" for ax in REPORTED_AXES) + f" | {s['n']} |")
    if failures:
        lines += ["", "Failures:", ""] + [f"- {f}" for f in failures]
    return "\n".join(lines) + "\n"


def verdict_json(verdicts: dict) -> dict:
    return {cond: {ax: {**c.__dict__, "mde": c.mde, "verdict": v.label, "passed": v.passed,
                        "margin": v.margin if v.margin != float("inf") else None}
                   for ax, (c, v) in axes.items()} for cond, axes in verdicts.items()}


# --------------------------------------------------------------------------- main
def parse_margins(specs: list[str]) -> dict:
    margins = dict(MARGINS)
    for s in specs:
        ax, _, val = s.partition("=")
        if ax not in MARGINS or not val:
            raise SystemExit(f"--margin takes AXIS=FRACTION with AXIS one of {', '.join(MARGINS)}")
        margins[ax] = float(val)
    return margins


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--exe", required=True)
    ap.add_argument("--workers", type=int, default=max(1, (os.cpu_count() or 4) // 2))
    ap.add_argument("--conditions", default="",
                    help=f"comma-separated subset of: {', '.join(BY_NAME)} (default: all)")
    ap.add_argument("--baseline-file", type=Path, default=None,
                    help="per-icon baseline to compare with / write (default: "
                         "bench/gate/baselines/<platform>.json)")
    ap.add_argument("--write-baseline", "--update", action="store_true",
                    help="record this build as the baseline (a decision; say why in the commit)")
    ap.add_argument("--no-tighten", action="store_true",
                    help="never rewrite a baseline on a gain (CI never does)")
    ap.add_argument("--margin", action="append", default=[], metavar="AXIS=FRACTION",
                    help="override a gated axis's margin, e.g. de00=0.01 (for analysis)")
    ap.add_argument("--report-json", type=Path, default=None, help="write the full comparison here")
    ap.add_argument("--artifact-dir", type=Path, default=None,
                    help="write this run as a ready-to-commit baseline <dir>/<platform>.json")
    ap.add_argument("--sample", type=float, default=None, metavar="FRACTION",
                    help="score a family-stratified sample of the set (a quick local read, "
                         "never a verdict to merge on; cannot write a baseline)")
    ap.add_argument("--bypass-gate", type=str, default=None, metavar="JUSTIFICATION",
                    help="Bypass a failing gate if the user agreed and gives a strong justification")
    a = ap.parse_args()
    if a.sample is not None and a.write_baseline:
        ap.error("--sample cannot write a baseline: a baseline holds the whole set")

    if a.bypass_gate is not None and len(a.bypass_gate.strip()) < 10:
        print("ERROR: --bypass-gate requires a substantial justification (at least 10 characters) "
              "explaining user approval.", file=sys.stderr)
        return 1
    margins = parse_margins(a.margin)
    names = [s.strip() for s in a.conditions.split(",") if s.strip()] or list(BY_NAME)
    unknown = [n for n in names if n not in BY_NAME]
    if unknown:
        print(f"ERROR: unknown condition(s) {', '.join(unknown)}; known: {', '.join(BY_NAME)}",
              file=sys.stderr)
        return 2

    # DISTS is not an axis of the gate (it is blind to boundary placement by design, which
    # is most of what this engine changes; r2-eval P4), so do not load torch and a VGG for it.
    os.environ.setdefault("INKVEC_SKIP_DISTS", "1")
    exe = Path(a.exe).resolve()
    tag = platform_tag()
    bfile = a.baseline_file or BASELINE_DIR / f"{tag}.json"
    base_doc = None
    if bfile.exists() and not a.write_baseline:
        base_doc = json.loads(bfile.read_text(encoding="utf-8"))
        if base_doc.get("format") != BASELINE_FORMAT or base_doc.get("scorer_version") != svgeval.SCORER_VERSION:
            print(f"FAIL: {bfile.name} was written by baseline format {base_doc.get('format')} / "
                  f"scorer version {base_doc.get('scorer_version')}; this gate is format "
                  f"{BASELINE_FORMAT} / scorer {svgeval.SCORER_VERSION}. Its numbers are not "
                  "comparable: re-record it with --write-baseline (a decision).")
            return 1
    legacy = base_doc is None and not a.write_baseline

    items = svgeval.load_sets()["screen"]
    if a.sample is not None:
        items = svgeval.sample(items, a.sample)
    print(f"gate: {len(items)} icons x {len(names)} conditions, platform {tag}, "
          + ("writing a baseline" if a.write_baseline else
             f"baseline {shown(bfile)}"
             if base_doc else f"legacy mode ({shown(BASELINE)})"), flush=True)
    results, failures = {}, []
    for name in names:
        rows, fails = score_condition(exe, BY_NAME[name], items, a.workers)
        results[name] = rows
        if len(rows) < len(items) - 2:
            failures.append(f"{name}: only {len(rows)} of {len(items)} icons scored "
                            f"({'; '.join(fails[:3])})")

    doc = baseline_document(exe, results)
    if a.artifact_dir:
        write_json(a.artifact_dir / f"{tag}.json", doc)
        print(f"wrote this run as {a.artifact_dir / (tag + '.json')}")
    if failures:
        print("\nregression gate FAILED: icons failed to trace or score\n")
        for f in failures:
            print(f"  {f}")
        return 1
    if a.write_baseline:
        write_json(bfile, doc)
        print(f"wrote {bfile}")
        return 0

    verdicts: dict = {}
    gate_fail: list[str] = []
    legacy_tightened = None
    if legacy:
        print(f"\nno per-icon baseline at {shown(bfile)}: "
              f"legacy rule on {LEGACY_CONDITION}, the rest informational")
        for name, rows in results.items():
            s = doc["conditions"][name]["summary"]
            print(f"  {name:16s} " + "  ".join(f"{ax} {s[ax]:.5f}" for ax in REPORTED_AXES))
        if LEGACY_CONDITION in results:
            if not BASELINE.exists():
                print("no baseline; run with --write-baseline")
                return 1
            base = json.loads(BASELINE.read_text(encoding="utf-8"))
            now = doc["conditions"][LEGACY_CONDITION]["summary"]
            print(f"\n{LEGACY_CONDITION} against {shown(BASELINE)}:")
            fails, gains, tightened = legacy_check(base, now)
            gate_fail += fails
            if gains and not fails:
                tightened.update(n=now["n"], self_res=now["self_res"], _provenance=provenance(exe))
                tightened.pop("_bypass_justification", None)
                legacy_tightened = (tightened, gains)
    else:
        print()
        for name, rows in results.items():
            cond_base = base_doc["conditions"].get(name)
            if cond_base is None:
                print(f"  {name}: not in the baseline, informational only")
                continue
            base_rows = unpack_rows(cond_base["icons"])
            missing = sorted(set(base_rows) - set(rows))
            if len(missing) > 2 and a.sample is None:
                gate_fail.append(f"{name}: {len(missing)} baseline icons were not scored")
            verdicts[name] = compare_condition(base_rows, rows, margins)
            for ax, (c, v) in verdicts[name].items():
                print(fmt_row(name, ax, c, v))
                if ax in GATED_AXES and not v.passed:
                    gate_fail.append(f"{name} {ax}: {v.label} (change {c.rel * 100:+.2f}%, "
                                     f"upper bound {c.p95 * 100:+.2f}% vs margin {v.margin * 100:.0f}%, "
                                     f"MDE {c.mde * 100:.2f}%)")

    if a.report_json:
        write_json(a.report_json, {"platform": tag, "legacy": legacy, "failures": gate_fail,
                                   "summaries": {n: d["summary"] for n, d in doc["conditions"].items()},
                                   "verdicts": verdict_json(verdicts)})
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as fh:
            fh.write(markdown_summary(verdicts, legacy) if verdicts else levels_markdown(doc, gate_fail))

    if gate_fail:
        if a.bypass_gate:
            print("\n======================================================================")
            print("  *** QUALITY GATE BYPASSED BY USER AGREEMENT ***")
            print(f"  Justification: {a.bypass_gate.strip()}")
            print("======================================================================")
            for f in gate_fail:
                print(f"  bypassed failure: {f}")
            return 0
        print("\nregression gate FAILED:\n")
        for f in gate_fail:
            print(f"  {f}")
        print("\nA change must show it is not worse. 'inconclusive' means the interval reaches the "
              f"margin:\nthe change is too broad for {len(items)} icons to resolve (compare the MDE). "
              "This can only be\nbypassed if the user explicitly agrees and provides strong justification:")
        print('  python bench/ci_gate.py --exe ... --bypass-gate "<strong justification approved by user>"')
        return 1

    if legacy:
        if legacy_tightened and not a.no_tighten and not in_ci() and a.sample is None:
            tightened, gains = legacy_tightened
            # Written exactly as the 09-19 gate wrote it, so the legacy file's diff stays small.
            BASELINE.write_text(json.dumps(tightened, indent=2), encoding="utf-8")
            print("\nlegacy ratchet passed, baseline auto-tightened to match:")
            for g in gains:
                print(f"  better  {g}")
        else:
            print("\nregression gate PASSED (legacy rule)")
        return 0

    better = [f"{n} {ax}" for n, axes in verdicts.items() for ax, (_, v) in axes.items()
              if ax in GATED_AXES and v.label == "better"]
    if better:
        msg = ("demonstrable gain on " + ", ".join(better) + ": the baseline is stale. "
               "Commit this run's numbers (the --artifact-dir file) so the gain cannot be given back.")
        if (in_ci() or a.no_tighten or a.baseline_file is not None or a.sample is not None
                or set(results) != set(base_doc["conditions"])):
            print(f"\n::warning title=regression gate::{msg}" if in_ci() else f"\nnote: {msg}")
        else:
            write_json(bfile, doc)
            print(f"\n{msg}\nbaseline tightened: wrote {bfile} (commit it with the change)")
    print("\nregression gate PASSED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
