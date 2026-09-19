"""Documentation: the options table in every README and guide that lists the options.

Each file keeps its own prose; only the region between the two markers below is rewritten:

    <!-- inkvec:options:begin -->
    ...generated table...
    <!-- inkvec:options:end -->
"""

from __future__ import annotations

from pathlib import Path

from schema import ROOT, Option

BEGIN = "<!-- inkvec:options:begin -->"
END = "<!-- inkvec:options:end -->"

TARGETS = [
    ROOT / "docs" / "BINDINGS.md",
    ROOT / "crates" / "inkvec" / "README.md",
    ROOT / "crates" / "inkvec-py" / "README.md",
    ROOT / "packages" / "npm" / "README.md",
    ROOT / "packages" / "java" / "README.md",
    ROOT / "packages" / "dotnet" / "README.md",
]

KIND_NAMES = {"boolean": "bool", "integer": "integer", "number": "number"}


def _default(o: Option) -> str:
    if o.kind == "boolean":
        return "true" if o.default else "false"
    if o.kind == "integer":
        return str(int(o.default))
    return repr(float(o.default))


def table(options: list[Option]) -> str:
    rows = ["| Option | Type | Default | Range | Meaning |", "|---|---|---|---|---|"]
    for o in options:
        desc = o.description.replace("|", "\\|")
        rows.append(
            f"| `{o.name}` | {KIND_NAMES[o.kind]} | `{_default(o)}` | {o.range_text() or '-'} | {desc} |"
        )
    return "\n".join(rows)


def render(options: list[Option]) -> dict[Path, str]:
    out = {}
    body = table(options)
    for path in TARGETS:
        text = path.read_text(encoding="utf-8").replace("\r\n", "\n")
        start, end = text.find(BEGIN), text.find(END)
        if start < 0 or end < start:
            raise SystemExit(f"{path.relative_to(ROOT)}: missing {BEGIN} / {END} markers")
        head = text[: start + len(BEGIN)]
        out[path] = f"{head}\n{body}\n{text[end:]}"
    return out
