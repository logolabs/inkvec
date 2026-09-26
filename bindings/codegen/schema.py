"""The options schema as plain Python data, shared by every generator in this folder.

`bindings/options.schema.json` is written by the Rust facade (`cargo test -p inkvec`) from
`inkvec::Options`; nothing here restates an option. A generator for a new language imports
`load()` and renders from the `Option` list it returns.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Optional

ROOT = Path(__file__).resolve().parents[2]
SCHEMA_PATH = ROOT / "bindings" / "options.schema.json"

# JSON Schema types a generator knows how to render. A new kind of option (a string
# enum, say) fails loudly in `load` until the generators are taught it, rather than being
# rendered as something wrong.
KINDS = ("boolean", "integer", "number", "string")


@dataclass(frozen=True)
class Option:
    """One option, as the schema records it."""

    name: str
    kind: str  # one of KINDS
    default: Any
    description: str
    minimum: Optional[float] = None
    maximum: Optional[float] = None
    exclusive_minimum: Optional[float] = None
    exclusive_maximum: Optional[float] = None
    # The values a string option may take, when the schema fixes them (`enum`).
    choices: Optional[tuple] = None

    def range_text(self) -> str:
        """The allowed range in words, or "" when the type alone bounds it."""
        lo = hi = ""
        if self.exclusive_minimum is not None:
            lo = f"> {_num(self.exclusive_minimum)}"
        elif self.minimum is not None and not (self.kind == "integer" and self.minimum == 0 and self.maximum is None):
            lo = f">= {_num(self.minimum)}"
        if self.exclusive_maximum is not None:
            hi = f"< {_num(self.exclusive_maximum)}"
        elif self.maximum is not None:
            hi = f"<= {_num(self.maximum)}"
        if lo and hi:
            return f"{lo} and {hi}"
        return lo or hi


def _num(x: float) -> str:
    return str(int(x)) if float(x).is_integer() else repr(x)


def load(path: Path = SCHEMA_PATH) -> list[Option]:
    """Every option, in the order the Rust struct declares them."""
    schema = json.loads(path.read_text(encoding="utf-8"))
    out = []
    for name, prop in schema["properties"].items():
        kind = prop.get("type")
        if kind not in KINDS:
            raise SystemExit(
                f"{path.name}: option `{name}` has type {kind!r}; teach bindings/codegen "
                f"to render it (known: {', '.join(KINDS)})"
            )
        if "default" not in prop or not prop.get("description"):
            raise SystemExit(f"{path.name}: option `{name}` needs a default and a description")
        if kind == "string" and not isinstance(prop["default"], str):
            raise SystemExit(f"{path.name}: string option `{name}` has a non-string default")
        out.append(
            Option(
                name=name,
                kind=kind,
                default=prop["default"],
                description=" ".join(prop["description"].split()),
                minimum=prop.get("minimum"),
                maximum=prop.get("maximum"),
                exclusive_minimum=prop.get("exclusiveMinimum"),
                exclusive_maximum=prop.get("exclusiveMaximum"),
                choices=tuple(prop["enum"]) if "enum" in prop else None,
            )
        )
    return out
