#!/usr/bin/env python3
"""Sync pom.xml's <version> to the workspace's, the way packages/npm/build.mjs does for
package.json.

    python packages/java/build.py           # write pom.xml's <version> if it drifted
    python packages/java/build.py --check    # exit 1 instead, printing what is stale

There is one version for Inkvec: `workspace.package.version` in the root Cargo.toml (see
docs/BINDINGS.md, "Versions"). Nothing about it is hand-maintained here; run this after a
version bump in Cargo.toml, before `mvn package`. It touches nothing else -- the typed
options (`InkvecOptions.java`) and the README's options table come from
`bindings/codegen/generate.py`, not from this script.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

PKG = Path(__file__).resolve().parent
ROOT = PKG.parents[1]
CARGO_TOML = ROOT / "Cargo.toml"
POM = PKG / "pom.xml"

VERSION_RE = re.compile(r'^version = "([^"]+)"', re.M)
POM_VERSION_RE = re.compile(r"(<artifactId>inkvec</artifactId>\s*\n\s*<version>)([^<]+)(</version>)")


def workspace_version() -> str:
    text = CARGO_TOML.read_text(encoding="utf-8")
    m = VERSION_RE.search(text)
    if not m:
        raise SystemExit(f"{CARGO_TOML}: no `version = \"...\"` under [workspace.package]")
    return m.group(1)


def synced_pom(version: str) -> str:
    text = POM.read_text(encoding="utf-8").replace("\r\n", "\n")
    if not POM_VERSION_RE.search(text):
        raise SystemExit(f"{POM}: could not find the <artifactId>inkvec</artifactId> <version> to sync")
    return POM_VERSION_RE.sub(lambda m: f"{m.group(1)}{version}{m.group(3)}", text, count=1)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="fail instead of writing")
    a = ap.parse_args()

    version = workspace_version()
    text = synced_pom(version)
    current = POM.read_text(encoding="utf-8").replace("\r\n", "\n")
    if current == text:
        print(f"pom.xml is already at {version}")
        return 0
    if a.check:
        print(f"stale: pom.xml (workspace is at {version})")
        return 1
    with open(POM, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print(f"wrote pom.xml at {version}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
