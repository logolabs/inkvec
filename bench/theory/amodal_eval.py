"""Amodal completion against the artists' hidden geometry.

The emoji corpora are stacked layers: an artist draws a whole disc and paints a shading
crescent over it, so every element has a full shape, part of it hidden under later ones.
That hidden part is the ground truth the completion stage (`crates/inkvec-cli/src/layers.rs`,
chain R's painter's interval) guesses at. For every screen icon of the emoji families this
script:

1. renders each of the artist's elements alone (every other drawing element hidden), to get
   its full shape as a mask and its colour, and the part of it that shows (its mask minus
   the masks of the elements painted after it);
2. traces the icon in quality mode with `INKVEC_COMPLETION_DEBUG=3`, which reports for every
   face the stage tried whether it was completed or refused, the winning candidate, the
   face's ink, and its element before and after the completion;
3. matches each tried face to the artist's element of the closest colour whose visible part
   it overlaps most;
4. scores the face's element before the completion (the visible crescent the trace would
   write without the stage) and after it against the artist's full shape, by IoU.

It reports the share of tried faces refused, and per candidate kind (primitive, corner,
corner cut, chord, offset, bulge, bridge) how often the completed shape is closer to the
artist's element than the crescent was, with the IoU distributions.

    python3 bench/theory/amodal_eval.py --exe target/release/inkvec [--tier 512ss]
        [--families noto-emoji,twemoji,openmoji] [--workers 2] [--json out.json]
"""

from __future__ import annotations

import argparse
import copy
import json
import os
import re
import subprocess
import sys
import xml.etree.ElementTree as ET
from collections import defaultdict
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))

SVG_NS = "http://www.w3.org/2000/svg"
DRAWING = {"path", "rect", "circle", "ellipse", "polygon", "polyline", "line"}
NONPAINT = {"defs", "clipPath", "mask", "symbol", "linearGradient", "radialGradient", "filter",
            "pattern", "marker", "title", "desc", "metadata", "style"}
#: Colour distance (sRGB, 0-255, Euclidean) within which an artist's element can be the one
#: a face stands for.
COLOUR_TOL = 40.0
#: A win: the completed shape's IoU beats the crescent's by more than this.
WIN = 1e-3
#: Kinds as the stage reports them, grouped as the review names them.
GROUP = {"circle": "primitive", "ellipse": "primitive", "rect": "primitive", "corner": "corner",
         "corner-cut": "corner cut", "chord": "chord", "offset": "offset", "bulge": "bulge",
         "bridge": "bridge"}


def tag(el) -> str:
    return el.tag.split("}")[-1]


def drawing_leaves(root) -> list:
    """The drawing elements in paint order, skipping definitions."""
    out = []

    def walk(node):
        for ch in node:
            t = tag(ch)
            if t in NONPAINT:
                continue
            if t in DRAWING:
                out.append(ch)
            walk(ch)

    walk(root)
    return out


def artist_layers(svg_text: str, size: int):
    """Per drawing element in paint order: (full mask, mean colour, visible mask)."""
    from inkvec_bench import render
    ET.register_namespace("", SVG_NS)
    root = ET.fromstring(svg_text)
    n = len(drawing_leaves(root))
    masks, colours = [], []
    for i in range(n):
        r = copy.deepcopy(root)
        leaves = drawing_leaves(r)
        for j, el in enumerate(leaves):
            if j != i:
                el.set("display", "none")
            else:
                # Alone and opaque: the shape is the mask, whatever its opacity.
                for k in ("opacity", "fill-opacity"):
                    if k in el.attrib:
                        del el.attrib[k]
        img = render.render(ET.tostring(r, encoding="unicode"), size, size)
        m = img[..., 3] > 0.5
        masks.append(m)
        colours.append(img[..., :3][m].mean(axis=0) * 255 if m.any() else np.array([-999.0] * 3))
    visible = []
    above = np.zeros((size, size), bool)
    for i in range(n - 1, -1, -1):
        visible.append(masks[i] & ~above)
        above |= masks[i]
    visible.reverse()
    return masks, colours, visible


def element_mask(el: str, size: int) -> np.ndarray | None:
    """One of our elements (as the stage printed it) drawn alone in the trace's frame."""
    from inkvec_bench import render
    if not el:
        return None
    el = re.sub(r'\sfill="[^"]*"', "", el)
    el = el.replace("/>", ' fill="#000000"/>', 1)
    svg = (f'<svg xmlns="{SVG_NS}" viewBox="-0.5 -0.5 {size} {size}" width="{size}" '
           f'height="{size}">{el}</svg>')
    return render.render(svg, size, size)[..., 3] > 0.5


def iou(a: np.ndarray, b: np.ndarray) -> float:
    u = (a | b).sum()
    return float((a & b).sum() / u) if u else 0.0


def hex_rgb(h: str) -> np.ndarray:
    h = h.lstrip("#")
    return np.array([int(h[i:i + 2], 16) for i in (0, 2, 4)], float)


def one_icon(arg):
    exe, fam, stem, tier = arg
    import svgeval
    svgeval.set_tier(tier)
    png, gt = svgeval.item_paths({"corpus": fam, "stem": stem})
    if not png.exists():
        return []
    from PIL import Image
    size = Image.open(png).size[0]
    env = dict(os.environ, INKVEC_COMPLETION_DEBUG="3")
    out = Path(os.environ.get("TMPDIR", "/tmp")) / f"amodal_{os.getpid()}.svg"
    r = subprocess.run([str(exe), str(png), "-o", str(out), "--quiet", "--mode", "quality"],
                       env=env, capture_output=True, text=True, timeout=900)
    rows = [ln.split("\t") for ln in r.stderr.splitlines() if ln.startswith("completion\t")]
    if not rows:
        return []
    masks, colours, visible = artist_layers(gt.read_text(encoding="utf-8"), size)
    res = []
    for _, face, status, kind, fill, before, after in (x + [""] * (7 - len(x)) for x in rows):
        b = element_mask(before, size)
        if b is None or not b.any():
            continue
        a = element_mask(after, size) if status == "completed" else b
        if a is None:
            continue
        try:
            c = hex_rgb(fill)
        except ValueError:
            continue
        best, best_ov = None, 0
        for i, (m, col, vis) in enumerate(zip(masks, colours, visible)):
            if np.linalg.norm(col - c) > COLOUR_TOL:
                continue
            ov = int((b & vis).sum())
            if ov > best_ov:
                best, best_ov = i, ov
        if best is None or best_ov < 0.5 * b.sum():
            res.append({"icon": f"{fam}/{stem}", "face": int(face), "status": status, "kind": kind,
                        "matched": False})
            continue
        m = masks[best]
        res.append({"icon": f"{fam}/{stem}", "face": int(face), "status": status, "kind": kind,
                    "matched": True, "iou_off": iou(b, m), "iou_on": iou(a, m),
                    "hidden_share": float(1 - visible[best].sum() / max(1, m.sum()))})
    return res


def quartiles(xs):
    if not xs:
        return "-"
    q = np.percentile(xs, [25, 50, 75])
    return f"{q[0]:.3f} / {q[1]:.3f} / {q[2]:.3f}"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--exe", type=Path, required=True)
    ap.add_argument("--tier", default="512ss")
    ap.add_argument("--families", default="noto-emoji,twemoji,openmoji")
    ap.add_argument("--workers", type=int, default=2)
    ap.add_argument("--json", type=Path)
    a = ap.parse_args()
    import svgeval
    fams = set(a.families.split(","))
    items = [it for it in svgeval.load_sets()["screen"] if it["corpus"] in fams]
    jobs = [(a.exe.resolve(), it["corpus"], it["stem"], a.tier) for it in items]
    rows = []
    with ProcessPoolExecutor(a.workers) as ex:
        for r in ex.map(one_icon, jobs):
            rows.extend(r)
    tried = len(rows)
    done = [r for r in rows if r["status"] == "completed"]
    matched = [r for r in done if r["matched"]]
    print(f"{len(items)} icons, {tried} faces tried, {len(done)} completed, "
          f"{tried - len(done)} refused ({(tried - len(done)) / max(1, tried):.1%}); "
          f"{len(matched)} completed faces matched to an artist element")
    by = defaultdict(list)
    for r in matched:
        for k in r["kind"].split("+"):
            by[GROUP.get(k, k)].append(r)
    print(f"{'candidate':12s} {'faces':>6s} {'win':>6s} {'loss':>6s} {'mean dIoU':>10s}   "
          f"{'IoU off q1/med/q3':>24s}   {'IoU on q1/med/q3':>24s}")
    for k in sorted(by):
        rs = by[k]
        d = [r["iou_on"] - r["iou_off"] for r in rs]
        win = sum(x > WIN for x in d) / len(d)
        loss = sum(x < -WIN for x in d) / len(d)
        print(f"{k:12s} {len(rs):6d} {win:6.1%} {loss:6.1%} {np.mean(d):+10.4f}   "
              f"{quartiles([r['iou_off'] for r in rs]):>24s}   {quartiles([r['iou_on'] for r in rs]):>24s}")
    if matched:
        d = [r["iou_on"] - r["iou_off"] for r in matched]
        print(f"{'all':12s} {len(matched):6d} {sum(x > WIN for x in d) / len(d):6.1%} "
              f"{sum(x < -WIN for x in d) / len(d):6.1%} {np.mean(d):+10.4f}   "
              f"{quartiles([r['iou_off'] for r in matched]):>24s}   "
              f"{quartiles([r['iou_on'] for r in matched]):>24s}")
    refused = [r for r in rows if r["status"] == "refused" and r["matched"]]
    if refused:
        print(f"refused faces matched: {len(refused)}, their crescent's IoU q1/med/q3 "
              f"{quartiles([r['iou_off'] for r in refused])}, hidden share of the artist's "
              f"element {quartiles([r['hidden_share'] for r in refused])}")
    if a.json:
        a.json.write_text(json.dumps(rows, indent=1))


if __name__ == "__main__":
    main()
