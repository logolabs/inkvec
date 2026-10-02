"""Small isolated cases, one property each, in under a minute.

    python bench/cases.py --exe target/release/inkvec.exe
    python bench/cases.py --only ribbon_w1 --verbose
    python bench/cases.py --json out/cases.json
    python bench/cases.py --exe ... -- --mode fast          # tracer flags for every case
    python bench/cases.py --exe ... --ratchet quality       # CI: hold the passing set
    python bench/cases.py --exe ... --ratchet fast -- --mode fast

The 980-icon run takes 35 minutes and answers one question: is the mean better. It cannot
say *what* broke, because every icon mixes edges, corners, thin strokes, junctions and
gradients, and the score adds them up. So a defect is found late, patched narrowly, and
the algorithm grows a new special case.

Each case here holds one property still. The input is a small SVG written by hand,
rasterised exactly the way the corpus is (8x supersampled, box-averaged in premultiplied
alpha), so the truth is geometry we know rather than a number we recorded. Thresholds are
set at what a correct tracer should achieve — not at what this one currently does — so a
case may fail on the day it is written. That is the point: a failing case is a named
defect with a number attached, and the number is the same one that has to move.

Adding a case: drop a module in `bench/cases/` exporting `CASES: list[Case]`. See
`bench/cases/_common.py` for the measurement primitives and the two conventions (output
coordinate frame; ink versus coverage) that are easy to get wrong.

Tracer flags. Everything after `--` is passed to the tracer for every case, after the case's
own flags, so the same suite runs in Fast mode (`-- --mode fast`) or under any option.

The ratchet. Because cases are written to fail until the tracer is right, "all pass" cannot
be a CI rule; "nothing that passed stops passing" can. `--ratchet NAME` compares the set of
passing cases with the set recorded under `cases:NAME` in `bench/quality_budget.json`:

* a recorded case that now fails, fails the run (a named regression);
* a case that newly passes is added to the record, locally (commit the budget with the work
  that earned it); in CI it is only reported, since the runner's copy is thrown away;
* a recorded case that no longer exists in the suite is reported and dropped on the next
  tightening, so renaming a case is a visible decision;
* no record at all fails, so the ratchet cannot silently enforce nothing.

`--ratchet-update` records the current passing set as it is, a decision for the commit
message. This is the same budget discipline as `bench/quality.py`'s `untested_modules` and
`unused_dependencies`, applied to behaviour: a set that may only grow.
"""
from __future__ import annotations

import argparse
import importlib
import json
import math
import shutil
import sys
import tempfile
import time
import traceback
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CASE_DIR = ROOT / "bench" / "cases"
BUDGET = ROOT / "bench" / "quality_budget.json"
# The package directory and this script share a name; importing the case modules by
# putting their own directory first keeps `import _common` working inside them and keeps
# this file out of the way.
sys.path.insert(0, str(CASE_DIR))

from _common import Case  # noqa: E402


def discover() -> tuple[list[Case], list[str]]:
    """Every module's CASES, plus any NOTES it left about cases it could not build."""
    cases: list[Case] = []
    notes: list[str] = []
    for path in sorted(CASE_DIR.glob("*.py")):
        if path.stem.startswith("_"):
            continue
        mod = importlib.import_module(path.stem)
        cases.extend(getattr(mod, "CASES", []))
        notes.extend(getattr(mod, "NOTES", []))
    return cases, notes


def run_case(case: Case, exe: Path, work: Path, extra: tuple[str, ...] = ()) -> dict:
    """Rasterise, trace (with the case's flags, then `extra`) and check one case. Never
    raises: a crash in the tracer or a check is the row's `error`, and the row fails."""
    from _common import Traced

    t0 = time.time()
    row = {"name": case.name, "what": case.what, "checks": []}
    try:
        traced = Traced(case, exe, work, extra)
        if traced.rc != 0 or not traced.out_svg:
            row["error"] = f"trace failed (rc={traced.rc}) {traced.stderr}"
        else:
            row["checks"] = [c.__dict__ | {"ok": c.ok, "margin": c.margin}
                             for c in case.check(traced)]
            row["n_shapes"] = len(traced.out_shapes)
            row["n_segments"] = traced.out_segments
    except Exception:
        row["error"] = traceback.format_exc(limit=3)
    row["seconds"] = round(time.time() - t0, 2)
    row["pass"] = "error" not in row and bool(row["checks"]) and all(c["ok"] for c in row["checks"])
    return row


def governing(row: dict) -> dict | None:
    """The check worth printing: the worst failure, else the case's own headline check.

    Cases list their checks in the order the author thinks a reader should hear them, so
    a passing case shows its first; a failing one shows whichever is furthest past its
    limit, which is the one that names the defect.
    """
    checks = row.get("checks") or []
    if not checks:
        return None
    bad = [c for c in checks if not c["ok"]]
    if not bad:
        return checks[0]
    return max(bad, key=lambda c: c["margin"] if math.isfinite(c["margin"]) else 1e9)


def fmt(v) -> str:
    if isinstance(v, float):
        if not math.isfinite(v):
            return "nan"
        return f"{v:.4g}"
    return str(v)


def in_ci() -> bool:
    import os
    return os.environ.get("CI", "").lower() in ("1", "true") or "GITHUB_ACTIONS" in os.environ


def platform_tag() -> str:
    """`<os>-<arch>`, as bench/ci_gate.py names its baselines."""
    import platform
    osname = {"win32": "windows", "darwin": "macos"}.get(sys.platform, sys.platform)
    osname = "linux" if osname.startswith("linux") else osname
    arch = platform.machine().lower()
    arch = {"amd64": "x86_64", "x64": "x86_64", "aarch64": "arm64"}.get(arch, arch)
    return f"{osname}-{arch}"


def ratchet_key(name: str, budget: dict) -> str:
    """The budget key holding the passing set: `cases:NAME:<platform>` when this platform has
    its own record (tracer output differs between platforms, so a borderline case can too),
    else the shared `cases:NAME`."""
    own = f"cases:{name}:{platform_tag()}"
    return own if own in budget else f"cases:{name}"


def ratchet(name: str, rows: list[dict], *, update: bool, tighten: bool,
            budget_path: Path = BUDGET) -> int:
    """Hold the set of passing cases: see the module docstring. Returns the exit code.

    Only the cases in `rows` are judged, so `--only` narrows the check rather than reading
    every unselected case as a regression; tightening and update need the whole suite.
    """
    budget = json.loads(budget_path.read_text(encoding="utf-8")) if budget_path.exists() else {}
    key = ratchet_key(name, budget)
    passing = sorted(r["name"] for r in rows if r["pass"])
    ran = {r["name"] for r in rows}
    old = budget.get(key)

    def save(values: list[str]) -> None:
        budget[key] = values
        budget_path.write_text(json.dumps(budget, indent=2, sort_keys=True) + "\n",
                               encoding="utf-8", newline="\n")

    if update:
        save(passing)
        print(f"\nratchet {key}: recorded {len(passing)} passing cases (a decision: say why "
              "in the commit)")
        return 0
    if old is None:
        print(f"\nratchet FAILED: no passing set recorded under {key} in "
              f"{budget_path.name}; record one with --ratchet-update (a decision)")
        return 1
    regressed = sorted(n for n in old if n in ran and n not in passing)
    gained = sorted(n for n in passing if n not in old)
    retired = sorted(n for n in old if n not in ran) if tighten else []
    for n in retired:
        print(f"  retired  {n} (recorded as passing, no longer in the suite)")
    if regressed:
        print(f"\nratchet {key} FAILED: {len(regressed)} recorded case(s) stopped passing:")
        for n in regressed:
            print(f"  - {n}")
        print("Fix the regression, or re-record with --ratchet-update and say why in the commit.")
        return 1
    if gained or retired:
        for n in gained:
            print(f"  newly passing  {n}")
        if tighten:
            save(sorted((set(old) - set(retired)) | set(gained)))
            print(f"ratchet {key} tightened: commit {budget_path.name} with the change")
        else:
            print(f"ratchet {key}: not tightened here (CI, --only, or --no-tighten)")
    print(f"ratchet {key} passed: {len(passing)} passing, {len(old)} recorded")
    return 0


def main() -> int:
    argv = sys.argv[1:]
    extra: tuple[str, ...] = ()
    if "--" in argv:
        cut = argv.index("--")
        argv, extra = argv[:cut], tuple(argv[cut + 1:])
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--exe", default=str(ROOT / "target/release/inkvec.exe"))
    ap.add_argument("--only", action="append", default=[],
                    help="substring of a case name; repeatable")
    ap.add_argument("--json", dest="json_out")
    ap.add_argument("--jobs", type=int, default=4,
                    help="cases in flight at once; each is a subprocess plus renders")
    ap.add_argument("--keep", metavar="DIR",
                    help="keep the rasters and traced SVGs here instead of a temp dir")
    ap.add_argument("--verbose", action="store_true", help="print every check, not one")
    ap.add_argument("--ratchet", metavar="NAME",
                    help="hold the passing set recorded as cases:NAME in bench/quality_budget.json")
    ap.add_argument("--ratchet-update", action="store_true",
                    help="record the current passing set under --ratchet NAME (a decision)")
    ap.add_argument("--no-tighten", action="store_true",
                    help="report newly passing cases without recording them")
    a = ap.parse_args(argv)
    if a.ratchet_update and (not a.ratchet or a.only):
        ap.error("--ratchet-update needs --ratchet NAME and the whole suite (no --only)")

    exe = Path(a.exe).resolve()
    if not exe.exists():
        print(f"no tracer at {exe}; build with: cargo build --release", file=sys.stderr)
        return 2

    cases, notes = discover()
    if a.only:
        cases = [c for c in cases if any(s in c.name for s in a.only)]
    if not cases:
        print("no cases matched", file=sys.stderr)
        return 2

    work = Path(a.keep) if a.keep else Path(tempfile.mkdtemp(prefix="inkvec_cases_"))
    work.mkdir(parents=True, exist_ok=True)
    t0 = time.time()
    try:
        with ThreadPoolExecutor(max(1, a.jobs)) as ex:
            rows = list(ex.map(lambda c: run_case(c, exe, work, extra), cases))
    finally:
        if not a.keep:
            shutil.rmtree(work, ignore_errors=True)

    width = max(len(r["name"]) for r in rows)
    for row in rows:
        verdict = "pass" if row["pass"] else "FAIL"
        g = governing(row)
        if g is None:
            detail = row.get("error", "no checks").splitlines()[-1][:90]
        else:
            detail = f"{g['name']} {fmt(g['value'])} {g['op']} {fmt(g['limit'])}"
        print(f"{row['name']:<{width}}  {verdict}  {detail}")
        if a.verbose and row.get("checks"):
            for c in row["checks"]:
                mark = " " if c["ok"] else "!"
                print(f"{'':<{width}}   {mark} {c['name']:<20} {fmt(c['value']):>10} "
                      f"{c['op']} {fmt(c['limit']):<8} {c['note']}")

    failed = [r["name"] for r in rows if not r["pass"]]
    print(f"\n{len(rows) - len(failed)}/{len(rows)} pass in {time.time() - t0:.1f}s"
          + (f" (tracer flags: {' '.join(extra)})" if extra else ""))
    if failed:
        print("failing: " + " ".join(failed))
    for n in notes:
        print(f"skipped: {n}")

    if a.json_out:
        out = Path(a.json_out)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(
            {"exe": str(exe), "args": list(extra), "seconds": round(time.time() - t0, 1),
             "cases": rows}, indent=1), encoding="utf-8")
        print(f"wrote {out}")
    if a.ratchet:
        return ratchet(a.ratchet, rows, update=a.ratchet_update,
                       tighten=not (a.no_tighten or a.only or in_ci()))
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
