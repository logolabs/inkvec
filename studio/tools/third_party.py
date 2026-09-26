#!/usr/bin/env python3
"""Regenerate studio/STUDIO_THIRD_PARTY.md from the app's own dependency graph.

    python3 studio/tools/third_party.py            # write studio/STUDIO_THIRD_PARTY.md
    python3 studio/tools/third_party.py --check    # exit 1 if the file is out of date

The repository's `tools/third_party.py` resolves the root workspace, and `studio/src-tauri`
is deliberately not a member of it — so the app's own dependencies (Tauri, resvg, the zip
writer, the HTTP client) appear in neither file unless something writes them here. The app
ships both documents and the About screen shows them, which is the point: the notices a
user reads have to be the notices for the binary they are running.

The same file ships with Inkvec Studio Lite, the browser build (`scripts/build-web.mjs`
copies it into the site), so the browser shell's own crates are resolved too, for the
WebAssembly target: wasm-bindgen, js-sys and the rest never appear in a desktop build.

Hand-maintained sections below cover what `cargo tree` cannot see: the two bundled
webfonts, the icon geometry, the npm packages that end up in the bundled JavaScript, and
ONNX Runtime Web, which the browser build's denoiser loads from jsDelivr at run time (its
version is read from `web/denoise.js`, so `--check` fails when the pin moves).
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
# The browser build's denoiser module, which pins the ONNX Runtime Web it fetches.
DENOISE_JS = ROOT.parent / "web" / "denoise.js"
# The trademark notice, shared with the engine's docs/THIRD_PARTY.md.
TRADEMARKS = ROOT.parent / "docs" / "TRADEMARKS.md"
# STUDIO_THIRD_PARTY.md, not THIRD_PARTY.md, and the awkward name is load-bearing: the
# app bundles this file next to the engine's own docs/THIRD_PARTY.md, and Tauri's WiX
# generator ignores the destination name a resource is mapped to and uses the source's
# file name. Two sources both called THIRD_PARTY.md therefore became one target file
# installed by two components, which WiX rejects (ICE30) -- the MSI could not be built at
# all. Distinct source names, no rename to ignore.
OUT = ROOT / "STUDIO_THIRD_PARTY.md"
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
    # Inkvec Studio Lite: the browser shell, for the target it is built for, in both of the
    # builds `scripts/build-web.mjs` makes (one core, and the rayon pool of Web Workers).
    ("browser", ["-p", "inkvec-studio-wasm", "--target", "wasm32-unknown-unknown"]),
    ("browser, threads", ["-p", "inkvec-studio-wasm", "--target", "wasm32-unknown-unknown",
                          "--features", "threads"]),
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

## The browser build

Inkvec Studio Lite is this app's interface and its shared core compiled to WebAssembly.
The crates only it compiles are marked `browser` above (`browser, threads` for the build
with a pool of Web Workers); the JavaScript glue `wasm-bindgen` generates for them, and the
worker helper of `wasm-bindgen-rayon`, ship in the site under the same licences.

Its denoiser runs in ONNX Runtime Web, which the page does not bundle: the first time the
denoiser runs, it loads the pinned release from jsDelivr, and the browser caches it.

| Component | Version | Licence | Source |
| --- | --- | --- | --- |
| ONNX Runtime Web (`onnxruntime-web`) | {ort_version} | MIT | https://cdn.jsdelivr.net/npm/onnxruntime-web@{ort_version}/ |

ONNX Runtime includes third-party code of its own; Microsoft lists it, with its licences,
in `ThirdPartyNotices.txt` at https://github.com/microsoft/onnxruntime.

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


def ort_web_version() -> str:
    """The ONNX Runtime Web release `web/denoise.js` pins."""
    text = DENOISE_JS.read_text(encoding="utf-8")
    found = re.search(r'const ORT_VERSION = "([^"]+)"', text)
    if not found:
        raise SystemExit(f"no ORT_VERSION in {DENOISE_JS}; the notices cannot name it")
    return found.group(1)


def render() -> str:
    seen: dict[str, tuple[str, str]] = {}
    for label, extra in BUILDS:
        for name, licence in tree(extra).items():
            seen.setdefault(name, (licence, label))

    rows = sorted(seen.items())
    unusual = [(n, l, b) for n, (l, b) in rows if not permissive(l)]

    lines = [
        "# Third-party components — Inkvec Studio",
        "",
        "Generated by `studio/tools/third_party.py`. Regenerate it after changing"
        " dependencies.",
        "Inkvec Studio itself is licensed under Apache-2.0 (`LICENSE`); Inkvec Studio Lite, its"
        " browser build, is the same code and the same licence.",
        "",
        f"{len(rows)} Rust crates are compiled into the app"
        f" ({sum(1 for _, (_, b) in rows if b == 'default')} in every build; the rest"
        f" only with the optional denoiser, only on the platform the Build column"
        f" names, or only in the browser build).",
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

    lines += ["## Trademarks", "", TRADEMARKS.read_text(encoding="utf-8").rstrip("\n"), ""]

    lines += [
        "## Crates",
        "",
        "| Crate | Licence | Build |",
        "| --- | --- | --- |",
    ]
    for name, (licence, build) in rows:
        lines.append(f"| `{name}` | {licence} | {build} |")

    return "\n".join(lines) + "\n" + FIXED_SECTIONS.replace("{ort_version}", ort_web_version())


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
        current = OUT.read_text(encoding="utf-8") if OUT.exists() else ""
        if current != text:
            print(
                f"{OUT.relative_to(ROOT.parent)} is out of date; run"
                " `python3 studio/tools/third_party.py`",
                file=sys.stderr,
            )
            return 1
        print("notices up to date")
        return 0

    # UTF-8 and LF on every platform, so a file written on Windows passes the check on Linux.
    OUT.write_text(text, encoding="utf-8", newline="\n")
    print(f"wrote {OUT.relative_to(ROOT.parent)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
