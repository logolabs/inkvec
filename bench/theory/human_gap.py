"""How far a trace sits from the artist's own file, before and after a change.

The gate reads `turning` and the parameter ratio as "lower is better". Against the artist
that is only half true: a trace can be smoother or sparser than the file it stands for (on
`quality-web`, lucide, material and simple-icons turn less than their artists do), and then a
rise moves it closer. This script reads both as a distance to the artist, per icon:

* **turning gap** `|T_trace - T_artist|`, where `T` is the gate's `turning` (absolute turning
  of the outlines' control polygons per unit of their length, `inkvec_bench/turning.py`)
  times the canvas width: the trace's viewBox width (the raster's, 400 px on `web`) and the
  artist's viewBox width (128 on noto-emoji). `T` is then in radians per canvas width, and a
  file drawn on a 128-unit grid compares with its trace at 400 px;
* **parameter gap** `|ln ratio|`, where `ratio` is the gate's parameter count of the trace
  (`svgmodel`) over the artist's: 0 when they spend the same, symmetric in "twice as many"
  and "half as many".

For each family it prints the mean of both gaps before and after, and how many icons moved
closer, further, or not at all (an icon whose SVG is byte-identical counts as unchanged).

With `--faces`, also: every fill colour the after-trace has and the before-trace lacks
(farther than 1 dE00 from each of its fills) -- a face the change brought back -- is matched
to the artist's nearest fill colour (CIEDE2000): within 2 dE00 counts as a match, and the
intersection over union of the two colour regions (pixels within 1 dE00 of the colour, on
renders over white at the raster's size) says whether it is the artist's region.

`--before` and `--after` each take an inkvec binary (traced now, at the tier's raster, with
`--mode` and `--extra` passed through) or a directory of SVGs named `<family>__<stem>.svg`
(as `--save DIR` keeps them, in `DIR/before` and `DIR/after`; the gate does not keep its SVGs).

    python3 bench/theory/human_gap.py --before old/inkvec --after target/release/inkvec \\
        --tier web [--mode fast] [--faces] [--jobs 2] [--json out.json] [--save DIR] [--verbose]

Needs numpy, Pillow, svgelements, scikit-image and resvg-py (the harness's renderer).
Not from the literature: a reading of the gate's own axes as distances.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import subprocess
import sys
import tempfile
from collections import defaultdict
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))
os.environ.setdefault("INKVEC_SKIP_DISTS", "1")

#: A fill farther than this from every fill of the before-trace is a face the change added.
NEW_FILL_DE00 = 1.0
#: A recovered face whose colour is within this of an artist fill matches that fill.
MATCH_DE00 = 2.0
#: A pixel belongs to a colour's region when it renders within this of the colour.
REGION_DE00 = 1.0

VIEWBOX = re.compile(r'<svg\b[^>]*?\bviewBox="\s*([-\d.eE+]+)[\s,]+([-\d.eE+]+)[\s,]+([-\d.eE+]+)')
WIDTH = re.compile(r'<svg\b[^>]*?\bwidth="\s*([\d.eE+]+)')
FILL = re.compile(r'\bfill="(#[0-9a-fA-F]{6}|#[0-9a-fA-F]{3})"')


def canvas_width(svg: str) -> float:
    """The root viewBox width, else the root `width`, else 0."""
    m = VIEWBOX.search(svg)
    if m:
        return float(m.group(3))
    m = WIDTH.search(svg)
    return float(m.group(1)) if m else 0.0


def norm_turning(svg: str) -> float:
    """The gate's `turning` times the canvas width: radians per canvas width."""
    from inkvec_bench.turning import turning
    return turning(svg) * canvas_width(svg)


def hex_rgb(h: str) -> tuple[float, float, float]:
    h = h.lstrip("#")
    if len(h) == 3:
        h = "".join(c * 2 for c in h)
    return tuple(int(h[i:i + 2], 16) / 255 for i in (0, 2, 4))


def lab(rgb) -> np.ndarray:
    from skimage.color import rgb2lab
    a = np.asarray(rgb, dtype=np.float64)
    return rgb2lab(a.reshape(-1, 1, 3)).reshape(a.shape)


def de00(lab_a: np.ndarray, lab_b: np.ndarray) -> np.ndarray:
    from skimage.color import deltaE_ciede2000
    a, b = np.broadcast_arrays(lab_a, lab_b)
    return deltaE_ciede2000(a.reshape(-1, 1, 3), b.reshape(-1, 1, 3)).reshape(a.shape[:-1])


def artist_fills(gt: Path) -> list[str]:
    """The solid fill colours of the artist's shapes, styles and inheritance resolved."""
    from svgelements import SVG, Color, Shape
    out = set()
    for e in SVG.parse(str(gt)).elements():
        if isinstance(e, Shape) and isinstance(e.fill, Color) and e.fill.value is not None \
                and e.fill.alpha > 0:
            out.add(f"#{e.fill.red:02x}{e.fill.green:02x}{e.fill.blue:02x}")
    return sorted(out)


def trace_fills(svg: str) -> list[str]:
    return sorted({f.lower() for f in FILL.findall(svg)})


def get_svg(source: str, it: dict, png: Path, td: str, mode: str, extra: list[str]) -> str | None:
    """The SVG `source` gives for one icon: read from a directory, or traced by a binary."""
    p = Path(source)
    name = f"{it['corpus']}__{it['stem']}.svg"
    if p.is_dir():
        f = p / name
        return f.read_text(encoding="utf-8") if f.exists() else None
    out = Path(td) / name
    args = [str(p), str(png), "-o", str(out), "--quiet", *extra]
    if mode == "fast":
        args += ["--mode", "fast"]
    r = subprocess.run(args, capture_output=True)
    return out.read_text(encoding="utf-8") if r.returncode == 0 else None


def faces(before: str, after: str, gt: Path, size: tuple[int, int]) -> list[dict]:
    """The fills `after` adds over `before`, each matched to the artist's nearest fill."""
    from inkvec_bench import render
    b_fills = trace_fills(before)
    added = []
    b_lab = lab([hex_rgb(h) for h in b_fills]) if b_fills else None
    for h in trace_fills(after):
        c = lab([hex_rgb(h)])[0]
        if b_lab is None or de00(b_lab, c[None]).min() > NEW_FILL_DE00:
            added.append((h, c))
    if not added:
        return []
    w, hgt = size
    img_t = lab(render.composite(render.render(after, w, hgt)))
    gsvg = gt.read_text(encoding="utf-8")
    img_a = lab(render.composite(render.render(gsvg, w, hgt)))
    a_fills = artist_fills(gt)
    a_lab = lab([hex_rgb(x) for x in a_fills]) if a_fills else None
    out = []
    for h, c in added:
        rec = {"fill": h, "pixels": int((de00(img_t, c) <= REGION_DE00).sum())}
        if a_lab is not None:
            d = de00(a_lab, c[None])
            k = int(np.argmin(d))
            rec.update(artist=a_fills[k], de00=float(d[k]), match=bool(d[k] <= MATCH_DE00))
            mt = de00(img_t, c) <= REGION_DE00
            ma = de00(img_a, a_lab[k]) <= REGION_DE00
            union = int((mt | ma).sum())
            rec["iou"] = float((mt & ma).sum() / union) if union else 0.0
        out.append(rec)
    return out


def one(args) -> dict:
    tier, it, before, after, mode, extra, want_faces, save = args
    import svgeval
    from inkvec_bench import svgmodel
    from PIL import Image
    svgeval.set_tier(tier)
    png, gt = svgeval.item_paths(it)
    key = f"{it['corpus']}/{it['stem']}"
    if not png.exists() or not gt.exists():
        return {"key": key, "fail": "missing input"}
    with tempfile.TemporaryDirectory() as td:
        Path(td, "b").mkdir()
        Path(td, "a").mkdir()
        sb = get_svg(before, it, png, td + "/b", mode, extra)
        sa = get_svg(after, it, png, td + "/a", mode, extra)
    if sb is None or sa is None:
        return {"key": key, "fail": "no trace"}
    if save:
        for tag, svg in (("before", sb), ("after", sa)):
            d = Path(save) / tag
            d.mkdir(parents=True, exist_ok=True)
            (d / f"{it['corpus']}__{it['stem']}.svg").write_text(svg, encoding="utf-8")
    gsvg = gt.read_text(encoding="utf-8")
    gt_params = max(1, it["gt_params"])
    rec = {"key": key, "family": it["corpus"], "changed": sb != sa,
           "artist_T": norm_turning(gsvg)}
    for tag, svg in (("before", sb), ("after", sa)):
        n = svgmodel.parse(svg).n_params
        rec[tag] = {"T": norm_turning(svg), "ratio": n / gt_params}
        rec[tag]["turning_gap"] = abs(rec[tag]["T"] - rec["artist_T"])
        rec[tag]["param_gap"] = abs(math.log(max(rec[tag]["ratio"], 1e-9)))
    if want_faces and rec["changed"]:
        with Image.open(png) as im:
            size = im.size
        rec["faces"] = faces(sb, sa, gt, size)
    return rec


def report(rows: list[dict]) -> None:
    fam = defaultdict(list)
    for r in rows:
        fam[r["family"]].append(r)
    print("turning normalised to canvas width (radians per canvas width), mean over icons:")
    print(f"  artist {np.mean([r['artist_T'] for r in rows]):.2f}  "
          f"before {np.mean([r['before']['T'] for r in rows]):.2f}  "
          f"after {np.mean([r['after']['T'] for r in rows]):.2f}")
    for gap in ("turning_gap", "param_gap"):
        print(f"\n{gap}: mean before -> after (all icons | changed icons), "
              f"changed icons closer / further / level")
        for f in sorted(fam) + ["ALL"]:
            rs = rows if f == "ALL" else fam[f]
            ch = [r for r in rs if r["changed"]]
            b = np.mean([r["before"][gap] for r in rs])
            a = np.mean([r["after"][gap] for r in rs])
            d = [r["after"][gap] - r["before"][gap] for r in ch]
            closer = sum(x < -1e-9 for x in d)
            further = sum(x > 1e-9 for x in d)
            cb = np.mean([r["before"][gap] for r in ch]) if ch else float("nan")
            ca = np.mean([r["after"][gap] for r in ch]) if ch else float("nan")
            print(f"  {f:14s} n={len(rs):3d}  {b:7.3f} -> {a:7.3f}  | changed {len(ch):3d}: "
                  f"{cb:7.3f} -> {ca:7.3f}  closer {closer:3d} further {further:3d} "
                  f"level {len(ch) - closer - further:3d}")
    fr = [(r["key"], x) for r in rows for x in r.get("faces", [])]
    if fr:
        print(f"\nfaces the change added: {len(fr)}")
        by = defaultdict(list)
        for k, x in fr:
            by[k.split("/")[0]].append(x)
        for f in sorted(by):
            xs = by[f]
            m = [x for x in xs if x.get("match")]
            iou = np.mean([x["iou"] for x in m]) if m else float("nan")
            print(f"  {f:14s} added {len(xs):3d}  within {MATCH_DE00} dE00 of an artist fill "
                  f"{len(m):3d}  mean IoU of those {iou:.2f}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--before", required=True, help="inkvec binary or directory of SVGs")
    ap.add_argument("--after", required=True, help="inkvec binary or directory of SVGs")
    ap.add_argument("--tier", default="web")
    ap.add_argument("--mode", default="quality", choices=("quality", "fast"))
    ap.add_argument("--extra", default="", help="more arguments for a traced binary, comma-separated")
    ap.add_argument("--icons", default="", help="family/stem,... (default: the gate's screen set)")
    ap.add_argument("--faces", action="store_true")
    ap.add_argument("--jobs", type=int, default=2)
    ap.add_argument("--json", type=Path)
    ap.add_argument("--save", default="", help="keep both sides' SVGs in DIR/before and DIR/after")
    ap.add_argument("--verbose", action="store_true", help="one line per changed icon")
    a = ap.parse_args()
    import svgeval
    svgeval.set_tier(a.tier)
    items = svgeval.load_sets()["screen"]
    if a.icons:
        want = set(a.icons.split(","))
        items = [i for i in items if f"{i['corpus']}/{i['stem']}" in want]
    extra = [x for x in a.extra.split(",") if x]
    jobs = [(a.tier, it, a.before, a.after, a.mode, extra, a.faces, a.save) for it in items]
    with ProcessPoolExecutor(a.jobs) as ex:
        rows = list(ex.map(one, jobs))
    bad = [r for r in rows if "fail" in r]
    for r in bad:
        print("FAIL", r["key"], r["fail"])
    rows = [r for r in rows if "fail" not in r]
    if a.verbose:
        for r in rows:
            if r["changed"]:
                b, f = r["before"], r["after"]
                print(f"{r['key']:52s} T artist {r['artist_T']:6.2f} {b['T']:6.2f} -> {f['T']:6.2f}  "
                      f"ratio {b['ratio']:5.2f} -> {f['ratio']:5.2f}"
                      + "".join(f"  +{x['fill']}~{x.get('artist')} {x.get('de00', 0):.1f}dE "
                                f"IoU {x.get('iou', 0):.2f}" for x in r.get("faces", [])))
    report(rows)
    if a.json:
        a.json.write_text(json.dumps(rows, indent=1), encoding="utf-8")


if __name__ == "__main__":
    main()
