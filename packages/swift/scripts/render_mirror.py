"""Render the Swift mirror, github.com/logolabs/inkvec-swift, from packages/swift.

    python packages/swift/scripts/render_mirror.py OUT --checksum HEX [--url URL]
    python packages/swift/scripts/render_mirror.py OUT --xcframework PATH [--with-tests]

Writes OUT/Package.swift from packages/swift/mirror/Package.swift.in with the C library as a
binary target: the release URL of InkvecFFI.xcframework.zip and its SwiftPM checksum (what
is pushed to the mirror), or a local XCFramework, copied into OUT, to test the manifest
before anything is released. Copies the Swift sources, the C module the manifest uses on
Linux, README.md, LICENSE and NOTICE. --with-tests adds the tests; they read the contract
from this repository (found through INKVEC_REPO_ROOT).

The version is the workspace's (root Cargo.toml); the default URL is the GitHub release of
the tag `v<version>` of logolabs/inkvec. OUT is emptied first, except for `.git`, so it ends
up holding exactly the mirror's tree. Standard library only; Python 3.9+.
"""

from __future__ import annotations

import argparse
import re
import shutil
import sys
from pathlib import Path

PACKAGE = Path(__file__).resolve().parents[1]
ROOT = PACKAGE.parents[1]
TEMPLATE = PACKAGE / "mirror" / "Package.swift.in"
RELEASES = "https://github.com/logolabs/inkvec/releases/download"
ZIP = "InkvecFFI.xcframework.zip"

GITIGNORE = ".build/\n.swiftpm/\nPackage.resolved\n"


def workspace_version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    section = text.split("[workspace.package]", 1)[1]
    m = re.search(r'^version\s*=\s*"([^"]+)"', section, re.M)
    if not m:
        raise SystemExit("no version in [workspace.package] of Cargo.toml")
    return m.group(1)


def manifest(version: str, binary_target: str, with_tests: bool) -> str:
    tests = (
        '\n        .testTarget(name: "InkvecTests", dependencies: ["Inkvec"], path: "Tests/InkvecTests"),'
        if with_tests
        else ""
    )
    text = TEMPLATE.read_text(encoding="utf-8").replace("\r\n", "\n")
    for key, value in (("@VERSION@", version), ("@BINARY_TARGET@", binary_target), ("@TEST_TARGET@", tests)):
        if key not in text:
            raise SystemExit(f"{TEMPLATE.name}: placeholder {key} missing")
        text = text.replace(key, value)
    return text


def clear(out: Path) -> None:
    """Empty OUT except for .git, refusing anything that is not a mirror tree or empty."""
    out = out.resolve()
    if out == ROOT or ROOT.is_relative_to(out) or out.is_relative_to(PACKAGE):
        raise SystemExit(f"refusing to render into {out}")
    if not out.exists():
        out.mkdir(parents=True)
        return
    entries = [p for p in out.iterdir() if p.name != ".git"]
    if entries and not (out / "Package.swift").exists():
        raise SystemExit(f"{out} is neither empty nor a mirror (no Package.swift); refusing to empty it")
    for p in entries:
        if p.is_dir() and not p.is_symlink():
            shutil.rmtree(p)
        else:
            p.unlink()


def copy_tree(src: Path, dst: Path, names: list[str] | None = None) -> None:
    dst.mkdir(parents=True, exist_ok=True)
    for p in sorted(src.iterdir()):
        if names is not None and p.name not in names:
            continue
        if p.is_dir():
            shutil.copytree(p, dst / p.name)
        else:
            shutil.copy2(p, dst / p.name)


def render(out: Path, binary_target: str, with_tests: bool, xcframework: Path | None) -> None:
    version = workspace_version()
    clear(out)
    (out / "Package.swift").write_text(manifest(version, binary_target, with_tests), encoding="utf-8", newline="\n")
    copy_tree(PACKAGE / "Sources" / "Inkvec", out / "Sources" / "Inkvec")
    copy_tree(PACKAGE / "Sources" / "InkvecFFI", out / "Sources" / "InkvecFFI", ["inkvec.h", "module.modulemap"])
    if with_tests:
        copy_tree(PACKAGE / "Tests" / "InkvecTests", out / "Tests" / "InkvecTests")
    if xcframework is not None:
        shutil.copytree(xcframework, out / "InkvecFFI.xcframework", symlinks=True)
    shutil.copy2(PACKAGE / "README.md", out / "README.md")
    for name in ("LICENSE", "NOTICE"):
        shutil.copy2(ROOT / name, out / name)
    (out / ".gitignore").write_text(GITIGNORE, encoding="utf-8", newline="\n")
    print(f"rendered Inkvec {version} for Swift into {out}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("out", type=Path, help="the mirror's working tree (created, or emptied except .git)")
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--checksum", help="SwiftPM checksum of the released zip (swift package compute-checksum)")
    src.add_argument("--xcframework", type=Path, help="a local InkvecFFI.xcframework, for testing")
    ap.add_argument("--url", help=f"URL of the zip (default: {RELEASES}/v<version>/{ZIP})")
    ap.add_argument("--with-tests", action="store_true", help="include Tests/ (not pushed to the mirror)")
    a = ap.parse_args()

    if a.checksum is not None:
        if not re.fullmatch(r"[0-9a-f]{64}", a.checksum):
            raise SystemExit(f"--checksum must be 64 lowercase hex digits, not {a.checksum!r}")
        url = a.url or f"{RELEASES}/v{workspace_version()}/{ZIP}"
        if not url.startswith("https://") or not url.endswith(".zip"):
            raise SystemExit(f"--url must be an https URL of a .zip, not {url!r}")
        target = f'.binaryTarget(\n    name: "InkvecFFI",\n    url: "{url}",\n    checksum: "{a.checksum}"\n)'
        render(a.out, target, a.with_tests, None)
    else:
        if a.url:
            raise SystemExit("--url goes with --checksum")
        xc = a.xcframework.resolve()
        if xc.name != "InkvecFFI.xcframework" or not (xc / "Info.plist").exists():
            raise SystemExit(f"{xc} is not an InkvecFFI.xcframework (no Info.plist)")
        target = '.binaryTarget(name: "InkvecFFI", path: "InkvecFFI.xcframework")'
        render(a.out, target, a.with_tests, xc)
    return 0


if __name__ == "__main__":
    sys.exit(main())
