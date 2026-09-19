"""Regenerate every language artifact from bindings/options.schema.json.

    python bindings/codegen/generate.py           # write
    python bindings/codegen/generate.py --check   # exit 1 if anything is out of date

The schema itself is written by the Rust facade: `cargo test -p inkvec` regenerates it from
`inkvec::Options` (and fails once so the change is reviewed). Run this afterwards.

Each generator is a module in this folder with `render(options) -> {path: text}`, reading
the options through `schema.load()`. To add a language, write one (see python_stub.py) and
list it in GENERATORS. Standard library only; Python 3.9+.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import csharp  # noqa: E402
import docs_tables  # noqa: E402
import java  # noqa: E402
import openapi  # noqa: E402
import golang  # noqa: E402
import python_stub  # noqa: E402
import schema  # noqa: E402
import swift  # noqa: E402
import typescript  # noqa: E402

GENERATORS = [python_stub, typescript, java, csharp, swift, openapi, golang, docs_tables]


def outputs() -> dict[Path, str]:
    options = schema.load()
    out: dict[Path, str] = {}
    for gen in GENERATORS:
        out.update(gen.render(options))
    return out


def stale() -> list[Path]:
    """Generated files whose committed text differs from what the schema produces."""
    bad = []
    for path, text in outputs().items():
        current = path.read_text(encoding="utf-8").replace("\r\n", "\n") if path.exists() else None
        if current != text:
            bad.append(path)
    return bad


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="fail instead of writing")
    a = ap.parse_args()
    if a.check:
        bad = stale()
        for p in bad:
            print(f"out of date: {p.relative_to(schema.ROOT).as_posix()}")
        if bad:
            print("run: python bindings/codegen/generate.py")
            return 1
        print("bindings are up to date with bindings/options.schema.json")
        return 0
    for path, text in outputs().items():
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            f.write(text)
        print(f"wrote {path.relative_to(schema.ROOT).as_posix()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
