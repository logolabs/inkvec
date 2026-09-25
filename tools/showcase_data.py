#!/usr/bin/env python3
"""The before/after showcase and the results summary, from one competitor run.

    python tools/showcase_data.py [out/crosscompare-competitors-2026-09-25]

Reads what `bench/crosscompare_competitors.py` wrote -- `results.json` (one row per case and
engine), `manifest.json` (the seed and the engine versions) and `cases/<key>/512/` (each
engine's SVG of the case, and the raster it traced) -- and writes `web/showcase.json`. Two
pages read that one file: the Space's presentation page (`web/index.html`) and the Studio's
Showcase screen (`studio/src/views/showcase.ts`, bundled into both the desktop app and
Inkvec Studio Lite). Every number either shows is computed here from `results.json`, never
typed in by hand.

The summary covers all the run's cases; the gallery is the handful in GALLERY, chosen to
show logos (three real brand marks and a brand icon) and the gradients and fine structure
tracers find hardest. Every engine's SVG is included as it was written, so the pages can
show its anchors and handles.
"""

from __future__ import annotations

import base64
import io
import json
import statistics
import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_RUN = ROOT / "out" / "crosscompare-competitors-2026-09-25"
OUT = ROOT / "web" / "showcase.json"

# The engines, in the order the table lists them, with the names the pages print.
ENGINES = [
    ("inkvec", "Inkvec"),
    ("trazor-auto", "Trazor (PhenX/Trazor, MIT)"),
    ("vtracer-1.0-simplify", "VTracer 1.0.0-alpha.4, best flags (--simplify)"),
    ("vtracer-1.0-default", "VTracer 1.0.0-alpha.4 (default)"),
    ("vtracer-default", "VTracer 0.6.15 (PyPI, default)"),
]
# The ones the gallery lets you put beside Inkvec's trace: each competitor at its best settings
# (VTracer's defaults are in the table; its SVGs are the largest and add little to look at).
GALLERY_ENGINES = ["inkvec", "vtracer-1.0-simplify", "trazor-auto"]
GALLERY = [
    ("brands__shinhancard_com", "Shinhan Card", "brand logo"),
    ("brands__sangchaimeter_com", "SCM", "brand logo, halftone dots"),
    ("simple-icons__barclays", "Barclays eagle", "brand icon (Simple Icons)"),
    ("brands__jurlique_com_au", "Jurlique", "brand wordmark"),
    ("noto-emoji__emoji_u1f478_1f3fd", "Princess", "Noto emoji, gradients"),
    ("fluent-emoji__Face_with_medical_mask_Color_face_with_medical_mask_color", "Face with mask", "Fluent emoji, gradients"),
    ("openmoji__1F334", "Palm tree", "OpenMoji"),
]
# Per-case numbers the gallery prints.
CASE_KEYS = ["de1024", "dists", "dino", "coordinates", "geometry_ratio", "paths", "bytes", "seconds", "gradients", "arcs", "cubics", "lines", "primitives"]


def summarise(rows: list[dict], engine: str) -> dict:
    ok = [r for r in rows if r["engine"] == engine and r.get("de1024") is not None]
    col = lambda k: [r[k] for r in ok if r.get(k) is not None]  # noqa: E731
    return {
        "n": len(ok),
        "de_mean": statistics.mean(col("de1024")),
        "de_median": statistics.median(col("de1024")),
        "dists": statistics.mean(col("dists")),
        "dino": statistics.mean(col("dino")),
        "geometry_mean": statistics.mean(col("geometry_ratio")),
        "geometry_median": statistics.median(col("geometry_ratio")),
        "coordinates": statistics.mean(col("coordinates")),
        "seconds_median": statistics.median(col("seconds")),
        "seconds_mean": statistics.mean(col("seconds")),
    }


def png_data_url(path: Path) -> str:
    im = Image.open(path)
    buf = io.BytesIO()
    im.save(buf, "PNG", optimize=True)
    return "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode()


def main() -> int:
    run = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_RUN
    rows = json.loads((run / "results.json").read_text(encoding="utf-8"))
    manifest = json.loads((run / "manifest.json").read_text(encoding="utf-8"))
    cases = sorted({r["key"] for r in rows})

    wins: dict[str, int] = {}
    for key in cases:
        scored = [r for r in rows if r["key"] == key and r.get("de1024") is not None]
        best = min(scored, key=lambda r: r["de1024"])
        wins[best["engine"]] = wins.get(best["engine"], 0) + 1

    date = run.name.rsplit("-", 3)[-3:]
    out = {
        "run": run.name,
        "date": "-".join(date),
        "seed": manifest.get("seed"),
        "cases": len(cases),
        "families": sorted({r["family"] for r in rows}),
        "engines": [{"id": e, "name": n, **summarise(rows, e)} for e, n in ENGINES],
        "best_de_wins": wins,
        "gallery": [],
    }
    for key, label, what in GALLERY:
        folder = run / "cases" / key / "512"
        by = {r["engine"]: r for r in rows if r["key"] == key}
        gt = next(iter(by.values()))["gt"]
        out["gallery"].append({
            "key": key,
            "label": label,
            "what": what,
            "artist": {"paths": gt["paths"], "coordinates": gt["coordinates"], "geometry_params": gt["geometry_params"]},
            "input": png_data_url(folder / "input.png"),
            "engines": {
                e: {"svg": (folder / f"{e}.svg").read_text(encoding="utf-8"), **{k: by[e].get(k) for k in CASE_KEYS}}
                for e in GALLERY_ENGINES
            },
        })
    OUT.write_text(json.dumps(out, separators=(",", ":")), encoding="utf-8", newline="\n")
    print(f"wrote {OUT} ({OUT.stat().st_size / 1024:.0f} KB): {len(cases)} cases, {len(out['gallery'])} in the gallery")
    for e in out["engines"]:
        print(
            f"  {e['id']:22s} mean {e['de_mean']:.3f} median {e['de_median']:.3f} DISTS {e['dists']:.4f} "
            f"DINO {e['dino']:.4f} geometry {e['geometry_mean']:.2f}x (median {e['geometry_median']:.2f}x) "
            f"{e['coordinates']:.0f} coords, {e['seconds_median']:.2f} s median"
        )
    print(f"  best dE00: {wins}")
    for g in out["gallery"]:
        size = len(json.dumps(g))
        print(f"  {g['key']:40s} {size / 1024:.0f} KB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
