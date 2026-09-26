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
    python bench/quality.py --report    # every metric, no gate
    python bench/quality.py --update    # re-baseline, after a deliberate decision

`--update` is the only way a budget loosens, and it prints what it loosened so the change
is visible in review. Improvements are absorbed automatically: a metric that got better
tightens the budget without being asked, so ground gained is never given back.

To see *where* the warnings are, run clippy directly. This only counts them.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
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


# A module smaller than this is usually a handful of helpers covered wherever they are
# called; above it, "no test at all" is a gap worth naming.
#
# This is a stand-in for line coverage, not a substitute for it: one trivial test clears a
# 2000-line module. `cargo llvm-cov` is the real answer and is not installed here; when it
# is, this should be replaced by a coverage floor rather than extended.
TESTABLE_MODULE_LINES = 200


def untested_modules() -> list[str]:
    """Source modules over `TESTABLE_MODULE_LINES` lines with no test anywhere.

    Counts both the inline `#[cfg(test)]` block and a same-named integration test file,
    because this repo uses both and either is fine.
    """
    out = []
    for p in rust_files():
        # `main.rs` is an entry point and `examples/` are demonstrations; neither is a unit
        # of library behaviour a test would pin down.
        if p.name == "main.rs" or "/examples/" in p.as_posix():
            continue
        body = p.read_text(encoding="utf-8", errors="replace")
        if len(body.splitlines()) < TESTABLE_MODULE_LINES:
            continue
        n = body.count("#[test]")
        sibling = p.parent.parent / "tests" / p.name
        if sibling.exists():
            n += sibling.read_text(encoding="utf-8", errors="replace").count("#[test]")
        if n == 0:
            out.append(p.relative_to(ROOT).as_posix())
    return sorted(out)


# The one place the engine may read its environment. Everything else goes through it, so a
# variable means the same thing everywhere and is read once per process; see
# docs/internal/env-vars.md for the variables that are left and why.
ENV_HELPER = ROOT / "crates" / "inkvec-core" / "src" / "env.rs"


def env_reads() -> int:
    """`std::env::var` / `var_os` calls in crate sources outside `inkvec_core::env`.

    Deployment settings that are genuinely environmental (the server's port, a model path)
    are counted too: the number is a ratchet, not a ban. What it stops is a new engine knob
    appearing behind the Options schema's back.
    """
    n = 0
    for p in rust_files():
        if p == ENV_HELPER or "/examples/" in p.as_posix():
            continue
        body = p.read_text(encoding="utf-8", errors="replace")
        n += len(re.findall(r"\benv::var(?:_os)?\s*\(", body))
    return n


def cargo(*args: str) -> str:
    """Run a measurement tool; failures must never look like zero warnings."""
    try:
        r = subprocess.run([CARGO, *args], cwd=ROOT, capture_output=True, text=True)
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

def measure(fast: bool = False) -> dict:
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
        "untested_modules": untested_modules(),
        "max_file_lines": max(files.values()) if files else 0,
        "debt_markers": markers,
        "test_functions": tests,
        "env_reads": env_reads(),
    }

    if not fast:
        # One clippy run supplies every code-shape metric. Each lint gets its own budget
        # line, so a project that fixes its documentation cannot silently spend the credit
        # on longer functions.
        for lint, n in lint_counts().items():
            m[f"lint:{lint}"] = n
        m["rustfmt_hunks"] = cargo("fmt", "--check").count("Diff in ")
    return m


# How each metric is allowed to move. `set` metrics are compared by membership: anything
# in the budget is grandfathered, anything new is a regression. Any key not named here is
# treated as a count that must not grow -- which is what every `lint:` entry wants.
DIRECTION = {
    "untested_modules": "set",
    "max_file_lines": "max",
    "debt_markers": "max",
    "rustfmt_hunks": "max",
    "test_functions": "min",
    "env_reads": "max",
}


def direction_of(key: str) -> str:
    return DIRECTION.get(key, "max")


EXPLAIN = {
    "untested_modules": f"modules over {TESTABLE_MODULE_LINES} lines with no test",
    "max_file_lines": "longest single file",
    "debt_markers": "TODO / FIXME / XXX / HACK markers",
    "rustfmt_hunks": "hunks cargo fmt would rewrite",
    "test_functions": "#[test] functions",
    "env_reads": "std::env::var reads outside inkvec_core::env",
}


def explain(key: str) -> str:
    if key.startswith("lint:"):
        return f"{key[5:]} warnings"
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
    return failures, gains, tightened


def report(now: dict) -> None:
    lints = {k: v for k, v in now.items() if k.startswith("lint:")}
    if lints:
        print(f"{sum(lints.values())} lint warnings across {len(lints)} lints")
        for k, v in sorted(lints.items(), key=lambda kv: -kv[1]):
            print(f"  {v:5d}  {k[5:]}")
        print()
    for k in ("rustfmt_hunks", "debt_markers", "test_functions", "max_file_lines", "env_reads"):
        if k in now:
            print(f"{k:20s} {now[k]:>6}   {explain(k)}")
    if now.get("untested_modules"):
        print(f"\n{len(now['untested_modules'])} modules over {TESTABLE_MODULE_LINES} "
              f"lines with no test:")
        for mod in now["untested_modules"]:
            print(f"        {mod}")
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
                    help="skip clippy and rustfmt (source metrics only)")
    a = ap.parse_args()

    try:
        now = measure(fast=a.fast)
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
                tightened[key] = now[key]
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
