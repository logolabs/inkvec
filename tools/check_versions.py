#!/usr/bin/env python3
"""Every file that carries its own copy of Inkvec's version agrees with Cargo.toml.

    python tools/check_versions.py              # exit 1 and list every file that disagrees
    python tools/check_versions.py --tag v0.1.7 # ... and the tag must be that version too
    python tools/check_versions.py --write      # sync the files no other generator owns

There is one version: `[workspace.package] version` in the root Cargo.toml (docs/BINDINGS.md,
"Versions"). A handful of files cannot inherit it and hold a copy; a release that bumps
Cargo.toml and forgets one of them publishes an npm package, a POM or an installer under the
previous number, or fails at the tag (npm.yml refuses a tag that disagrees with package.json).
CI runs this, so the drift is caught on the pull request instead.

Who writes what, after bumping Cargo.toml:

  - `node packages/npm/build.mjs --skip-wasm`: packages/npm/package.json, its lock and
    src/version.generated.ts (written first, so even a checkout without packages/npm/wasm/,
    where the rest of that build stops, gets them);
  - `python packages/java/build.py`: packages/java/pom.xml;
  - `python bindings/codegen/generate.py`: bindings/openapi.json;
  - `cargo update --workspace --offline` (root) and `cargo update -p inkvec-studio --offline
    --manifest-path studio/src-tauri/Cargo.toml`: the two Cargo.lock files;
  - this script with --write: the sibling requirements under [workspace.dependencies], both
    wasm-pack packages under web/ (rebuilt only when the Space is redeployed, so the number
    is synced by hand in between), and Inkvec Studio's four (its src-tauri is a workspace of
    its own and inherits nothing).
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# The crates whose version the root workspace publishes; the Cargo.lock entries of every
# `inkvec*` package follow the workspace version.
LOCK_ENTRY_RE = re.compile(r'^name = "(inkvec[\w-]*)"\nversion = "([^"]+)"', re.M)


def read(rel: str) -> str:
    return (ROOT / rel).read_text(encoding="utf-8").replace("\r\n", "\n")


def write(rel: str, text: str) -> None:
    with open(ROOT / rel, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def workspace_version() -> str:
    toml = read("Cargo.toml")
    m = re.search(r"^\[workspace\.package\]\s*\n(.*?)(?=^\[)", toml, re.M | re.S)
    v = m and re.search(r'^version\s*=\s*"([^"]+)"', m.group(1), re.M)
    if not v:
        raise SystemExit("Cargo.toml: no version under [workspace.package]")
    return v.group(1)


# ---- the files, each as (path, found versions, rewrite or None) --------------------------


def workspace_deps(version: str):
    """[workspace.dependencies]: `inkvec-x = { path = ..., version = "..." }`."""
    rel = "Cargo.toml"
    text = read(rel)
    m = re.search(r"^\[workspace\.dependencies\]\s*\n(.*?)(?=^\[|\Z)", text, re.M | re.S)
    if not m:
        return rel, [], None
    dep_re = re.compile(r'^(inkvec[\w-]*\s*=\s*\{[^}\n]*\bversion\s*=\s*")([^"]+)(")', re.M)
    found = [d.group(2) for d in dep_re.finditer(m.group(1))]
    body = dep_re.sub(lambda d: f"{d.group(1)}{version}{d.group(3)}", m.group(1))
    return rel, found, text[: m.start(1)] + body + text[m.end(1) :]


def json_file(rel: str, version: str, lock: bool = False):
    text = read(rel)
    data = json.loads(text)
    found = [data.get("version")]
    data["version"] = version
    if lock:
        found.append(data["packages"][""].get("version"))
        data["packages"][""]["version"] = version
    return rel, found, json.dumps(data, indent=2, ensure_ascii=False) + "\n"


def tauri_conf(version: str):
    # Rewritten in place rather than re-serialised: tauri.conf.json is hand-formatted.
    rel = "studio/src-tauri/tauri.conf.json"
    text = read(rel)
    found = [json.loads(text).get("version")]
    new = re.sub(r'^(  "version":\s*")[^"]+(")', rf"\g<1>{version}\g<2>", text, count=1, flags=re.M)
    return rel, found, new


def studio_cargo(version: str, rel: str = "studio/src-tauri/Cargo.toml"):
    """A Studio crate's own [package] version: the desktop shell, the shared core, the
    browser shell. They sit outside the root workspace, so they cannot inherit it."""
    text = read(rel)
    m = re.search(r'(^\[package\]\s*\n(?:(?!^\[).*\n)*?version\s*=\s*")([^"]+)(")', text, re.M)
    if not m:
        return rel, [None], None
    return rel, [m.group(2)], text[: m.start(2)] + version + text[m.end(2) :]


def generated(rel: str, pattern: str):
    """A file another generator owns: checked here, written there."""
    m = re.search(pattern, read(rel), re.M)
    return rel, [m.group(1) if m else None], None


def lockfile(rel: str):
    return rel, [v for _, v in LOCK_ENTRY_RE.findall(read(rel))], None


def all_files(version: str):
    return [
        workspace_deps(version),
        json_file("web/pkg/package.json", version),
        json_file("web/pkg-threads/package.json", version),
        json_file("studio/package.json", version),
        json_file("studio/package-lock.json", version, lock=True),
        studio_cargo(version),
        studio_cargo(version, "studio/core/Cargo.toml"),
        studio_cargo(version, "studio/wasm/Cargo.toml"),
        tauri_conf(version),
        generated("packages/npm/package.json", r'^  "version":\s*"([^"]+)"'),
        generated("packages/npm/package-lock.json", r'^  "version":\s*"([^"]+)"'),
        generated("packages/npm/src/version.generated.ts", r'VERSION = "([^"]+)"'),
        generated("packages/java/pom.xml", r"<artifactId>inkvec</artifactId>\s*\n\s*<version>([^<]+)<"),
        generated("bindings/openapi.json", r'^    "version":\s*"([^"]+)"'),
        lockfile("Cargo.lock"),
        lockfile("studio/src-tauri/Cargo.lock"),
    ]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--write", action="store_true", help="sync the files this script owns")
    ap.add_argument("--tag", help="a release tag (vX.Y.Z) that must name the workspace version")
    a = ap.parse_args()

    version = workspace_version()
    bad = 0
    if a.tag is not None and a.tag != f"v{version}":
        print(f"tag {a.tag}, but Cargo.toml's workspace version is {version}")
        bad += 1
    for rel, found, rewritten in all_files(version):
        if found and all(v == version for v in found):
            continue
        if a.write and rewritten is not None:
            write(rel, rewritten)
            print(f"wrote {rel} at {version}")
            continue
        seen = ", ".join(sorted({str(v) for v in found})) or "no version found"
        print(f"stale: {rel} ({seen}; workspace is {version})")
        bad += 1
    if bad:
        print("\nSee the docstring of tools/check_versions.py for which tool writes each file.")
        return 1
    print(f"every version file is at {version}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
