#!/usr/bin/env python3
"""The before/after showcase and the results summary: one command to refresh both.

    python tools/showcase_data.py --exe target/release/inkvec.exe
        [--run out/crosscompare-competitors-2026-09-25]
        [--brands "M:/AI STORAGE/AITrains/BrandsDataset/dataset/brands"]
        [--work out/showcase] [--vendor tools/vendor]

Inkvec is always traced afresh with `--exe`, so the pictures and the numbers are what the
current engine does; the other engines did not change, so their traces are reused:

* **Benchmark cases** are the competitor run's (`bench/crosscompare_competitors.py`, its
  `results.json` and `cases/<key>/512/`). Inkvec re-traces each case's `input.png` and is
  scored exactly as that script scores (CIEDE2000, SSIM, DISTS and DINO at 1024 against the
  source render, geometry against the artist's file); the competitors' rows are the run's.
* **Brand logos** are the twenty real brand marks the Space showed before 0.2 (an external
  dataset, not in this repository), rendered with the same protocol (square viewBox, 4%
  margin, 512 px on white, scored at 1024). Their competitor traces are made once, with
  the same engines and flags as the competitor run, and kept in `--work`; later runs reuse
  them.

Writes `web/showcase.json` (the summaries and each case's metrics and thumbnail) and
`web/showcase/<key>.json` (a case's raster and every engine's SVG, fetched only when the
case is opened). The Space's presentation page and the Studio's Showcase screen read both.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import io
import json
import re
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WEB = ROOT / "web"

# The engines, in the order the tables list them, with the names the pages print.
ENGINES = [
    ("inkvec", "Inkvec"),
    ("trazor-auto", "Trazor (PhenX/Trazor, MIT)"),
    ("vtracer-1.0-simplify", "VTracer 1.0.0-alpha.4, best flags (--simplify)"),
    ("vtracer-1.0-default", "VTracer 1.0.0-alpha.4 (default)"),
    ("vtracer-default", "VTracer 0.6.15 (PyPI, default)"),
]
# The engines the brand set is traced by: each competitor at its best settings, and the
# VTracer most people have.
GALLERY_ENGINES = ["inkvec", "vtracer-1.0-simplify", "trazor-auto", "vtracer-default"]
# The ones whose SVGs ship for the gallery to show. VTracer's defaults stay in the numbers:
# its SVGs are the largest by far (about 40% of the gallery's bytes) and look like its best
# flags' with more nodes.
SHOWN_ENGINES = ["inkvec", "vtracer-1.0-simplify", "trazor-auto"]

BENCH_GALLERY = [
    ("brands__shinhancard_com", "Shinhan Card", "brand logo"),
    ("brands__sangchaimeter_com", "SCM", "brand logo, halftone dots"),
    ("simple-icons__barclays", "Barclays eagle", "brand icon (Simple Icons)"),
    ("brands__jurlique_com_au", "Jurlique", "brand wordmark"),
    ("noto-emoji__emoji_u1f478_1f3fd", "Princess", "Noto emoji, gradients"),
    ("fluent-emoji__Face_with_medical_mask_Color_face_with_medical_mask_color", "Face with mask", "Fluent emoji, gradients"),
    ("openmoji__1F334", "Palm tree", "OpenMoji"),
]
# The Space's brand set before 0.2, the four it featured first.
BRANDS = [
    "365retailmarkets_com", "115animal_com", "5corunafs_com", "abrinor_fr",
    "3-gis_com", "1jour1actu_com", "9marks_org", "abillionveg_com", "abplastics_com",
    "12onyourside_com", "acs-logistics_at", "acmc_gov_au", "adamo_es", "7forallmankind_eu",
    "5avshop_es", "aandeheikant_be", "724canlidestek_com", "12port_com", "acceleratetech_net",
    "aboutkidshealth_ca",
]
CASE_KEYS = ["de1024", "dists", "dino", "coordinates", "geometry_ratio", "paths", "bytes", "seconds", "gradients"]


def sha(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def data_url(path: Path, size: int | None = None) -> str:
    from PIL import Image

    im = Image.open(path)
    if size:
        im = im.convert("RGB")
        im.thumbnail((size, size))
    buf = io.BytesIO()
    im.save(buf, "PNG", optimize=True)
    return "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode()


class Scorer:
    """`bench/crosscompare_competitors.py`'s scoring, row for row."""

    def __init__(self, exe: Path, vendor: Path | None):
        sys.path[:0] = [str(ROOT), str(ROOT / "bench")]
        import crosscompare_competitors as c  # noqa: E402

        self.c = c
        c.EXE = exe
        if vendor:
            c.VTRACER10 = vendor / "vtracer-1.0" / "extracted" / "vtracer.exe"
            c.TRAZOR_DIR = vendor / "trazor"
            c.TRAZOR_CLI = c.TRAZOR_DIR / "trace-cli.ts"
        self.backbone = "dinov3" if c.mdino.available("dinov3") else "dinov2"

    def ref(self, truth_png: Path):
        import numpy as np
        from PIL import Image

        return np.asarray(Image.open(truth_png).convert("RGB"), dtype=np.float64) / 255.0

    def trace_and_score(self, engine: str, input_png: Path, out_svg: Path, ref, gt: dict) -> dict:
        c = self.c
        t = time.perf_counter()
        c.trace_one(engine, input_png, out_svg)
        row: dict = {"engine": engine, "seconds": time.perf_counter() - t}
        return {**row, **self.score(out_svg.read_text(encoding="utf-8"), ref, gt)}

    def score(self, svg: str, ref, gt: dict) -> dict:
        c = self.c
        row = dict(c.cc.structure(svg))
        pred = c.cc.rgb(svg, 1024)
        de = c.deltaE_ciede2000(c.rgb2lab(ref), c.rgb2lab(pred))
        row.update(
            de1024=float(de.mean()),
            ssim1024=float(c.structural_similarity(ref, pred, data_range=1.0, channel_axis=2)),
            dists=float(c.mraster.dists_distance(ref, pred)),
            dino=float(c.mdino.dino_score(ref, pred, backbone=self.backbone)),
            geometry_ratio=row["geometry_params"] / max(gt["geometry_params"], 1),
        )
        return row


def summarise(rows: list[dict], engine: str) -> dict | None:
    ok = [r for r in rows if r["engine"] == engine and r.get("de1024") is not None]
    if not ok:
        return None
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


def wins_of(rows: list[dict]) -> dict[str, int]:
    wins: dict[str, int] = {}
    for key in sorted({r["key"] for r in rows}):
        scored = [r for r in rows if r["key"] == key and r.get("de1024") is not None]
        if scored:
            best = min(scored, key=lambda r: r["de1024"])
            wins[best["engine"]] = wins.get(best["engine"], 0) + 1
    return wins


def case_file(key: str, input_png: Path, svgs: dict[str, Path]) -> None:
    (WEB / "showcase").mkdir(exist_ok=True)
    body = {"input": data_url(input_png), "svg": {e: p.read_text(encoding="utf-8") for e, p in svgs.items() if e in SHOWN_ENGINES}}
    (WEB / "showcase" / f"{key}.json").write_text(json.dumps(body, separators=(",", ":")), encoding="utf-8", newline="\n")


def case_entry(key, label, what, gt, input_png, by: dict[str, dict]) -> dict:
    return {
        "key": key,
        "label": label,
        "what": what,
        "artist": {"paths": gt["paths"], "coordinates": gt["coordinates"], "geometry_params": gt["geometry_params"]},
        "thumb": data_url(input_png, 96),
        "m": {e: {k: by[e].get(k) for k in CASE_KEYS} for e in GALLERY_ENGINES if e in by},
    }


def benchmark(s: Scorer, run: Path, work: Path) -> tuple[list[dict], list[dict]]:
    """The competitor run with Inkvec re-traced: every row, and the gallery's cases."""
    rows = json.loads((run / "results.json").read_text(encoding="utf-8"))
    keep = [r for r in rows if r["engine"] != "inkvec"]
    fresh = []
    for key in sorted({r["key"] for r in rows}):
        base = next(r for r in rows if r["key"] == key)
        folder = run / "cases" / key / "512"
        out = work / "bench" / key
        out.mkdir(parents=True, exist_ok=True)
        ref = s.ref(run / "cases" / key / "truth.png")
        row = {k: base[k] for k in ("family", "name", "key", "source", "source_sha256", "gt")}
        row.update(s.trace_and_score("inkvec", folder / "input.png", out / "inkvec.svg", ref, base["gt"]))
        fresh.append(row)
        print(f"  bench {key}: dE {row['de1024']:.3f} (was {next(r['de1024'] for r in rows if r['key'] == key and r['engine'] == 'inkvec'):.3f})", flush=True)
    all_rows = keep + fresh
    gallery = []
    for key, label, what in BENCH_GALLERY:
        by = {r["engine"]: r for r in all_rows if r["key"] == key}
        folder = run / "cases" / key / "512"
        svgs = {e: (work / "bench" / key / "inkvec.svg") if e == "inkvec" else folder / f"{e}.svg" for e in GALLERY_ENGINES}
        case_file(key, folder / "input.png", svgs)
        gallery.append(case_entry(key, label, what, by["inkvec"]["gt"], folder / "input.png", by))
    return all_rows, gallery


def brands(s: Scorer, dataset: Path, work: Path) -> tuple[list[dict], list[dict]]:
    """The brand set: rendered, competitors traced once and kept, Inkvec always afresh."""
    c = s.c
    rows, gallery = [], []
    for name in BRANDS:
        src = dataset / name / "logo_1.svg"
        key = "brand__" + re.sub(r"[^a-zA-Z0-9_.-]", "_", name)
        out = work / "brands" / key
        out.mkdir(parents=True, exist_ok=True)
        original = src.read_text(encoding="utf-8-sig")
        truth = c.render.fit_viewbox(c.render.normalize_svg(original)[0], 512, 512, 0.04)
        if not (out / "truth.png").exists():
            c.cc.png(c.cc.rgb(truth, 1024), out / "truth.png")
            c.cc.png(c.cc.rgb(truth, 512), out / "input.png")
        ref = s.ref(out / "truth.png")
        gt = c.cc.structure(original)
        by = {}
        for engine in GALLERY_ENGINES:
            cached = out / f"{engine}.json"
            if engine != "inkvec" and cached.exists() and (out / f"{engine}.svg").exists():
                row = json.loads(cached.read_text(encoding="utf-8"))
            else:
                row = s.trace_and_score(engine, out / "input.png", out / f"{engine}.svg", ref, gt)
                if engine != "inkvec":
                    cached.write_text(json.dumps(row), encoding="utf-8")
            row.update(key=key, name=name, family="brands", gt=gt)
            rows.append(row)
            by[engine] = row
        label = name.rsplit("_", 1)
        label = f"{label[0]}.{label[1]}" if len(label) == 2 else name
        case_file(key, out / "input.png", {e: out / f"{e}.svg" for e in GALLERY_ENGINES})
        gallery.append(case_entry(key, label, "brand logo", gt, out / "input.png", by))
        print(f"  brand {name}: " + ", ".join(f"{e} {by[e]['de1024']:.3f}" for e in GALLERY_ENGINES), flush=True)
    return rows, gallery


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--exe", required=True, type=Path, help="the inkvec binary to trace with")
    ap.add_argument("--run", type=Path, default=ROOT / "out" / "crosscompare-competitors-2026-09-25")
    ap.add_argument("--brands", type=Path, default=Path("M:/AI STORAGE/AITrains/BrandsDataset/dataset/brands"))
    ap.add_argument("--work", type=Path, default=ROOT / "out" / "showcase")
    ap.add_argument("--vendor", type=Path, default=ROOT / "tools" / "vendor", help="where vtracer 1.0 and Trazor are")
    a = ap.parse_args()
    exe = a.exe.resolve()
    s = Scorer(exe, a.vendor if a.vendor.exists() else None)
    manifest = json.loads((a.run / "manifest.json").read_text(encoding="utf-8"))
    for old in (WEB / "showcase").glob("*.json") if (WEB / "showcase").exists() else []:
        old.unlink()

    print("benchmark cases, Inkvec re-traced:", flush=True)
    bench_rows, bench_gallery = benchmark(s, a.run, a.work)
    print("brand logos:", flush=True)
    brand_rows, brand_gallery = brands(s, a.brands, a.work)

    git = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    date = "-".join(a.run.name.rsplit("-", 3)[-3:])
    out = {
        "traced": time.strftime("%Y-%m-%d"),
        "inkvec": {"git": git, "sha256": sha(exe)},
        "run": a.run.name,
        "date": date,
        "seed": manifest.get("seed"),
        "cases": len({r["key"] for r in bench_rows}),
        "families": sorted({r["family"] for r in bench_rows}),
        "gallery_engines": SHOWN_ENGINES,
        "engines": [{"id": e, "name": n, **(summarise(bench_rows, e) or {})} for e, n in ENGINES],
        "best_de_wins": wins_of(bench_rows),
        "brands": {
            "cases": len(BRANDS),
            "engines": [{"id": e, "name": n, **summarise(brand_rows, e)} for e, n in ENGINES if summarise(brand_rows, e)],
            "best_de_wins": wins_of(brand_rows),
        },
        "sets": [
            {"id": "brands", "label": "Brand logos", "cases": brand_gallery},
            {"id": "bench", "label": "Benchmark cases", "cases": bench_gallery},
        ],
    }
    (WEB / "showcase.json").write_text(json.dumps(out, separators=(",", ":")), encoding="utf-8", newline="\n")
    per_case = sum(p.stat().st_size for p in (WEB / "showcase").glob("*.json"))
    print(f"wrote web/showcase.json ({(WEB / 'showcase.json').stat().st_size / 1024:.0f} KB) and "
          f"{len(list((WEB / 'showcase').glob('*.json')))} case files ({per_case / 1024:.0f} KB)")
    for title, block in (("benchmark", out), ("brands", out["brands"])):
        print(f"{title}: best dE00 {block['best_de_wins']}")
        for e in block["engines"]:
            if "de_mean" in e:
                print(f"  {e['id']:22s} mean {e['de_mean']:.3f} median {e['de_median']:.3f} DISTS {e['dists']:.4f} "
                      f"DINO {e['dino']:.4f} geometry {e['geometry_mean']:.2f}x {e['coordinates']:.0f} coords "
                      f"{e['seconds_median']:.2f} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
