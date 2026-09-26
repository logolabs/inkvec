#!/usr/bin/env python3
"""The before/after showcase and the results summary: one command to refresh both.

    python tools/showcase_data.py --exe target/release/inkvec.exe [--size 1024]
        [--run out/crosscompare-competitors-2026-09-25]
        [--brands "M:/AI STORAGE/AITrains/BrandsDataset/dataset/brands"]
        [--work out/showcase] [--vendor tools/vendor]

Every case is rendered from its source SVG onto white, square, with a 4% margin, at
`--size` pixels (1024 by default: a logo's fine detail -- an ear tip, a serif -- is a few
pixels at 512 and is lost by every engine), traced by every engine from that raster, and
scored at 1024 against the render exactly as `bench/crosscompare_competitors.py` scores
(CIEDE2000, SSIM, DISTS, DINO; geometry against the artist's file). Three sets:

* **Benchmark cases**: the 21 of the competitor run named by `--run` (its seed and
  selection). That run traced them at 512 px and is the benchmark of record; the gallery
  re-runs them at `--size` so its pictures and its numbers are the same traces.
* **Brand logos**: the twenty real brand marks the Space showed before 0.2 and thirty more
  picked by PICK_SEED alone (hash order, a quota per kind of source file: gradients, thin
  lines, multi-colour, flat), from an external dataset not in this repository.
* **More icons and emoji**: three more files from each of the repository's corpora, picked
  by the same seed.

Inkvec is traced afresh on every run with `--exe`. The other engines have not changed, so
their traces are made once and kept in `--work`, under a folder named for the size, so a
trace at one size is never mistaken for another; later runs only re-trace Inkvec.

Writes `web/showcase.json` (the summaries and each case's metrics and thumbnail) and
`web/showcase/<key>.json` (a case's raster, lossless WebP, and the shown engines' SVGs,
fetched only when the case is opened). The Space's presentation page and the Studio's
Showcase screen read both.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import io
import json
import os
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

# Readable names for the benchmark's logos and the cases people ask about; the rest are
# named from their file.
BENCH_LABELS = [
    ("brands__shinhancard_com", "Shinhan Card", "brand logo"),
    ("brands__sangchaimeter_com", "SCM", "brand logo, halftone dots"),
    ("brands__jurlique_com_au", "Jurlique", "brand wordmark"),
    ("simple-icons__barclays", "Barclays eagle", "brand icon"),
    ("simple-icons__slackware", "Slackware", "brand icon"),
    ("noto-emoji__emoji_u1f478_1f3fd", "Princess", "Noto emoji, gradients"),
    ("fluent-emoji__Face_with_medical_mask_Color_face_with_medical_mask_color", "Face with mask", "Fluent emoji, gradients"),
    ("fluent-emoji__Musical_notes_Color_musical_notes_color", "Musical notes", "Fluent emoji, gradients"),
    ("openmoji__1F334", "Palm tree", "OpenMoji"),
]
FAMILY_WHAT = {
    "lucide": "Lucide icon", "material-icons": "Material icon", "simple-icons": "brand icon",
    "twemoji": "Twemoji", "noto-emoji": "Noto emoji", "openmoji": "OpenMoji",
    "fluent-emoji": "Fluent emoji", "synthetic": "synthetic probe", "brands": "brand logo",
}
# The Space's brand set before 0.2, the four it featured first.
BRANDS = [
    "365retailmarkets_com", "115animal_com", "5corunafs_com", "abrinor_fr",
    "3-gis_com", "1jour1actu_com", "9marks_org", "abillionveg_com", "abplastics_com",
    "12onyourside_com", "acs-logistics_at", "acmc_gov_au", "adamo_es", "7forallmankind_eu",
    "5avshop_es", "aandeheikant_be", "724canlidestek_com", "12port_com", "acceleratetech_net",
    "aboutkidshealth_ca",
]
# More, picked by this seed and nothing else, stratified by what the source file is so the
# set is not all flat wordmarks: see `brand_cases`.
PICK_SEED = "showcase-2026-09-26-v1"
BRAND_QUOTA = {"gradients": 8, "thin lines": 7, "multi-colour": 8, "flat": 7}
CORPUS_FAMILIES = ["lucide", "material-icons", "simple-icons", "twemoji", "noto-emoji", "openmoji", "fluent-emoji"]
EXTRA_PER_FAMILY = 3
CASE_KEYS = ["de1024", "dists", "dino", "coordinates", "geometry_ratio", "paths", "bytes", "seconds", "gradients"]


def sha(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def data_url(path: Path, size: int | None = None) -> str:
    """The case's raster as lossless WebP (the pixels exactly, about a third smaller than
    PNG), or a thumbnail of it as lossy WebP."""
    from PIL import Image

    im = Image.open(path).convert("RGB")
    buf = io.BytesIO()
    if size:
        im.thumbnail((size, size), Image.LANCZOS)
        im.save(buf, "WEBP", quality=82, method=6)
    else:
        im.save(buf, "WEBP", lossless=True, quality=100, method=6)
    return "data:image/webp;base64," + base64.b64encode(buf.getvalue()).decode()


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
        "thumb": data_url(input_png, 80),
        "m": {e: {k: by[e].get(k) for k in CASE_KEYS} for e in GALLERY_ENGINES if e in by},
    }


def label_of(key: str) -> tuple[str, str]:
    """A readable name and a kind for a benchmark case key, `family__name`."""
    named = {k: (l, w) for k, l, w in BENCH_LABELS}
    if key in named:
        return named[key]
    family, _, name = key.partition("__")
    return name.replace("_", " ").replace("-", " ")[:40], FAMILY_WHAT.get(family, family)


def benchmark_cases(run: Path) -> list[tuple[str, str, str, Path]]:
    """The competitor run's cases, from the source SVGs it kept: the labelled ones first."""
    rows = json.loads((run / "results.json").read_text(encoding="utf-8"))
    keys = sorted({r["key"] for r in rows})
    labelled = [k for k, _, _ in BENCH_LABELS if k in keys]
    order = labelled + [k for k in keys if k not in labelled]
    return [(k, *label_of(k), run / "cases" / k / "original.svg") for k in order]


def traced_set(s: Scorer, cases: list[tuple[str, str, str, Path]], work: Path, name: str, size: int) -> tuple[list[dict], list[dict]]:
    """One set: each case rendered at `size`, the competitors traced once and kept in
    `work/<size>px/<set>/`, Inkvec always afresh, everything scored at 1024."""
    c = s.c
    rows, gallery = [], []
    for key, label, what, src in cases:
        out = work / f"{size}px" / name / key
        out.mkdir(parents=True, exist_ok=True)
        original = src.read_text(encoding="utf-8-sig")
        if not (out / "truth.png").exists():
            truth = c.render.fit_viewbox(c.render.normalize_svg(original)[0], 512, 512, 0.04)
            c.cc.png(c.cc.rgb(truth, 1024), out / "truth.png")
            c.cc.png(c.cc.rgb(truth, size), out / "input.png")
        ref = s.ref(out / "truth.png")
        gt = c.cc.structure(original)
        by = {}
        try:
            for engine in GALLERY_ENGINES:
                cached = out / f"{engine}.json"
                if engine != "inkvec" and cached.exists() and (out / f"{engine}.svg").exists():
                    row = json.loads(cached.read_text(encoding="utf-8"))
                else:
                    try:
                        row = s.trace_and_score(engine, out / "input.png", out / f"{engine}.svg", ref, gt)
                    except Exception as e:  # noqa: BLE001 - name the engine; its stderr may be empty
                        raise RuntimeError(f"{engine}: {e!r}") from e
                    if engine != "inkvec":
                        cached.write_text(json.dumps(row), encoding="utf-8")
                row.update(key=key, family=name, gt=gt)
                by[engine] = row
        except Exception as e:  # noqa: BLE001 - one engine failing a case drops the case, said aloud
            print(f"  {name} {key}: SKIPPED ({e!r})"[:300], flush=True)
            continue
        rows.extend(by.values())
        case_file(key, out / "input.png", {e: out / f"{e}.svg" for e in GALLERY_ENGINES})
        gallery.append(case_entry(key, label, what, gt, out / "input.png", by))
        print(f"  {name} {key}: " + ", ".join(f"{e} {by[e]['de1024']:.3f}" for e in GALLERY_ENGINES), flush=True)
    return rows, gallery


def classify(svg: str) -> str:
    """A brand file's kind, read from its source, for the stratified pick."""
    if "<linearGradient" in svg or "<radialGradient" in svg:
        return "gradients"
    if re.search(r'stroke-width\s*[:=]\s*"?\s*[0-9.]', svg) and not re.search(r'stroke\s*[:=]\s*"?\s*none', svg):
        return "thin lines"
    fills = {f.lower() for f in re.findall(r"#[0-9a-fA-F]{6}\b", svg)}
    if len(fills) >= 3:
        return "multi-colour"
    return "flat"


def brand_cases(dataset: Path, exclude: set[str]) -> list[tuple[str, str, str, Path]]:
    """The twenty the Space showed before 0.2, then EXTRA_BRANDS more picked by a fixed seed:
    names in the order of sha256(seed + name), size-filtered as the benchmark filters its
    brands (800 to 120,000 bytes), taken until each kind has its quota. Nothing about how
    any engine traces them takes part in the choice."""
    def entry(name: str, what: str) -> tuple[str, str, str, Path]:
        label = name.rsplit("_", 1)
        label = f"{label[0]}.{label[1]}" if len(label) == 2 else name
        return ("brand__" + re.sub(r"[^a-zA-Z0-9_.-]", "_", name), label, what, dataset / name / "logo_1.svg")

    out = [entry(n, "brand logo") for n in BRANDS]
    taken = set(BRANDS) | exclude
    quota = dict(BRAND_QUOTA)
    names = sorted(os.listdir(dataset), key=lambda n: hashlib.sha256((PICK_SEED + n).encode()).hexdigest())
    for name in names:
        if not any(quota.values()):
            break
        src = dataset / name / "logo_1.svg"
        if name in taken or not src.is_file() or not 800 < src.stat().st_size < 120_000:
            continue
        try:
            kind = classify(src.read_text(encoding="utf-8-sig"))
        except (OSError, UnicodeDecodeError):
            continue
        if quota.get(kind, 0) > 0:
            quota[kind] -= 1
            out.append(entry(name, f"brand logo, {kind}"))
    return out


def corpus_cases(exclude: set[str]) -> list[tuple[str, str, str, Path]]:
    """EXTRA_PER_FAMILY more files from each of the repository's icon and emoji corpora, in the
    order of sha256(seed + family + file name), skipping the benchmark's own."""
    out = []
    for family in CORPUS_FAMILIES:
        files = sorted((ROOT / "bench" / "data" / "corpus_svg" / family).glob("*.svg"),
                       key=lambda p: hashlib.sha256((PICK_SEED + family + p.name).encode()).hexdigest())
        n = 0
        for p in files:
            key = re.sub(r"[^a-zA-Z0-9_.-]", "_", f"corpus__{family}__{p.stem}")
            if f"{family}__{p.stem}" in exclude or not 200 < p.stat().st_size < 120_000:
                continue
            out.append((key, p.stem.replace("_", " ").replace("-", " ")[:40], FAMILY_WHAT[family], p))
            n += 1
            if n == EXTRA_PER_FAMILY:
                break
    return out


def block(rows: list[dict], n: int) -> dict:
    return {
        "cases": n,
        "engines": [{"id": e, "name": nm, **summarise(rows, e)} for e, nm in ENGINES if summarise(rows, e)],
        "best_de_wins": wins_of(rows),
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--exe", required=True, type=Path, help="the inkvec binary to trace with")
    ap.add_argument("--run", type=Path, default=ROOT / "out" / "crosscompare-competitors-2026-09-25")
    ap.add_argument("--brands", type=Path, default=Path("M:/AI STORAGE/AITrains/BrandsDataset/dataset/brands"))
    ap.add_argument("--work", type=Path, default=ROOT / "out" / "showcase")
    ap.add_argument("--vendor", type=Path, default=ROOT / "tools" / "vendor", help="where vtracer 1.0 and Trazor are")
    ap.add_argument("--size", type=int, default=1024, help="the raster every engine traces, in pixels (square)")
    a = ap.parse_args()
    exe = a.exe.resolve()
    s = Scorer(exe, a.vendor if a.vendor.exists() else None)
    manifest = json.loads((a.run / "manifest.json").read_text(encoding="utf-8"))
    for old in (WEB / "showcase").glob("*.json") if (WEB / "showcase").exists() else []:
        old.unlink()

    print(f"benchmark cases at {a.size} px:", flush=True)
    bench = benchmark_cases(a.run)
    bench_rows, bench_gallery = traced_set(s, bench, a.work, "bench", a.size)
    run_rows = json.loads((a.run / "results.json").read_text(encoding="utf-8"))
    bench_names = {r["name"] for r in run_rows if r["family"] == "brands"}
    print("brand logos:", flush=True)
    brand_rows, brand_gallery = traced_set(s, brand_cases(a.brands, bench_names), a.work, "brands", a.size)
    print("more icons and emoji:", flush=True)
    corpus_rows, corpus_gallery = traced_set(s, corpus_cases({k for k, *_ in bench}), a.work, "corpus", a.size)

    git = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    date = "-".join(a.run.name.rsplit("-", 3)[-3:])
    out = {
        "traced": time.strftime("%Y-%m-%d"),
        "inkvec": {"git": git, "sha256": sha(exe)},
        "run": a.run.name,
        "date": date,
        "seed": manifest.get("seed"),
        "pick_seed": PICK_SEED,
        "size": a.size,
        "cases": len(bench_gallery),
        "families": sorted({r["family"] for r in run_rows}),
        "gallery_engines": SHOWN_ENGINES,
        "engines": [{"id": e, "name": n, **summarise(bench_rows, e)} for e, n in ENGINES if summarise(bench_rows, e)],
        "best_de_wins": wins_of(bench_rows),
        "brands": block(brand_rows, len(brand_gallery)),
        "corpus": block(corpus_rows, len(corpus_gallery)),
        "trademark_notice": "All third-party trademarks, logos, and brand names are property of their respective owners, used solely for nominative benchmarking demonstration.",
        "sets": [
            {"id": "brands", "label": "Brand logos", "cases": brand_gallery},
            {"id": "bench", "label": "Benchmark cases", "cases": bench_gallery},
            {"id": "corpus", "label": "More icons and emoji", "cases": corpus_gallery},
        ],
    }
    (WEB / "showcase.json").write_text(json.dumps(out, separators=(",", ":")), encoding="utf-8", newline="\n")
    per_case = sum(p.stat().st_size for p in (WEB / "showcase").glob("*.json"))
    print(f"wrote web/showcase.json ({(WEB / 'showcase.json').stat().st_size / 1024:.0f} KB) and "
          f"{len(list((WEB / 'showcase').glob('*.json')))} case files ({per_case / 1024:.0f} KB)")
    for title, b in (("benchmark", out), ("brands", out["brands"]), ("corpus", out["corpus"])):
        print(f"{title}: {b['cases']} cases, best dE00 {b['best_de_wins']}")
        for e in b["engines"]:
            if "de_mean" in e:
                print(f"  {e['id']:22s} mean {e['de_mean']:.3f} median {e['de_median']:.3f} DISTS {e['dists']:.4f} "
                      f"DINO {e['dino']:.4f} geometry {e['geometry_mean']:.2f}x {e['coordinates']:.0f} coords "
                      f"{e['seconds_median']:.2f} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
