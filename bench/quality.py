"""Code quality ratchet: hold the line today, tighten it deliberately.

**This file does not analyse anything.** `cargo clippy`, `rustc`'s own lints and
`cargo fmt` do that, configured under `[workspace.lints]` in the workspace `Cargo.toml`.
What this adds is the part they do not have: a budget that grandfathers existing debt so a
warning count can fall but never rise.

That division of labour was learned the hard way. This file used to carry three
hand-written analyses -- a brace counter for long functions, a regex for undocumented
public items, and a token counter for constants nothing reads. Every one was a worse copy
of a lint that already existed, and measurably so: the regex found 81 undocumented items
where `missing_docs` finds 230, because it never looked at struct fields or methods. They
were deleted.

The same lesson applies to the one gap the lints genuinely leave. A `pub const` that
nothing in the workspace reads is invisible to `dead_code`, which assumes a public item has
callers it cannot see. The fix is not a scanner: it is to stop marking internal constants
`pub`, after which `dead_code` reports them immediately and for free.

Why a ratchet and not a threshold. The codebase carries a heavy tail -- 34 functions clippy
considers too long, 230 undocumented items -- so a hard rule would fail on day one and get
switched off, while a bare pass/fail on the total lets a new violation in as soon as an old
one is fixed. Recording today's count per lint means each one has to be paid down
separately: documenting things cannot buy credit for a longer function.

    python bench/quality.py             # check; exit 1 on regression
    python bench/quality.py --coverage  # ... plus per-crate line-coverage floors (CI runs this)
    python bench/quality.py --report    # every metric, no gate
    python bench/quality.py --update    # re-baseline, after a deliberate decision

Two measurements are too slow for every run and are opt-in:

* `--coverage` runs the whole test suite under `cargo llvm-cov` (about as long as the
  release test run itself) and holds each crate's line coverage to a floor. CI's quality
  job runs it.
* `--mutants` runs `cargo mutants` on the engine's core files (`MUTANT_FILES`) over a
  fixed systematic sample (`MUTANT_SHARD`) and holds each file's kill rate to a floor. It
  takes about two hours at four jobs, so it is a weekly job
  (`.github/workflows/mutation.yml`) and a manual check before merging test or engine
  work on those files; `--mutants-from DIR` scores an existing cargo-mutants output
  directory instead of running one. Coverage says a line ran; the kill rate says a test
  would notice if it were wrong, which is the stronger claim and the one that was found
  missing (a 41 % kill rate under 81 % line coverage, 2026-09-26).

Floors (`coverage:*`, `mutation:*`) are whole percentages, set at the measured value
rounded down, and rise the same way when a measurement clears the next integer.

`--update` is the only way a budget loosens, and it prints what it loosened so the change
is visible in review. Improvements are absorbed automatically: a metric that got better
tightens the budget without being asked, so ground gained is never given back.

To see *where* the warnings are, run clippy directly. This only counts them.
"""

from __future__ import annotations

import argparse
import collections
import json
import math
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUDGET = ROOT / "bench" / "quality_budget.json"
CARGO = "cargo"



# ---------------------------------------------------------------- source scanning

def rust_files(include_tests: bool = False) -> list[Path]:
    out = []
    for p in (ROOT / "crates").rglob("*.rs"):
        s = p.as_posix()
        if "/target/" in s:
            continue
        if not include_tests and "/tests/" in s:
            continue
        out.append(p)
    return sorted(out)


def lint_counts() -> dict[str, int]:
    """Warnings per lint, from clippy's JSON output.

    This used to be three hand-written scanners -- a brace counter for long functions, a
    regex for undocumented public items, and a token counter for unread constants. All
    three were worse than the tools that already exist, and measurably so: the regex found
    81 undocumented items where `missing_docs` finds 230, because it never looked at struct
    fields or methods. Analysis belongs to the compiler; what this file adds is the ratchet
    on top of it.

    The lints themselves are configured in the workspace `Cargo.toml` under
    `[workspace.lints]`, so `cargo clippy` and an editor agree about what counts.
    """
    out = cargo("clippy", "--release", "--workspace", "--message-format=json")
    counts: dict[str, int] = {}
    for line in out.splitlines():
        try:
            m = json.loads(line)
        except json.JSONDecodeError:
            continue
        if m.get("reason") != "compiler-message":
            continue
        msg = m["message"]
        if msg.get("level") not in ("warning", "error"):
            continue
        code = (msg.get("code") or {}).get("code") or "uncoded"
        counts[code] = counts.get(code, 0) + 1
    return counts


# ---------------------------------------------------------------- coverage

# A crate with fewer instrumented lines than this is too small for a percentage to mean
# anything (inkvec-py without its `python` feature, say); it gets no floor.
COVERAGE_MIN_LINES = 100


def is_test_source(rel: str) -> bool:
    """Test-only code: integration tests and child `tests` modules under `src/`.

    They are covered by construction, so counting them would let a floor be met by
    writing tests about nothing. Inline `#[cfg(test)]` blocks cannot be told apart from
    the code around them and are counted; they are small.
    """
    name = rel.rsplit("/", 1)[-1]
    return "/tests/" in rel or name == "tests.rs" or name.endswith("_tests.rs")


def coverage_by_crate() -> dict[str, float]:
    """Line coverage of each workspace crate's `src/`, over the whole test suite.

    This replaced a proxy, `untested_modules` (a module over 200 lines with no `#[test]`
    anywhere), that one trivial test could clear for a 2000-line file. `cargo llvm-cov`
    measures what actually ran. The profile matches the one the tests use in CI (release),
    without LTO: instrumenting every crate is slow enough without a fat link on top.
    """
    env = dict(os.environ, CARGO_PROFILE_RELEASE_LTO="false",
               CARGO_PROFILE_RELEASE_CODEGEN_UNITS="16")
    with tempfile.TemporaryDirectory() as tmp:
        summary = Path(tmp) / "summary.json"
        cargo("llvm-cov", "--workspace", "--release", "--json", "--summary-only",
              "--output-path", str(summary), env=env)
        data = json.loads(summary.read_text(encoding="utf-8"))["data"][0]
    lines: dict[str, list[int]] = collections.defaultdict(lambda: [0, 0])
    for f in data["files"]:
        rel = Path(f["filename"]).resolve().as_posix()
        root = ROOT.as_posix()
        if not rel.startswith(root + "/crates/"):
            continue
        rel = rel[len(root) + 1:]
        parts = rel.split("/")
        if len(parts) < 4 or parts[2] != "src" or is_test_source(rel):
            continue
        c = lines[parts[1]]
        c[0] += f["summary"]["lines"]["covered"]
        c[1] += f["summary"]["lines"]["count"]
    return {f"coverage:{crate}": round(100.0 * cov / n, 2)
            for crate, (cov, n) in sorted(lines.items()) if n >= COVERAGE_MIN_LINES}


# ---------------------------------------------------------------- mutation

# The engine's core: palette and colour science, the native-alpha palette, the boundary
# solve, gradient fitting and the segment DP. Chosen by the 2026-09-26 audit as the code
# whose arithmetic decides the output and whose tests least pinned it.
MUTANT_FILES = [
    "crates/inkvec-trace/src/color.rs",
    "crates/inkvec-trace/src/native.rs",
    "crates/inkvec-trace/src/boundary_opt.rs",
    "crates/inkvec-trace/src/gradient.rs",
    "crates/inkvec-fit/src/multimodel.rs",
]
# Every 16th mutant, round-robin: a systematic sample of about 300 of the ~4,700, which
# is what two hours buys. The floors in the budget were measured on this sample.
MUTANT_SHARD = "0/16"


def run_mutants(out: Path, shard: str = MUTANT_SHARD) -> Path:
    """Run cargo-mutants the way the floors were measured; returns its output directory."""
    env = dict(os.environ, CARGO_PROFILE_RELEASE_LTO="false",
               CARGO_PROFILE_RELEASE_CODEGEN_UNITS="16", CARGO_PROFILE_RELEASE_INCREMENTAL="true",
               CARGO_BUILD_JOBS="1")
    env.pop("CARGO_TARGET_DIR", None)  # each job builds in its own copy of the tree
    args = ["mutants", "--profile", "release", "-j", "4", "--minimum-test-timeout", "90",
            "--no-times", "-o", str(out), "--shard", shard, "--sharding", "round-robin"]
    for f in MUTANT_FILES:
        args += ["-f", f]
    try:
        r = subprocess.run([CARGO, *args], cwd=ROOT, env=env)
    except OSError as exc:
        raise RuntimeError(f"could not run cargo mutants: {exc}") from exc
    # 0: all caught; 2: some missed; 3: some timed out. Anything else is not a measurement.
    if r.returncode not in (0, 2, 3):
        raise RuntimeError(f"cargo mutants failed ({r.returncode})")
    return out / "mutants.out"


def mutation_scores(mdir: Path) -> dict[str, float]:
    """Kill rate per core file from a cargo-mutants output directory: caught plus timed
    out, over every viable mutant (unviable ones never compiled and say nothing)."""
    f = mdir / "outcomes.json"
    if not f.exists():
        raise RuntimeError(f"{f} missing: not a cargo-mutants output directory")
    per: dict[str, collections.Counter] = collections.defaultdict(collections.Counter)
    for o in json.loads(f.read_text(encoding="utf-8"))["outcomes"]:
        if o.get("scenario") == "Baseline":
            continue
        per[o["scenario"]["Mutant"]["file"].replace("\\", "/")][o.get("summary")] += 1
    out = {}
    for file in MUTANT_FILES:
        c = per.get(file)
        if not c:
            continue
        killed = c["CaughtMutant"] + c["Timeout"]
        viable = killed + c["MissedMutant"]
        if viable:
            out[f"mutation:{file}"] = round(100.0 * killed / viable, 2)
    return out


# ---------------------------------------------------------------- dependencies

def unused_dependencies() -> list[str]:
    """Dependencies no source file names, by `cargo machete`, as `crate: dep`.

    Every manifest under the repository is scanned, Studio's included. A dependency that is
    used in a way machete cannot see (a build script, generated code, a feature forwarded
    to it) is listed under `[package.metadata.cargo-machete] ignored` in its manifest, with
    the reason beside it.
    """
    try:
        r = subprocess.run([CARGO, "machete"], cwd=ROOT, capture_output=True, text=True)
    except OSError as exc:
        raise RuntimeError(f"could not run cargo machete: {exc}") from exc
    if r.returncode not in (0, 1) or "cargo-machete" not in r.stdout + r.stderr:
        raise RuntimeError(f"cargo machete failed ({r.returncode}):\n{r.stdout}{r.stderr}")
    found, crate = [], None
    for line in r.stdout.splitlines():
        m = re.match(r"^(\S+) -- ", line)
        if m:
            crate = m.group(1)
        elif crate and line.startswith("\t"):
            found.append(f"{crate}: {line.strip()}")
    if r.returncode == 1 and not found:
        raise RuntimeError(f"cargo machete reported findings this could not parse:\n{r.stdout}")
    return sorted(found)


def cargo(*args: str, env: dict | None = None) -> str:
    """Run a measurement tool; failures must never look like zero warnings."""
    try:
        r = subprocess.run([CARGO, *args], cwd=ROOT, capture_output=True, text=True, env=env)
    except OSError as exc:
        raise RuntimeError(f"could not run cargo {' '.join(args)}: {exc}") from exc
    output = r.stdout + r.stderr
    # rustfmt uses status 1 for ordinary formatting differences. Other failures,
    # including status 1 without a diff, mean there is no valid measurement.
    formatting_diff = args == ("fmt", "--check") and r.returncode == 1 and "Diff in " in output
    if r.returncode != 0 and not formatting_diff:
        raise RuntimeError(f"cargo {' '.join(args)} failed ({r.returncode}):\n{output}")
    return output


# ---------------------------------------------------------------- metrics

def measure(fast: bool = False, coverage: bool = False, mutants: Path | None = None) -> dict:
    files = {
        p.relative_to(ROOT).as_posix(): len(p.read_text(encoding="utf-8", errors="replace").splitlines())
        for p in rust_files()
    }
    markers = tests = 0
    for p in rust_files(include_tests=True):
        body = p.read_text(encoding="utf-8", errors="replace")
        markers += len(re.findall(r"\b(TODO|FIXME|XXX|HACK)\b", body))
        tests += body.count("#[test]")

    m = {
        "max_file_lines": max(files.values()) if files else 0,
        "debt_markers": markers,
        "test_functions": tests,
    }

    if not fast:
        # One clippy run supplies every code-shape metric. Each lint gets its own budget
        # line, so a project that fixes its documentation cannot silently spend the credit
        # on longer functions.
        for lint, n in lint_counts().items():
            m[f"lint:{lint}"] = n
        m["rustfmt_hunks"] = cargo("fmt", "--check").count("Diff in ")
        m["unused_dependencies"] = unused_dependencies()
    if coverage:
        m.update(coverage_by_crate())
    if mutants is not None:
        m.update(mutation_scores(mutants))
    return m


# How each metric is allowed to move. `set` metrics are compared by membership: anything
# in the budget is grandfathered, anything new is a regression. Any key not named here is
# treated as a count that must not grow -- which is what every `lint:` entry wants.
DIRECTION = {
    "unused_dependencies": "set",
    "max_file_lines": "max",
    "debt_markers": "max",
    "rustfmt_hunks": "max",
    "test_functions": "min",
}


def direction_of(key: str) -> str:
    # Floors: a whole percentage that must not fall, raised as measurements clear it.
    if key.startswith(("coverage:", "mutation:")):
        return "floor"
    return DIRECTION.get(key, "max")


def budget_value(key: str, measured):
    """What `--update` records for a measurement: floors are rounded down."""
    return math.floor(measured) if direction_of(key) == "floor" else measured


EXPLAIN = {
    "unused_dependencies": "dependencies no source uses (cargo machete)",
    "max_file_lines": "longest single file",
    "debt_markers": "TODO / FIXME / XXX / HACK markers",
    "rustfmt_hunks": "hunks cargo fmt would rewrite",
    "test_functions": "#[test] functions",
}


def explain(key: str) -> str:
    if key.startswith("lint:"):
        return f"{key[5:]} warnings"
    if key.startswith("coverage:"):
        return f"% of {key[9:]}'s lines the tests run"
    if key.startswith("mutation:"):
        return f"% of sampled mutants of {key[9:]} the tests kill"
    return EXPLAIN.get(key, key)


def load_budget() -> dict:
    if not BUDGET.exists():
        return {}
    return json.loads(BUDGET.read_text(encoding="utf-8"))


def check(now: dict, budget: dict) -> tuple[list[str], list[str], dict]:
    """Returns (failures, gains, tightened budget)."""
    failures, gains = [], []
    tightened = dict(budget)

    measured = dict(now)
    # A completed lint run with no warnings omits its lint keys. Retire those
    # allowances, but leave them alone during --fast (no compiler measurement).
    if "rustfmt_hunks" in now:
        for key in budget:
            if key.startswith("lint:"):
                measured.setdefault(key, 0)
    for key in sorted(measured):
        if key.startswith("_"):
            continue
        how = direction_of(key)
        cur, old = measured[key], budget.get(key)
        if old is None and key.startswith("lint:"):
            old = 0
        if old is None:
            tightened[key] = cur
            continue

        if how == "set":
            new_items = sorted(set(cur) - set(old))
            gone = sorted(set(old) - set(cur))
            if new_items:
                failures.append(
                    f"{key}: {len(new_items)} new ({explain(key)})\n"
                    + "".join(f"      + {x}\n" for x in new_items[:12])
                    + (f"      ... and {len(new_items) - 12} more\n" if len(new_items) > 12 else "")
                )
            if gone:
                gains.append(f"{key}: {len(gone)} fewer")
            # Absorb the improvement, keep the grandfathered rest.
            tightened[key] = sorted(set(old) - set(gone))
        elif how == "max":
            if cur > old:
                failures.append(f"{key}: {cur} > {old} allowed ({explain(key)})")
            elif cur < old:
                gains.append(f"{key}: {old} -> {cur}")
                tightened[key] = cur
        elif how == "min":
            if cur < old:
                failures.append(f"{key}: {cur} < {old} required ({explain(key)})")
            elif cur > old:
                gains.append(f"{key}: {old} -> {cur}")
                tightened[key] = cur
        elif how == "floor":
            if cur < old:
                failures.append(f"{key}: {cur} below the floor of {old} ({explain(key)})")
            elif math.floor(cur) > old:
                gains.append(f"{key}: floor {old} -> {math.floor(cur)}")
                tightened[key] = math.floor(cur)
    return failures, gains, tightened


def report(now: dict) -> None:
    lints = {k: v for k, v in now.items() if k.startswith("lint:")}
    if lints:
        print(f"{sum(lints.values())} lint warnings across {len(lints)} lints")
        for k, v in sorted(lints.items(), key=lambda kv: -kv[1]):
            print(f"  {v:5d}  {k[5:]}")
        print()
    for k in ("rustfmt_hunks", "debt_markers", "test_functions", "max_file_lines"):
        if k in now:
            print(f"{k:20s} {now[k]:>6}   {explain(k)}")
    for prefix in ("coverage:", "mutation:"):
        rows = {k: v for k, v in now.items() if k.startswith(prefix)}
        if rows:
            print()
            for k, v in sorted(rows.items()):
                print(f"{v:6.2f}  {explain(k)}")
    if now.get("unused_dependencies"):
        print(f"\n{len(now['unused_dependencies'])} unused dependencies (cargo machete):")
        for dep in now["unused_dependencies"]:
            print(f"        {dep}")
    print("\nWhat analyses this: `cargo clippy` with the lints configured in the workspace\n"
          "Cargo.toml. To see the individual sites, run clippy directly -- this only "
          "counts them and holds the line.")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--update", action="store_true",
                    help="re-baseline the budget, loosening it where needed")
    ap.add_argument("--report", action="store_true", help="print every metric, gate nothing")
    ap.add_argument("--fast", action="store_true",
                    help="skip clippy, rustfmt and machete (source metrics only)")
    ap.add_argument("--coverage", action="store_true",
                    help="also measure per-crate line coverage with cargo llvm-cov")
    ap.add_argument("--mutants", action="store_true",
                    help=f"also run cargo mutants on the core files (shard {MUTANT_SHARD}, ~2 h)")
    ap.add_argument("--mutants-from", type=Path, metavar="DIR",
                    help="score an existing cargo-mutants output dir instead of running one")
    ap.add_argument("--mutants-out", type=Path, default=ROOT / "target" / "mutants",
                    help="where --mutants writes (default target/mutants)")
    a = ap.parse_args()

    try:
        mdir = None
        if a.mutants_from:
            mdir = a.mutants_from / "mutants.out" if (a.mutants_from / "mutants.out").is_dir() else a.mutants_from
        elif a.mutants:
            mdir = run_mutants(a.mutants_out)
        now = measure(fast=a.fast, coverage=a.coverage, mutants=mdir)
    except RuntimeError as exc:
        print(f"quality measurement FAILED: {exc}", file=sys.stderr)
        return 1

    if a.report:
        report(now)
        return 0

    budget = load_budget()
    failures, gains, tightened = check(now, budget)

    if a.update:
        for key in now:
            if not key.startswith("_"):
                tightened[key] = budget_value(key, now[key])
        tightened["_note"] = ("Written by bench/quality.py --update. Every entry is a "
                              "ratchet: the check fails on a move away from these values. "
                              "Loosening one is a decision, and belongs in the commit "
                              "message that does it.")
        BUDGET.write_text(json.dumps(tightened, indent=2, sort_keys=True) + "\n",
                          encoding="utf-8")
        if failures:
            print("budget loosened:")
            for f in failures:
                print("  " + f.rstrip())
        if gains:
            print("budget tightened:")
            for g in gains:
                print("  " + g)
        if not failures and not gains:
            print("budget unchanged")
        print(f"\nwrote {BUDGET.relative_to(ROOT).as_posix()}")
        return 0

    if not budget:
        print(f"no budget at {BUDGET.relative_to(ROOT).as_posix()}; run --update to create it")
        return 1

    for g in gains:
        print(f"  better  {g}")
    if failures:
        print("\nquality ratchet FAILED:\n")
        for f in failures:
            print("  " + f.rstrip() + "\n")
        print("Fix it, or run --update and say why in the commit message.")
        return 1

    if gains:
        # Gains are absorbed on the spot so ground taken is not quietly given back later.
        tightened["_note"] = budget.get("_note", "")
        BUDGET.write_text(json.dumps(tightened, indent=2, sort_keys=True) + "\n",
                          encoding="utf-8")
        print("\nquality ratchet passed, budget tightened to match")
    else:
        print("quality ratchet passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
