#!/usr/bin/env python3
"""Install a git pre-push hook that runs `tools/prepush.py` before every push.

    python tools/install_hooks.py              # install (refuses to replace a hook it did not write)
    python tools/install_hooks.py --full       # the hook runs `prepush.py --full`
    python tools/install_hooks.py --force      # replace whatever pre-push hook is there
    python tools/install_hooks.py --uninstall  # remove the hook this script installed

The hook goes where git looks for hooks (`git rev-parse --git-path hooks`), so it respects
`core.hooksPath`, and every worktree of the repository shares it. A failing check stops the
push with the check's own hint; `git push --no-verify` skips the hook for one push.
"""

from __future__ import annotations

import argparse
import os
import stat
import subprocess
import sys
from pathlib import Path

#: The line that marks a hook as this script's, so a reinstall may replace it and an
#: uninstall may remove it, and nobody else's hook is touched.
MARKER = "# Installed by tools/install_hooks.py"


def hook_text(full: bool) -> str:
    """The hook: a POSIX shell script (git runs hooks through its own `sh`, on Windows too)
    that finds a Python and hands over to `tools/prepush.py` at the top of the work tree."""
    flags = " --full" if full else ""
    return (
        "#!/bin/sh\n"
        f"{MARKER}: run the local CI checks before every push.\n"
        "# Skip it once with `git push --no-verify`.\n"
        'cd "$(git rev-parse --show-toplevel)" || exit 1\n'
        "if command -v python3 >/dev/null 2>&1; then PY=python3; else PY=python; fi\n"
        f'exec "$PY" tools/prepush.py{flags}\n'
    )


def git(*args: str) -> str:
    """Run git in the current directory and return its output, stripped."""
    return subprocess.run(
        ["git", *args], check=True, stdout=subprocess.PIPE, text=True
    ).stdout.strip()


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--full", action="store_true", help="the hook also runs the quality ratchet")
    ap.add_argument("--force", action="store_true", help="replace a pre-push hook this script did not write")
    ap.add_argument("--uninstall", action="store_true", help="remove the hook this script installed")
    args = ap.parse_args()

    top = Path(git("rev-parse", "--show-toplevel"))
    if not (top / "tools" / "prepush.py").is_file():
        print(f"no tools/prepush.py under {top}; run this from an Inkvec checkout", file=sys.stderr)
        return 1
    hooks = Path(git("rev-parse", "--git-path", "hooks"))
    if not hooks.is_absolute():
        hooks = (Path.cwd() / hooks).resolve()
    hook = hooks / "pre-push"
    ours = hook.is_file() and MARKER in hook.read_text(encoding="utf-8", errors="replace")

    if args.uninstall:
        if not hook.exists():
            print("no pre-push hook installed")
        elif ours:
            hook.unlink()
            print(f"removed {hook}")
        else:
            print(f"{hook} was not installed by this script; left alone", file=sys.stderr)
            return 1
        return 0

    if hook.exists() and not ours and not args.force:
        print(
            f"{hook} already exists and was not written by this script; "
            "pass --force to replace it",
            file=sys.stderr,
        )
        return 1
    hooks.mkdir(parents=True, exist_ok=True)
    # LF endings whatever the platform: the hook is read by `sh`.
    hook.write_bytes(hook_text(args.full).encode("utf-8"))
    if os.name != "nt":
        hook.chmod(hook.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    print(f"installed {hook}: `git push` now runs tools/prepush.py{' --full' if args.full else ''}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
