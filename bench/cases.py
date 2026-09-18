"""Small isolated cases, one property each, in under a minute.

    python bench/cases.py --exe target/release/inkvec.exe
    python bench/cases.py --only ribbon_w1 --verbose
    python bench/cases.py --json out/cases.json

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


def run_case(case: Case, exe: Path, work: Path) -> dict:
    from _common import Traced

    t0 = time.time()
    row = {"name": case.name, "what": case.what, "checks": []}
    try:
        traced = Traced(case, exe, work)
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


def main() -> int:
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
    a = ap.parse_args()

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
            rows = list(ex.map(lambda c: run_case(c, exe, work), cases))
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
    print(f"\n{len(rows) - len(failed)}/{len(rows)} pass in {time.time() - t0:.1f}s")
    if failed:
        print("failing: " + " ".join(failed))
    for n in notes:
        print(f"skipped: {n}")

    if a.json_out:
        out = Path(a.json_out)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(
            {"exe": str(exe), "seconds": round(time.time() - t0, 1), "cases": rows},
            indent=1), encoding="utf-8")
        print(f"wrote {out}")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
