#!/usr/bin/env python3
"""Read the advanced drawer's controls out of src-tauri/src/options.rs into controls.json.

The browser mock (`mock.ts`) has no Rust behind it, so it takes the control table from the
one place it is defined rather than keeping a second copy that can drift.
"""
import json
import re
from pathlib import Path

here = Path(__file__).resolve().parent
src = (here.parent / "src-tauri" / "src" / "options.rs").read_text(encoding="utf-8")

start = src.index("pub const CONTROLS")
body = src[start:]
end = body.index("\n];")
body = body[:end]

controls = []
for block in re.split(r"\n    Control \{", body)[1:]:
    def field(name):
        m = re.search(rf"\b{name}: (.+?),\n", block)
        return m.group(1).strip() if m else None

    def text(name):
        m = re.search(rf'\b{name}: "((?:[^"\\]|\\.)*)"', block)
        return m.group(1) if m else ""

    stops = [
        {"at": float(a), "label": b}
        for a, b in re.findall(r'Stop \{ at: ([\d.]+), label: "([^"]*)" \}', block)
    ]
    controls.append(
        {
            "group": text("group"),
            "key": text("key"),
            "label": text("label"),
            "unit": text("unit"),
            "kind": field("kind").split("::")[1].lower(),
            "min": float(field("min")),
            "max": float(field("max")),
            "curve": float(field("curve")),
            "decimals": int(field("decimals")),
            "stops": stops,
            "help": text("help").replace('\\"', '"'),
        }
    )

(here / "controls.json").write_text(json.dumps(controls, indent=1), encoding="utf-8")
print(f"{len(controls)} controls")
