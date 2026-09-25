#!/usr/bin/env python3
"""Compile and dry-run the Windows installer's hooks (studio/src-tauri/installer/hooks.nsh).

    python studio/tools/check_installer_hooks.py                 # compile, run, report
    python studio/tools/check_installer_hooks.py --compile-only  # syntax only
    python studio/tools/check_installer_hooks.py --hooks old.nsh # another copy of the hooks

The hooks replace an old "Inkvec Studio Lite" install before installing "Inkvec Studio".
Building and running the real installer would do that to this machine, so instead the
hooks are inserted into a small harness (tools/installer_test/harness.nsi) the way Tauri's
template inserts them, around a staged old install whose uninstaller is a fake
(tools/installer_test/fake_uninstaller.nsi). The harness points the hooks at a private
registry key (HKCU\\Software\\InkvecHookDryTest) and a folder in %TEMP%, removes both when it
is done, and never looks at the real uninstall entries, context menu or PATH.

Needs Windows and NSIS 3 with Tauri's `nsis_tauri_utils` plugin, which is what
`tauri build` downloads to %LOCALAPPDATA%\\tauri\\NSIS; `--makensis` names another.
"""
from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

STUDIO = Path(__file__).resolve().parents[1]
HOOKS = STUDIO / "src-tauri" / "installer" / "hooks.nsh"
TESTS = STUDIO / "tools" / "installer_test"


def find_makensis(given: str | None) -> Path:
    candidates = [given] if given else []
    local = os.environ.get("LOCALAPPDATA")
    if local:
        candidates.append(str(Path(local) / "tauri" / "NSIS" / "makensis.exe"))
    candidates.append(shutil.which("makensis") or "")
    for c in candidates:
        if c and Path(c).is_file():
            return Path(c)
    sys.exit("makensis not found: run `tauri build` once, or pass --makensis")


def compile_nsi(makensis: Path, script: Path, defines: dict[str, str], strict: bool = True) -> None:
    """Compile `script`; `strict` treats warnings as errors, as the hooks deserve."""
    flags = ["/V2"] + (["/WX"] if strict else [])
    args = [str(makensis), *flags] + [f"/D{k}={v}" for k, v in defines.items()] + [str(script)]
    run = subprocess.run(args, capture_output=True, text=True)
    if run.returncode != 0:
        print(run.stdout[-4000:], run.stderr[-4000:], sep="\n", file=sys.stderr)
        sys.exit(f"{script.name} did not compile")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--makensis", help="the makensis to use")
    ap.add_argument("--hooks", type=Path, default=HOOKS, help="the hooks file to test")
    ap.add_argument("--compile-only", action="store_true", help="check that it compiles; run nothing")
    args = ap.parse_args()
    if os.name != "nt" and not args.compile_only:
        sys.exit("the dry run needs Windows; --compile-only works anywhere makensis does")

    makensis = find_makensis(args.makensis)
    plugins = makensis.parent / "Plugins" / "x86-unicode" / "additional"
    with tempfile.TemporaryDirectory(prefix="inkvec-hooks-") as tmp:
        tmp_path = Path(tmp)
        work = tmp_path / "work"
        common = {
            "INKVEC_SOFTWARE": r"Software\InkvecHookDryTest", "INKVEC_CLI_DIR": str(work / "WindowsApps"),
            "WORK": str(work), "DRY_GUID": "{0D1CE0DE-1111-4222-8333-444455556666}",
        }
        fake = tmp_path / "fake-uninstall.exe"
        harness = tmp_path / "harness.exe"
        result = tmp_path / "result.txt"
        compile_nsi(makensis, TESTS / "fake_uninstaller.nsi", {**common, "OUT": str(fake)})
        compile_nsi(makensis, TESTS / "harness.nsi", {
            **common, "OUT": str(harness), "HOOKS": str(args.hooks.resolve()), "PLUGINS": str(plugins),
            "FAKE": str(fake), "RESULT": str(result),
        }, strict=args.hooks.resolve() == HOOKS)
        if args.compile_only:
            print("hooks compile")
            return 0
        subprocess.run([str(harness), "/S"], check=True, timeout=120)
        if not result.exists():
            sys.exit("the harness wrote no result")
        lines = result.read_text(encoding="utf-8", errors="replace").splitlines()
    for line in lines:
        print(line)
    failed = sum(1 for line in lines if line.startswith("FAIL"))
    print(f"{len(lines)} checks, {failed} failed")
    return 1 if failed or not lines else 0


if __name__ == "__main__":
    sys.exit(main())
