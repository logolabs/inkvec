#!/usr/bin/env python3
"""The CI checks a push most often fails on, run locally in about a minute.

    python tools/prepush.py            # the quick checks, in CI's order
    python tools/prepush.py --full     # ... and the quality ratchet (bench/quality.py), slower
    python tools/prepush.py --skip tsc --skip versions
    python tools/prepush.py --list     # name every check and stop

While 0.2.3 was cut, CI broke three times on things this catches: unformatted code in one
of the four Cargo workspaces, a version copy that was not bumped, and generated files
(bindings, the Studio guide, the Studio licence notices) left stale. None of them needs a
build, so together they take seconds, where the CI round trip takes half an hour.

The checks run in order and stop at the first failure, which prints its output and a
one-line hint saying how to fix it. Every command runs at below-normal priority, so a check
started while something else is building does not take the machine from it.

`--full` adds the quality ratchet, which builds and lints the workspace and so takes
minutes. When a count has fallen, the ratchet tightens `bench/quality_budget.json` by
itself; commit that change with the work that earned it.

`tools/install_hooks.py` installs this as a git pre-push hook; `git push --no-verify` skips
it for one push.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: Windows' BELOW_NORMAL_PRIORITY_CLASS, for `subprocess.Popen(creationflags=...)`.
BELOW_NORMAL = 0x00004000


@dataclass(frozen=True)
class Check:
    """One check: a short name for `--skip`, the command, and what to do when it fails."""

    name: str
    command: list[str]
    hint: str
    cwd: Path = ROOT
    full_only: bool = False
    #: A file the command needs that a fresh checkout lacks, and how to get it.
    needs: tuple[Path, str] | None = None


def checks() -> list[Check]:
    """Every check, in the order they run: cheapest and most often broken first."""
    py = sys.executable
    fmt = []
    for manifest in [None, "studio/core", "studio/src-tauri", "studio/wasm"]:
        where = [] if manifest is None else ["--manifest-path", f"{manifest}/Cargo.toml"]
        fmt.append(
            Check(
                name="fmt" if manifest is None else f"fmt:{manifest}",
                command=["cargo", "fmt", "--all", *where, "--", "--check"],
                hint=f"run `cargo fmt --all{' ' + ' '.join(where) if where else ''}` and commit the result",
            )
        )
    return [
        *fmt,
        Check(
            name="versions",
            command=[py, "tools/check_versions.py"],
            hint="bring every copy to Cargo.toml's version: `python tools/check_versions.py --write`, "
            "then the generators its docstring lists",
        ),
        Check(
            name="bindings",
            command=[py, "bindings/codegen/generate.py", "--check"],
            hint="regenerate the bindings: `python bindings/codegen/generate.py`, and commit them",
        ),
        Check(
            name="guide",
            command=[py, "tools/build_site.py", "--studio-guide", "--check"],
            hint="regenerate the Studio's copy of the guide: `python tools/build_site.py --studio-guide`",
        ),
        Check(
            name="notices",
            command=[py, "studio/tools/third_party.py", "--check"],
            hint="regenerate the Studio's licence notices: `python studio/tools/third_party.py`",
        ),
        Check(
            name="tsc",
            # What `npx tsc --noEmit -p studio` means, spelled so that it can only ever run
            # the Studio's own pinned TypeScript: `npx tsc` from the repository root would
            # fetch an unrelated package named `tsc` from the registry instead.
            command=["node", "node_modules/typescript/bin/tsc", "--noEmit", "-p", "."],
            cwd=ROOT / "studio",
            hint="fix the type errors above",
            needs=(
                ROOT / "studio/node_modules/typescript/bin/tsc",
                "run `npm ci` in studio/ once to install TypeScript (or pass --skip tsc)",
            ),
        ),
        Check(
            name="quality",
            command=[py, "bench/quality.py"],
            full_only=True,
            hint="see CONTRIBUTING.md, 'The quality ratchet'; never loosen a budget to pass",
        ),
    ]


def low_priority() -> dict:
    """`subprocess` keyword arguments that start a child at below-normal priority."""
    if os.name == "nt":
        return {"creationflags": BELOW_NORMAL}
    return {"preexec_fn": lambda: os.nice(10)}


def resolve(command: list[str]) -> list[str] | None:
    """The command with its program found on PATH (`npx` is `npx.cmd` on Windows), or `None`
    when the program is not installed."""
    program = shutil.which(command[0])
    return None if program is None else [program, *command[1:]]


def run(check: Check) -> tuple[bool, str, float]:
    """Run one check: whether it passed, its combined output, and how long it took. A
    program that is not installed fails the check with that as the output."""
    command = resolve(check.command)
    if command is None:
        return False, f"`{check.command[0]}` is not on PATH", 0.0
    start = time.perf_counter()
    done = subprocess.run(
        command,
        cwd=check.cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        **low_priority(),
    )
    return done.returncode == 0, done.stdout, time.perf_counter() - start


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--full", action="store_true", help="also run bench/quality.py (slower)")
    ap.add_argument(
        "--skip", action="append", default=[], metavar="NAME", help="skip a check by name (repeatable)"
    )
    ap.add_argument("--list", action="store_true", help="list the checks and exit")
    args = ap.parse_args()

    selected = [c for c in checks() if args.full or not c.full_only]
    known = {c.name for c in checks()}
    unknown = [s for s in args.skip if s not in known]
    if unknown:
        ap.error(f"no check named {', '.join(unknown)}; --list shows them")
    if args.list:
        for c in checks():
            print(f"{c.name:<22} {' '.join(c.command)}{'   (--full)' if c.full_only else ''}")
        return 0

    total = time.perf_counter()
    for check in selected:
        if check.name in args.skip:
            print(f"skip  {check.name}")
            continue
        if check.needs is not None and not check.needs[0].exists():
            print(f"FAIL  {check.name}: {check.needs[0].relative_to(ROOT)} is missing")
            print(f"hint: {check.needs[1]}")
            return 1
        ok, output, seconds = run(check)
        if not ok:
            print(output.rstrip())
            print(f"FAIL  {check.name} ({seconds:.1f} s)")
            if resolve(check.command) is None:
                print(f"hint: install `{check.command[0]}` or put it on PATH, or pass --skip {check.name}")
            else:
                print(f"hint: {check.hint}")
            return 1
        print(f"ok    {check.name} ({seconds:.1f} s)")
    print(f"all checks passed in {time.perf_counter() - total:.1f} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
