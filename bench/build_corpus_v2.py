"""Build the ~1k-icon evaluation corpus with ground truth, stratified by family.

The 200-icon corpus is 90 % emoji. A loop that runs thousands of trials against it
learns emoji idioms (soft shadows, skin-tone radials) that a Material glyph, a Lucide
stroke icon or a brand mark never exercise. This pulls seven families with licences that
allow it, filters out what cannot be ground truth (text, embedded rasters, empty renders,
pathological sizes), renders the 128 px intake, and writes `bench/devset_v2.json` with
per-family splits:

    dev     — selection signal for the evolve loop (8 per family, ~60)
    held_a  — promotion gate (25 per family, ~200)
    held_b  — sealed; reports only (25 per family, ~200)
    full    — everything (~1000)

Sampling is seeded so the corpus can be rebuilt exactly. Run from the repo root:

    python bench/build_corpus_v2.py               # fetch + filter + render + split
    python bench/build_corpus_v2.py --no-fetch    # re-filter / re-split what is on disk
"""
from __future__ import annotations

import argparse
import json
import random
import re
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
import requests

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from inkvec_bench import config, render, svgmodel  # noqa: E402
from inkvec_bench.corpora import fetch as F  # noqa: E402

# Extra families on top of inkvec_bench.corpora.fetch.SOURCES.
F.SOURCES.update({
    "lucide": F.Source("lucide", "lucide-icons/lucide", "main", "icons", "ISC"),
    "simple-icons": F.Source("simple-icons", "simple-icons/simple-icons", "develop", "icons",
                             "CC0-1.0"),
    "openmoji": F.Source("openmoji", "hfg-gmuend/openmoji", "master", "color/svg",
                         "CC-BY-SA-4.0"),
})

# family -> how many to *keep*; fetch oversamples by OVERSAMPLE to survive the filter.
TARGETS = {
    "noto-emoji": 170,     # gradients, translucency, soft shadows
    "twemoji": 170,        # flat, thin strokes, many faces
    "openmoji": 140,       # flat colour with outlines
    "material-icons": 160, # flat monochrome glyphs
    "lucide": 160,         # stroke-only icons
    "simple-icons": 160,   # brand marks, monochrome (the "logo" family with GT)
    # fluent-emoji was tried and dropped: 166 of 175 icons are built from SVG filters.
}
OVERSAMPLE = 1.35
SPLIT = dict(dev=8, held_a=25, held_b=25)
SMALL_SPLIT = dict(dev=3, held_a=6, held_b=6)   # families with < 60 items (synthetic)
TIER = 128
# The intake is rendered at SUPERSAMPLE x TIER and box-filtered down, in straight alpha.
# resvg's own anti-aliasing quantises edge coverage to four levels (0, 1/4, 1/2, 3/4,
# 1), which fixes an edge's position to +-0.125 px before any tracer sees it; real
# assets carry 256-level coverage. 8x8 samples give 64 levels per pixel from geometry
# alone and 256 after the 8-bit average. Directory name: "<TIER>ss".
SUPERSAMPLE = 8
MAX_PARAMS = 12000    # keeps evaluation time bounded; the old dev had one 46k-param outlier
MIN_PARAMS = 4
MIN_COVERAGE = 0.005  # fraction of non-transparent pixels at 128 px
MIN_FAMILY = 15       # a family too small to split is dropped rather than over-weighted

# Filters (blur, inner shadow, blend modes) make the rendered truth something no flat-
# partition tracer can reproduce; 166 of 175 fluent-emoji use them. Not ground truth.
REJECT = re.compile(
    r"<text\b|<image\b|data:image|<foreignObject\b|<video\b|<audio\b|<filter\b|<fe[A-Z]", re.I)


def fetch_family(name: str, limit: int, seed: int, workers: int = 8) -> str:
    """Like corpora.fetch.fetch but downloads in parallel and returns a note."""
    src = F.SOURCES[name]
    dest = config.CORPUS_SVG_DIR / name
    dest.mkdir(parents=True, exist_ok=True)
    try:
        paths = F._list_tree(src)
    except Exception as e:
        return f"listing failed: {type(e).__name__}: {e}"
    if not paths:
        return "no SVGs listed"
    rng = random.Random(seed)
    sample = sorted(paths if len(paths) <= limit else rng.sample(paths, limit))

    def get(p: str) -> bool:
        stem = re.sub(r"[^A-Za-z0-9._-]+", "_", p[len(src.path):].strip("/"))
        target = dest / stem
        if target.exists():
            return True
        try:
            r = requests.get(f"https://raw.githubusercontent.com/{src.repo}/{src.ref}/{p}",
                             timeout=F.TIMEOUT)
            r.raise_for_status()
            target.write_bytes(r.content)
            return True
        except Exception:
            return False

    with ThreadPoolExecutor(workers) as ex:
        ok = sum(ex.map(get, sample))
    (dest / "manifest.json").write_text(json.dumps({
        "name": src.name, "repo": src.repo, "ref": src.ref, "path": src.path,
        "licence": src.licence, "seed": seed, "limit": limit, "available": len(paths),
        "written": ok}, indent=2), encoding="utf-8")
    return f"{ok}/{len(sample)} files (of {len(paths)} available)"


def render_supersampled(svg: str, size: int, ss: int) -> bytes:
    """Exact-coverage raster: render at `ss` times the size, average `ss x ss` blocks of
    premultiplied colour and alpha, unpremultiply, and encode as 8-bit straight RGBA."""
    from PIL import Image as _Image
    rgba = render.render(svg, size * ss, size * ss)  # straight RGBA float32 in [0, 1]
    a = rgba[..., 3:4]
    pre = np.concatenate([rgba[..., :3] * a, a], axis=-1)
    blk = pre.reshape(size, ss, size, ss, 4).mean(axis=(1, 3))
    alpha = blk[..., 3:4]
    rgb = np.where(alpha > 1e-6, blk[..., :3] / np.maximum(alpha, 1e-6), 0.0)
    out = np.concatenate([rgb, alpha], axis=-1)
    img = _Image.fromarray((np.clip(out, 0, 1) * 255 + 0.5).astype(np.uint8), "RGBA")
    buf = __import__("io").BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


def qualify(svg_path: Path, family: str, force: bool = False) -> dict | None:
    """Filter one SVG and render its 128 px intake. Returns the devset item or None."""
    try:
        svg = svg_path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return None
    if REJECT.search(svg):
        return None
    info = svgmodel.parse(svg)
    if info.parse_error or not (MIN_PARAMS <= info.n_params <= MAX_PARAMS):
        return None
    png = config.CORPUS_RASTER_DIR / family / f"{TIER}ss" / f"{svg_path.stem}.png"
    png.parent.mkdir(parents=True, exist_ok=True)
    try:
        if force or not png.exists():
            png.write_bytes(render_supersampled(svg, TIER, SUPERSAMPLE))
        rgba = render.load_rgba(png)
    except Exception:
        return None
    if rgba.shape[:2] != (TIER, TIER):
        return None
    if float((rgba[..., 3] > 0.02).mean()) < MIN_COVERAGE:
        png.unlink(missing_ok=True)
        return None
    return {"corpus": family, "stem": svg_path.stem, "gt_params": int(info.n_params)}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--no-fetch", action="store_true")
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--rerender", action="store_true")
    ap.add_argument("--out", default=str(HERE / "devset_v2.json"))
    a = ap.parse_args()

    if not a.no_fetch:
        for fam, n in TARGETS.items():
            note = fetch_family(fam, int(n * OVERSAMPLE), a.seed)
            print(f"  fetch {fam:16s} {note}", flush=True)

    families = list(TARGETS) + ["synthetic"]
    items: dict[str, list[dict]] = {}
    for fam in families:
        d = config.CORPUS_SVG_DIR / fam
        if not d.exists():
            print(f"  {fam}: no SVG dir"); continue
        cands = sorted(d.glob("*.svg"))
        with ThreadPoolExecutor(8) as ex:
            got = [x for x in ex.map(lambda p: qualify(p, fam, a.rerender), cands) if x]
        rng = random.Random(a.seed * 1000 + len(fam))
        rng.shuffle(got)
        keep = got[: TARGETS.get(fam, len(got))]
        if len(keep) < MIN_FAMILY:
            print(f"  {fam:16s} only {len(keep)} qualify -> family dropped", flush=True)
            continue
        keep.sort(key=lambda x: x["stem"])
        items[fam] = keep
        print(f"  {fam:16s} {len(cands):4d} on disk -> {len(got):4d} qualify -> keep {len(keep)}",
              flush=True)

    out = {"seed": a.seed, "tier": f"{TIER}ss", "supersample": SUPERSAMPLE,
           "families": {f: len(v) for f, v in items.items()},
           "dev": [], "held_a": [], "held_b": [], "full": []}
    for fam, lst in items.items():
        rng = random.Random(a.seed * 7919 + len(fam))
        order = list(lst)
        rng.shuffle(order)
        split = SPLIT if len(order) >= 60 else SMALL_SPLIT
        i = 0
        for name, n in split.items():
            out[name] += order[i:i + n]
            i += n
        out["full"] += lst
    for k in ("dev", "held_a", "held_b", "full"):
        out[k].sort(key=lambda x: (x["corpus"], x["stem"]))
    Path(a.out).write_text(json.dumps(out, indent=1), encoding="utf-8")
    print(f"\nwrote {a.out}: " + ", ".join(f"{k} {len(out[k])}" for k in ("dev", "held_a", "held_b", "full")))
    print("families:", out["families"])
    gp = np.array([x["gt_params"] for x in out["full"]])
    print(f"gt_params median {np.median(gp):.0f}, p95 {np.percentile(gp, 95):.0f}, max {gp.max()}")


if __name__ == "__main__":
    main()
