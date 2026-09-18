"""Render traced output so its defects can be *seen*, by a person or by a vision model.

Scores rank; they do not explain. DISTS 0.093 does not say "this smooth shoulder came out
as eleven two-pixel chords", and a metric can be flat while the geometry is visibly wrong —
which happened here: a junction fix that plainly improved a corner measured as a colour
regression, because the score was taken against the source raster's own antialiasing rather
than against the truth.

So this writes images built for looking at. Every panel is labelled in the image itself, so
a model opening the file has the context without the filename, and the crops go wherever the
error actually is rather than to the middle of the icon.

The panel that matters most for the current defect is the segment map: our own path drawn
over the render with straight segments and curved ones in different colours, and a dot at
every join. Faceting is invisible in a rendered shape and unmistakable there — a smooth
boundary paved in short chords shows up as a dotted line of joins.

Usage:
    python bench/visual_audit.py --set dev --out output/audit
    python bench/visual_audit.py --stems 1f17e 1f343 --zoom 6
    python bench/visual_audit.py --set dev --exe path/to/inkvec.exe -- --precision 0.6

Writes one contact sheet per icon, close-ups of the worst regions, and INDEX.md listing
every file with its measurements, worst first.
"""
from __future__ import annotations

import argparse
import json
import math
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "bench"))
from inkvec_bench import render, svgmodel  # noqa: E402
from inkvec_bench.metrics import raster  # noqa: E402

JND = 2.3
PANEL = 340
GUTTER = 14
LABEL_H = 26


# --------------------------------------------------------------------------- text

def _font(size: int):
    for name in ("consola.ttf", "DejaVuSansMono.ttf", "arial.ttf", "DejaVuSans.ttf"):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            continue
    return ImageFont.load_default()


FONT = _font(13)
FONT_SMALL = _font(11)
FONT_BIG = _font(17)


def label(draw, xy, text, font=FONT, fill=(20, 22, 26), bg=None):
    if bg is not None:
        w = draw.textlength(text, font=font)
        draw.rectangle([xy[0] - 3, xy[1] - 2, xy[0] + w + 3, xy[1] + font.size + 4], fill=bg)
    draw.text(xy, text, font=font, fill=fill)


# --------------------------------------------------------------------------- geometry

@dataclass
class Seg:
    kind: str          # "L" or "C"
    pts: list          # [(x, y), ...] flattened for drawing
    start: tuple
    end: tuple


def parse_segments(d: str) -> list[Seg]:
    """Flatten one path's `d` into drawable segments, keeping line/curve identity."""
    out: list[Seg] = []
    cur = start = None
    toks = re.findall(r"([MLCZmlcz])([^MLCZmlcz]*)", d)
    for cmd, args in toks:
        n = [float(x) for x in re.findall(r"-?\d+\.?\d*(?:[eE]-?\d+)?", args)]
        if cmd in "Mm" and len(n) >= 2:
            cur = (n[0], n[1])
            start = cur
        elif cmd in "Ll" and len(n) >= 2:
            p = (n[0], n[1])
            out.append(Seg("L", [cur, p], cur, p))
            cur = p
        elif cmd in "Cc" and len(n) >= 6:
            p0, p1, p2, p3 = cur, (n[0], n[1]), (n[2], n[3]), (n[4], n[5])
            pts = []
            for i in range(17):
                t = i / 16
                u = 1 - t
                pts.append((
                    u**3 * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t**3 * p3[0],
                    u**3 * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t**3 * p3[1],
                ))
            out.append(Seg("C", pts, p0, p3))
            cur = p3
        elif cmd in "Zz" and cur and start:
            if cur != start:
                out.append(Seg("L", [cur, start], cur, start))
            cur = start
    return out


def svg_segments(svg: str) -> list[Seg]:
    segs = []
    for m in re.finditer(r'<path[^>]*\sd="([^"]+)"', svg):
        for sub in re.split(r"(?=[Mm])", m.group(1)):
            if sub.strip():
                segs.extend(parse_segments(sub))
    return segs


def viewbox(svg: str):
    m = re.search(r'viewBox="([-\d.eE]+)\s+([-\d.eE]+)\s+([-\d.eE]+)\s+([-\d.eE]+)"', svg)
    return tuple(float(m.group(i)) for i in range(1, 5)) if m else (0.0, 0.0, 128.0, 128.0)


# --------------------------------------------------------------------------- panels

def to_img(arr: np.ndarray) -> Image.Image:
    return Image.fromarray((np.clip(arr, 0, 1) * 255).astype(np.uint8))


def checkerboard(size: int, cell: int = 12) -> Image.Image:
    a = np.zeros((size, size, 3), np.uint8)
    a[:] = 255
    yy, xx = np.mgrid[0:size, 0:size]
    a[((yy // cell) + (xx // cell)) % 2 == 1] = 232
    return Image.fromarray(a)


def over_checker(rgba: np.ndarray) -> Image.Image:
    """Composite over a checkerboard so transparency is visible rather than assumed white."""
    h = rgba.shape[0]
    bg = np.asarray(checkerboard(h), np.float32) / 255.0
    a = rgba[:, :, 3:4]
    return to_img(rgba[:, :, :3] * a + bg * (1 - a))


def heatmap(de: np.ndarray) -> Image.Image:
    """Colour error by how visible it is: below a JND stays pale, above it burns."""
    x = np.clip(de / (JND * 4), 0, 1)
    r = np.clip(1.6 * x, 0, 1)
    g = np.clip(1.6 * x - 0.6, 0, 1)
    b = np.clip(2.2 * x - 1.4, 0, 1)
    base = 1.0 - np.clip(de / JND, 0, 1) * 0.12
    img = np.stack([base * (1 - x) + r * x, base * (1 - x) + g * x, base * (1 - x) + b * x], -1)
    return to_img(img)


def segment_map(svg: str, size: int, backdrop: Image.Image | None = None) -> Image.Image:
    """Our own path, drawn so the model class is visible: straight vs curved vs joins.

    This is the panel that shows faceting. A shape can render as a plausible curve while
    being a chain of short chords, and only the segment identity reveals it.
    """
    vb = viewbox(svg)
    sc = size / vb[2]
    img = (backdrop.copy().convert("RGB") if backdrop is not None
           else Image.new("RGB", (size, size), (255, 255, 255)))
    img = Image.blend(img, Image.new("RGB", (size, size), (255, 255, 255)), 0.62)
    dr = ImageDraw.Draw(img)

    def T(p):
        return ((p[0] - vb[0]) * sc, (p[1] - vb[1]) * sc)

    segs = svg_segments(svg)
    for s in segs:
        col = (206, 58, 34) if s.kind == "L" else (26, 96, 176)
        dr.line([T(p) for p in s.pts], fill=col, width=2, joint="curve")
    for s in segs:
        x, y = T(s.start)
        dr.ellipse([x - 2.6, y - 2.6, x + 2.6, y + 2.6], fill=(16, 16, 16))
    n_l = sum(1 for s in segs if s.kind == "L")
    n_c = len(segs) - n_l
    label(dr, (8, size - 21), f"straight {n_l}   curved {n_c}", FONT,
          fill=(20, 20, 20), bg=(255, 255, 255))
    dr.rectangle([6, 6, 14, 14], fill=(206, 58, 34))
    label(dr, (19, 4), "line", FONT_SMALL, fill=(206, 58, 34))
    dr.rectangle([60, 6, 68, 14], fill=(26, 96, 176))
    label(dr, (73, 4), "cubic", FONT_SMALL, fill=(26, 96, 176))
    return img


def sheet(panels: list[tuple[str, Image.Image]], title: str, cols: int) -> Image.Image:
    rows = math.ceil(len(panels) / cols)
    W = cols * PANEL + (cols + 1) * GUTTER
    H = 36 + rows * (PANEL + LABEL_H + GUTTER) + GUTTER
    img = Image.new("RGB", (W, H), (247, 248, 249))
    dr = ImageDraw.Draw(img)
    label(dr, (GUTTER, 10), title, FONT_BIG)
    for i, (cap, p) in enumerate(panels):
        r, c = divmod(i, cols)
        x = GUTTER + c * (PANEL + GUTTER)
        y = 36 + r * (PANEL + LABEL_H + GUTTER)
        label(dr, (x, y), cap, FONT, fill=(70, 78, 84))
        img.paste(p.resize((PANEL, PANEL), Image.NEAREST if p.width < PANEL else Image.LANCZOS),
                  (x, y + LABEL_H))
        dr.rectangle([x, y + LABEL_H, x + PANEL, y + LABEL_H + PANEL], outline=(214, 220, 224))
    return img


# --------------------------------------------------------------------------- driver

def de00_map(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    from skimage.color import deltaE_ciede2000, rgb2lab
    d = deltaE_ciede2000(rgb2lab(np.clip(a, 0, 1)), rgb2lab(np.clip(b, 0, 1)))
    return np.nan_to_num(d, nan=0.0, posinf=0.0, neginf=0.0)


def worst_boxes(de: np.ndarray, k: int, box: int, n: int = 2):
    """Where is the error concentrated? Returns top-n non-overlapping boxes."""
    size = de.shape[0]
    g = de.reshape(size // k, k, size // k, k).sum(axis=(1, 3)).copy()
    out = []
    for _ in range(n):
        gy, gx = np.unravel_index(int(g.argmax()), g.shape)
        if g[gy, gx] <= 0:
            break
        cy, cx = gy * k + k // 2, gx * k + k // 2
        y0 = int(np.clip(cy - box // 2, 0, size - box))
        x0 = int(np.clip(cx - box // 2, 0, size - box))
        out.append((x0, y0))
        r = max(1, box // (2 * k))
        g[max(0, gy - r):gy + r + 1, max(0, gx - r):gx + r + 1] = 0
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--set", default="dev", help="dev | held | all (from devset.json)")
    ap.add_argument("--stems", nargs="*", help="explicit icon stems instead of a set")
    ap.add_argument("--out", default="output/audit")
    ap.add_argument("--exe", default=None)
    ap.add_argument("--size", type=int, default=1024, help="render size for scoring")
    ap.add_argument("--zoom", type=int, default=4, help="close-up magnification")
    ap.add_argument("--limit", type=int, default=0, help="stop after N icons")
    ap.add_argument("--devset", default=None, help="path to devset.json")
    ap.add_argument("rest", nargs="*", help="args after -- go to inkvec")
    a = ap.parse_args()

    exe = Path(a.exe) if a.exe else ROOT / "target" / "release" / "inkvec.exe"
    if not exe.exists():
        for cand in sorted(ROOT.glob("target*/release/inkvec.exe")):
            exe = cand
            break
    extra = [x for x in a.rest if x != "--"] or ["--precision", "0.6"]
    out = ROOT / a.out
    out.mkdir(parents=True, exist_ok=True)

    items = []
    if a.stems:
        for stem in a.stems:
            for corpus in ("twemoji", "noto-emoji", "synthetic"):
                p = ROOT / "bench/data/corpus_raster" / corpus / "128" / f"{stem}.png"
                if p.exists():
                    items.append({"corpus": corpus, "stem": stem})
                    break
    else:
        dsp = Path(a.devset) if a.devset else ROOT / "bench" / "devset.json"
        if not dsp.exists():
            print(f"no devset at {dsp}; pass --stems or --devset")
            return 2
        sets = json.loads(dsp.read_text(encoding="utf-8"))
        items = sets["dev"] + sets["held"] if a.set == "all" else sets[a.set]
    if a.limit:
        items = items[: a.limit]

    rows = []
    for it in items:
        stem, corpus = it["stem"], it["corpus"]
        png = ROOT / "bench/data/corpus_raster" / corpus / "128" / f"{stem}.png"
        gt = ROOT / "bench/data/corpus_svg" / corpus / f"{stem}.svg"
        if not png.exists() or not gt.exists():
            continue
        tmp = out / f"_{stem}.svg"
        r = subprocess.run([str(exe), str(png), "-o", str(tmp), "--quiet"] + extra,
                           capture_output=True, timeout=900)
        if r.returncode != 0:
            print(f"  FAILED {stem}")
            continue
        our_svg = tmp.read_text(encoding="utf-8")
        gt_svg = gt.read_text(encoding="utf-8")

        gt_rgba = np.asarray(render.render(gt_svg, a.size, a.size), np.float32)
        our_rgba = np.asarray(render.render(our_svg, a.size, a.size), np.float32)
        gt_rgb = render.composite(gt_rgba)
        our_rgb = render.composite(our_rgba)
        de = de00_map(gt_rgb, our_rgb)

        segs = svg_segments(our_svg)
        n_l = sum(1 for s in segs if s.kind == "L")
        lens = [math.dist(s.start, s.end) for s in segs if s.kind == "L"]
        rec = {
            "stem": stem,
            "corpus": corpus,
            "dists": float(raster.compare(gt_rgb, our_rgb)["dists"]),
            "de00_mean": float(de.mean()),
            "wrong_area": float((de > JND).mean()),
            "params": svgmodel.parse(our_svg).n_params,
            "gt_params": svgmodel.parse(gt_svg).n_params,
            "lines": n_l,
            "cubics": len(segs) - n_l,
            "pct_curved": round(100 * (len(segs) - n_l) / max(1, len(segs))),
            "median_line_px": round(float(np.median(lens)), 2) if lens else 0.0,
        }

        src = Image.open(png).convert("RGBA").resize((a.size, a.size), Image.NEAREST)
        panels = [
            (f"source 128px (nearest {a.size // 128}x)", over_checker(np.asarray(src, np.float32) / 255)),
            ("ground truth", over_checker(gt_rgba)),
            ("ours", over_checker(our_rgba)),
            (f"error, red is over {JND:.1f} dE00", heatmap(de)),
            ("our segments: red straight, blue curved", segment_map(our_svg, a.size, to_img(our_rgb))),
            (f"{rec['pct_curved']}% curved  |  gt params {rec['gt_params']}  ours {rec['params']}",
             segment_map(gt_svg, a.size, to_img(gt_rgb))),
        ]
        title = (f"{stem}   DISTS {rec['dists']:.4f}   wrong area {rec['wrong_area'] * 100:.2f}%   "
                 f"lines {rec['lines']} (median {rec['median_line_px']}px)  cubics {rec['cubics']}")
        sh = sheet(panels, title, cols=3)
        sheet_path = out / f"{stem}.sheet.png"
        sh.save(sheet_path)

        box = a.size // a.zoom // 2
        crops = []
        for i, (x0, y0) in enumerate(worst_boxes(de, k=max(8, a.size // 64), box=box, n=2)):
            sub = []
            for cap, im in (("ground truth", over_checker(gt_rgba)), ("ours", over_checker(our_rgba)),
                            ("error", heatmap(de)),
                            ("our segments", segment_map(our_svg, a.size, to_img(our_rgb)))):
                sub.append((cap, im.crop((x0, y0, x0 + box, y0 + box))))
            cp = out / f"{stem}.worst{i + 1}.png"
            sheet(sub, f"{stem} — worst region {i + 1} at ({x0}, {y0}), {a.zoom}x", cols=4).save(cp)
            crops.append(cp.name)
        rec["sheet"] = sheet_path.name
        rec["crops"] = crops
        rows.append(rec)
        print(f"  {stem:<34} DISTS {rec['dists']:.4f}  curved {rec['pct_curved']:>3}%  "
              f"wrong {rec['wrong_area'] * 100:5.2f}%", flush=True)
        tmp.unlink(missing_ok=True)

    rows.sort(key=lambda r: -r["dists"])
    (out / "audit.json").write_text(json.dumps(rows, indent=1), encoding="utf-8")

    lines = [
        "# Visual audit",
        "",
        f"`{len(rows)}` icons, traced at 128px and rendered at {a.size}px against the "
        "ground-truth SVG. Worst first.",
        "",
        "Each sheet has six panels: the source, the truth, our output, an error map, and "
        "the **segment maps** for ours and for the truth, which draw straight segments red "
        "and curved ones blue with a dot at every join. Faceting is invisible in a rendered "
        "shape and obvious there.",
        "",
        "| icon | DISTS | wrong area | curved | median line | params (ours/gt) | files |",
        "| --- | ---: | ---: | ---: | ---: | ---: | --- |",
    ]
    for r in rows:
        files = " ".join([f"`{r['sheet']}`"] + [f"`{c}`" for c in r["crops"]])
        lines.append(
            f"| {r['stem']} | {r['dists']:.4f} | {r['wrong_area'] * 100:.2f}% | "
            f"{r['pct_curved']}% | {r['median_line_px']}px | {r['params']}/{r['gt_params']} | {files} |")
    lines += [
        "",
        "## What to look for",
        "",
        "- **Faceting**: a run of red segments along a boundary that the truth draws blue. "
        "A median line length near 2px is the signature.",
        "- **Corner rounding**: a corner the truth rounds and we meet as two straights.",
        "- **Opaque transparency**: colour in the checkerboard where the truth shows through.",
        "- **Colour drift**: broad warm areas in the error map away from any edge.",
        "- **Lost features**: something present in the truth panel and missing from ours.",
    ]
    (out / "INDEX.md").write_text("\n".join(lines), encoding="utf-8")
    print(f"\n  wrote {len(rows)} sheets to {out}")
    print(f"  index: {out / 'INDEX.md'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
