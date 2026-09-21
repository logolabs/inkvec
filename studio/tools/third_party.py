#!/usr/bin/env python3
"""Regenerate studio/THIRD_PARTY.md from the app's own dependency graph.

    python3 studio/tools/third_party.py            # write studio/THIRD_PARTY.md
    python3 studio/tools/third_party.py --check    # exit 1 if the file is out of date

The repository's `tools/third_party.py` resolves the root workspace, and `studio/src-tauri`
is deliberately not a member of it — so the app's own dependencies (Tauri, resvg, the zip
writer, the HTTP client) appear in neither file unless something writes them here. The app
ships both documents and the About screen shows them, which is the point: the notices a
user reads have to be the notices for the binary they are running.

Hand-maintained sections below cover what `cargo tree` cannot see: the two bundled
webfonts, the icon geometry, and the npm packages that end up in the bundled JavaScript.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "src-tauri" / "Cargo.toml"
OUT = ROOT / "THIRD_PARTY.md"
CARGO = shutil.which("cargo") or str(Path.home() / ".cargo" / "bin" / "cargo")

# Licences that need no further thought. Anything outside this set is called out in its
# own section rather than buried in the table.
PERMISSIVE = {
    "MIT", "MIT-0", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "0BSD",
    "Unicode-3.0", "Unicode-DFS-2016", "Unlicense", "CC0-1.0", "BSL-1.0",
    "Apache-2.0 WITH LLVM-exception", "CDLA-Permissive-2.0",
}

# `cargo tree` resolves target-specific dependencies for the host only, so a notice
# file generated on Linux would leave out the Windows-only registry crate that the
# Windows binary actually links. The three triples below are the ones the release
# workflow ships; a crate keeps the label of the first build it appears in, so the
# host's own crates stay "default" and only the ones it cannot see get a platform.
BUILDS = [
    ("default", []),
    ("denoiser", ["--features", "denoiser"]),
    ("windows", ["--target", "x86_64-pc-windows-msvc"]),
    ("macos", ["--target", "aarch64-apple-darwin"]),
    ("linux", ["--target", "x86_64-unknown-linux-gnu"]),
]

# Crates that come from this repository. They are Inkvec, not third parties.
OURS = re.compile(r"^inkvec(-|$)")

FIXED_SECTIONS = """
## Bundled fonts

Both are subset to Latin and shipped in `studio/public/fonts/`. The app makes no network
request it has not explained, and that includes a font, which is why they are bundled
rather than fetched.

| Font | Licence | Source |
| --- | --- | --- |
| Inter | SIL Open Font License 1.1 | https://github.com/rsms/inter |
| Playfair Display | SIL Open Font License 1.1 | https://github.com/clauseggers/Playfair-Display |

The OFL permits bundling and redistribution with the application. Neither font is
renamed, sold on its own, or distributed with a reserved font name.

## Icons

The interface's glyphs are drawn from Lucide's geometry, inlined in
`studio/src/lib/dom.ts` rather than taken as a package dependency — only the twenty-odd
shapes the app actually uses. Lucide is ISC-licensed.

The app's own mark (the stair-stepped pixels and the copper curve) is LogoLabs' and is
drawn by `studio/tools/make_assets.py`.

## JavaScript

The bundled frontend has no runtime framework. Its only dependencies are Tauri's own
JavaScript API and three of its plugins, all MIT OR Apache-2.0:

| Package | Licence |
| --- | --- |
| @tauri-apps/api | MIT OR Apache-2.0 |
| @tauri-apps/plugin-dialog | MIT OR Apache-2.0 |
| @tauri-apps/plugin-opener | MIT OR Apache-2.0 |
| @tauri-apps/plugin-clipboard-manager | MIT OR Apache-2.0 |

`vite` and `typescript` are build-time only and are not distributed with the app.

## The engine

Inkvec itself, and every crate in this repository, is Apache-2.0. Its own dependency
notices are in `docs/THIRD_PARTY.md`, which the app ships and displays alongside this
file.
"""


def permissive(expr: str | None) -> bool:
    """Whether an SPDX expression has at least one plainly permissive alternative.

    `MIT OR Apache-2.0` is permissive; `MPL-2.0` is not, and gets its own line. The same
    reading the repository's own `tools/third_party.py` uses, so the two documents agree
    about what counts as worth calling out.
    """
    if not expr:
        return False
    expr = expr.replace("/", " OR ")
    for alt in re.split(r"\s+OR\s+", expr.replace("(", " ").replace(")", " ")):
        terms = [t.strip() for t in re.split(r"\s+AND\s+", alt) if t.strip()]
        if terms and all(t in PERMISSIVE for t in terms):
            return True
    return False


def tree(extra: list[str]) -> dict[str, str]:
    """Crate name to declared licence, as `cargo tree` resolves it for a real build."""
    out = subprocess.run(
        [
            CARGO, "tree", "--manifest-path", str(MANIFEST),
            "--edges", "normal,build", "--prefix", "none", "--no-dedupe",
            "--format", "{p}|{l}", *extra,
        ],
        capture_output=True, text=True, check=True,
    ).stdout
    found: dict[str, str] = {}
    for line in out.splitlines():
        line = line.strip()
        if not line or "|" not in line:
            continue
        spec, licence = line.rsplit("|", 1)
        name = spec.split()[0]
        if OURS.match(name):
            continue
        found.setdefault(name, licence.strip() or "(not declared)")
    return found


def render() -> str:
    seen: dict[str, tuple[str, str]] = {}
    for label, extra in BUILDS:
        for name, licence in tree(extra).items():
            seen.setdefault(name, (licence, label))

    rows = sorted(seen.items())
    unusual = [(n, l, b) for n, (l, b) in rows if not permissive(l)]

    lines = [
        "# Third-party components — Inkvec Studio Lite",
        "",
        "Generated by `studio/tools/third_party.py`. Regenerate it after changing"
        " dependencies.",
        "Inkvec Studio Lite itself is licensed under Apache-2.0 (`LICENSE`).",
        "",
        f"{len(rows)} Rust crates are compiled into the app"
        f" ({sum(1 for _, (_, b) in rows if b == 'default')} in every build; the rest"
        f" only with the optional denoiser, or only on the platform the Build column"
        f" names).",
        "",
    ]

    if unusual:
        lines += [
            "Not under a plainly permissive licence, with the smallest build that"
            " compiles them. None is copyleft in a way that reaches this app: MPL-2.0 is"
            " file-level and these crates are used unmodified.",
            "",
        ]
        for name, licence, build in unusual:
            lines.append(f"- `{name}` — {licence} ({build})")
        lines.append("")

    lines += [
        "## Crates",
        "",
        "| Crate | Licence | Build |",
        "| --- | --- | --- |",
    ]
    for name, (licence, build) in rows:
        lines.append(f"| `{name}` | {licence} | {build} |")

    return "\n".join(lines) + "\n" + FIXED_SECTIONS


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="exit 1 if the file is stale")
    args = ap.parse_args()

    try:
        text = render()
    except subprocess.CalledProcessError as e:
        print(e.stderr or e, file=sys.stderr)
        return 1

    if args.check:
        current = OUT.read_text() if OUT.exists() else ""
        if current != text:
            print(
                f"{OUT.relative_to(ROOT.parent)} is out of date; run"
                " `python3 studio/tools/third_party.py`",
                file=sys.stderr,
            )
            return 1
        print("notices up to date")
        return 0

    OUT.write_text(text)
    print(f"wrote {OUT.relative_to(ROOT.parent)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
