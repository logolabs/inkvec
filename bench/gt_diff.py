"""Diff a trace against the ground-truth SVG at the level the artist worked at: regions.

Scores say how far off we are; the visual audit says where. Neither says *what went
wrong*. This does, by treating both documents as painter's-order region maps and
comparing the maps:

* each ground-truth path is rendered with a unique flat id colour so every output pixel
  knows which GT element owns it; the same for our output;
* the two label maps are cross-tabulated, and every GT region is classed as **matched**
  (one of ours covers it and little else), **absorbed** (it ended up inside a region that
  is mostly something else — a nostril swallowed by the muzzle), **shattered** (it came
  back as several of ours — a ramp quantised into bands), or **missing** (it went to the
  backdrop);
* boundaries of the two maps are distance-transformed against each other, giving the
  displacement of our contours from the truth in source pixels;
* the per-pixel dE00 is then attributed to the reason: **boundary** (within a source
  pixel of either contour), **structure** (interior of an absorbed / shattered / missing
  region), **gradient** (interior of a matched region the artist filled with a gradient) or
  **flat** (interior of a matched flat region); and for matched pairs the fill kinds and
  stop counts are compared.

Summed over a set, the attribution says which disease carries the error, and the region
table says which icons and which regions to open first.

Usage:
    python bench/gt_diff.py --set dev --exe path/to/inkvec.exe --out output/gtdiff
    python bench/gt_diff.py --stems emoji_u1f98e --intake 512 --svg-dir output/vs512_stops12/_tmp_svgs
    python bench/gt_diff.py --gt a.svg --svg b.svg --out output/gtdiff

Writes one overlay PNG and one JSON per icon, INDEX.md (worst first) and summary.json.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import time
import xml.etree.ElementTree as ET
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont
from scipy import ndimage

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "bench"))
from inkvec_bench import render  # noqa: E402

SVG_NS = "http://www.w3.org/2000/svg"
XLINK_NS = "http://www.w3.org/1999/xlink"
ET.register_namespace("", SVG_NS)
ET.register_namespace("xlink", XLINK_NS)

DRAWABLE = {"path", "circle", "ellipse", "rect", "polygon", "polyline", "line", "use"}
CONTAINER_SKIP = {"defs", "clipPath", "mask", "symbol", "pattern", "marker",
                  "linearGradient", "radialGradient", "style", "title", "desc", "metadata"}
OPACITY_ATTRS = ("opacity", "fill-opacity", "stroke-opacity", "filter", "mix-blend-mode")

CATEGORIES = ("boundary", "structure", "gradient", "flat")
CLASSES = ("matched", "absorbed", "shattered", "missing")


def _local(tag: str) -> str:
    return tag.split("}")[-1]


# ------------------------------------------------------------------ document → labels

@dataclass
class Region:
    label: int
    tag: str
    element_id: str | None
    fill: str            # "flat" | "gradient" | "none"
    stops: int           # gradient stops (0 for flat)
    opacity: float       # effective opacity written in the source (before it was stripped)
    stroked: bool


def _style_dict(s: str | None) -> dict[str, str]:
    out = {}
    for part in (s or "").split(";"):
        if ":" in part:
            k, v = part.split(":", 1)
            out[k.strip()] = v.strip()
    return out


def _paint(el, name: str) -> str | None:
    v = el.get(name)
    if v is None:
        v = _style_dict(el.get("style")).get(name)
    return v


def _label_colour(k: int) -> tuple[int, int, int]:
    # A well-spread 24-bit hash: an anti-aliased mix of two ids landing exactly on a third
    # is then as unlikely as any other 24-bit collision.
    c = (k * 0x9E3779B1 + 0x7F4A7C15) & 0xFFFFFF
    return (c >> 16) & 255, (c >> 8) & 255, c & 255


def labelled_document(svg: str) -> tuple[str, list[Region]]:
    """Rewrite `svg` so every drawable element paints one flat id colour, fully opaque.

    Gradient fills become flat, opacity and filters are stripped at every level (groups
    too), strokes take their element's colour. Returns the rewritten document and the
    region table; `Region.label` k is painted with `_label_colour(k)`.
    """
    root = ET.fromstring(svg)
    grads: dict[str, int] = {}
    for g in root.iter():
        if _local(g.tag) in ("linearGradient", "radialGradient") and g.get("id"):
            grads[g.get("id")] = sum(1 for c in g if _local(c.tag) == "stop")
    # Inherited stop lists via href.
    for g in root.iter():
        if _local(g.tag) in ("linearGradient", "radialGradient") and g.get("id") and grads[g.get("id")] == 0:
            ref = g.get("href") or g.get(f"{{{XLINK_NS}}}href") or ""
            grads[g.get("id")] = grads.get(ref.lstrip("#"), 0)

    regions: list[Region] = []

    def walk(el, opacity: float, inherited_fill: str | None, in_skip: bool):
        name = _local(el.tag)
        skip = in_skip or name in CONTAINER_SKIP
        op = opacity
        for a in ("opacity", "fill-opacity"):
            v = _paint(el, a)
            if v is not None:
                try:
                    op *= float(v.rstrip("%")) / (100.0 if v.endswith("%") else 1.0)
                except ValueError:
                    pass
        fill_raw = _paint(el, "fill")
        fill_here = fill_raw if fill_raw is not None else inherited_fill
        # Strip transparency and effects everywhere so the id colour arrives intact.
        for a in OPACITY_ATTRS:
            if a in el.attrib:
                del el.attrib[a]
        st = _style_dict(el.get("style"))
        if st:
            for a in OPACITY_ATTRS + ("fill", "stroke"):
                st.pop(a, None)
            el.set("style", ";".join(f"{k}:{v}" for k, v in st.items()))
        if name in DRAWABLE and not skip:
            k = len(regions) + 1
            r, g, b = _label_colour(k)
            hexc = f"#{r:02x}{g:02x}{b:02x}"
            fr = (fill_here or "black").strip()
            stroke = _paint(el, "stroke")
            stroked = stroke is not None and stroke.strip().lower() != "none"
            if fr.lower().startswith("url("):
                ref = re.sub(r"^url\(\s*#?|\s*\)$", "", fr).strip("'\"")
                fill, stops = "gradient", grads.get(ref, 0)
            elif fr.lower() in ("none", "transparent"):
                fill, stops = "none", 0
            else:
                fill, stops = "flat", 0
            el.set("fill", "none" if fill == "none" else hexc)
            if stroked:
                el.set("stroke", hexc)
            regions.append(Region(k, name, el.get("id"), fill, stops, op, stroked))
        elif fill_raw is not None and not skip:
            # A group's own fill is only a default for its children; leave it in place.
            pass
        for c in list(el):
            walk(c, op, fill_here, skip)

    walk(root, 1.0, None, False)
    # No anti-aliasing: every pixel then decodes to exactly one id.
    root.set("shape-rendering", "crispEdges")
    return ET.tostring(root, encoding="unicode"), regions


def label_map(svg: str, regions: list[Region], size: int) -> tuple[np.ndarray, float]:
    """Render the labelled document and decode it to an int map (0 = backdrop).

    Anti-aliased pixels decode to nothing and take the label of the nearest decoded
    pixel. Returns the map and the fraction of covered pixels that needed that.
    """
    rgba = render.render(svg, size, size)
    a = rgba[..., 3] >= 0.5
    rgb = np.clip(np.rint(rgba[..., :3] * 255), 0, 255).astype(np.int64)
    key = (rgb[..., 0] << 16) | (rgb[..., 1] << 8) | rgb[..., 2]
    lut = {}
    for r in regions:
        cr, cg, cb = _label_colour(r.label)
        lut[(cr << 16) | (cg << 8) | cb] = r.label
    flat = np.full(key.size, -1, dtype=np.int64)
    keys = key.ravel()
    uniq, inv = np.unique(keys, return_inverse=True)
    mapped = np.array([lut.get(int(u), -1) for u in uniq], dtype=np.int64)
    flat = mapped[inv]
    lab = flat.reshape(key.shape)
    lab[~a] = 0
    unknown = (lab < 0)
    frac = float(unknown.sum() / max(1, a.sum()))
    if unknown.any():
        _, idx = ndimage.distance_transform_edt(unknown, return_indices=True)
        lab = lab[idx[0], idx[1]]
    return lab, frac


# ------------------------------------------------------------------ comparison

def boundaries(lab: np.ndarray) -> np.ndarray:
    b = np.zeros(lab.shape, dtype=bool)
    b[:, :-1] |= lab[:, :-1] != lab[:, 1:]
    b[:-1, :] |= lab[:-1, :] != lab[1:, :]
    return b


def de00_map(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    from skimage.color import deltaE_ciede2000, rgb2lab
    d = deltaE_ciede2000(rgb2lab(np.clip(a, 0, 1)), rgb2lab(np.clip(b, 0, 1)))
    return np.nan_to_num(d, nan=0.0, posinf=0.0, neginf=0.0)


@dataclass
class Diff:
    stem: str
    size: int
    intake: int
    n_gt: int
    n_ours: int
    unknown_gt: float
    unknown_ours: float
    classes: dict = field(default_factory=dict)        # class -> count of GT regions
    class_area: dict = field(default_factory=dict)     # class -> GT area share
    attribution: dict = field(default_factory=dict)    # category -> share of summed dE
    category_area: dict = field(default_factory=dict)  # category -> pixel share
    category_mean: dict = field(default_factory=dict)  # category -> mean dE
    de_mean: float = 0.0
    disp_ours_to_gt: dict = field(default_factory=dict)  # mean / p95 / frac_gt_1px (source px)
    disp_gt_to_ours: dict = field(default_factory=dict)
    fills: dict = field(default_factory=dict)          # "gt->ours" kind pairs on matched regions
    stops: dict = field(default_factory=dict)          # gt stops vs ours on matched gradients
    regions: list = field(default_factory=list)        # worst GT regions by summed dE


def compare(stem: str, gt_svg: str, our_svg: str, size: int, intake: int,
            min_area_src: float = 4.0) -> tuple[Diff, dict]:
    g_doc, g_regs = labelled_document(gt_svg)
    o_doc, o_regs = labelled_document(our_svg)
    G, ug = label_map(g_doc, g_regs, size)
    O, uo = label_map(o_doc, o_regs, size)
    ref = render.composite(render.render(gt_svg, size, size))
    out = render.composite(render.render(our_svg, size, size))
    de = de00_map(ref, out)

    ng, no = len(g_regs) + 1, len(o_regs) + 1
    M = np.bincount((G * no + O).ravel(), minlength=ng * no).reshape(ng, no).astype(np.float64)
    ag, ao = M.sum(1), M.sum(0)
    scale = size / intake                      # output px per source px
    min_area = min_area_src * scale * scale

    # --- class every GT region
    cls = np.zeros(ng, dtype=np.int64)         # index into CLASSES; 0 = matched
    match = np.full(ng, -1, dtype=np.int64)
    for k in range(1, ng):
        if ag[k] == 0:
            continue
        j = int(np.argmax(M[k]))
        cover = M[k, j] / ag[k]
        if j == 0 or cover < 0.3:
            # Went to the backdrop, or no single region of ours holds even a third of it.
            pieces = int(((M[k, 1:] / ag[k]) >= 0.1).sum())
            cls[k] = CLASSES.index("shattered") if pieces >= 2 else CLASSES.index("missing")
            continue
        match[k] = j
        share_of_ours = M[k, j] / ao[j]
        if cover >= 0.6 and share_of_ours >= 0.5:
            cls[k] = 0
        elif cover >= 0.6:
            cls[k] = CLASSES.index("absorbed")
        else:
            pieces = int(((M[k, 1:] / ag[k]) >= 0.1).sum())
            cls[k] = CLASSES.index("shattered") if pieces >= 2 else CLASSES.index("absorbed")

    sig = np.array([k for k in range(1, ng) if ag[k] >= min_area])
    classes = Counter(CLASSES[int(cls[k])] for k in sig)
    class_area = {c: 0.0 for c in CLASSES}
    tot = float(sum(ag[k] for k in sig)) or 1.0
    for k in sig:
        class_area[CLASSES[int(cls[k])]] += float(ag[k]) / tot

    # --- boundary displacement
    bG, bO = boundaries(G), boundaries(O)
    dG = ndimage.distance_transform_edt(~bG) if bG.any() else np.full(G.shape, np.inf)
    dO = ndimage.distance_transform_edt(~bO) if bO.any() else np.full(O.shape, np.inf)

    def stats(d: np.ndarray) -> dict:
        # No counterpart contour at all (one side has no boundary): every pixel is
        # unmatched, which is a structural fact, not a distance.
        d = d[np.isfinite(d)]
        if d.size == 0:
            return dict(mean=0.0, p95=0.0, frac_gt_1px=0.0, n=0)
        s = d / scale
        return dict(mean=float(s.mean()), p95=float(np.percentile(s, 95)),
                    frac_gt_1px=float((s > 1.0).mean()), n=int(s.size))

    # A contour of a GT region we never matched has no counterpart of ours to be displaced
    # from: its error is structural, so only matched contours count as boundary.
    bGm = bG & (cls[G] == 0)
    dGm = ndimage.distance_transform_edt(~bGm) if bGm.any() else np.full(G.shape, np.inf)
    d_o2g = stats(dG[bO])
    d_g2o = stats(dO[bGm])

    # --- attribute dE
    band = (dGm <= scale) | (dO <= scale)
    structural = np.isin(G, [k for k in range(1, ng) if cls[k] != 0]) & ~band
    grad_lab = [r.label for r in g_regs if r.fill == "gradient"]
    gradient = np.isin(G, grad_lab) & ~band & ~structural
    flat = ~band & ~structural & ~gradient
    total = float(de.sum()) or 1.0
    attribution, category_area, category_mean = {}, {}, {}
    for name, m in zip(CATEGORIES, (band, structural, gradient, flat)):
        attribution[name] = float(de[m].sum() / total)
        category_area[name] = float(m.mean())
        category_mean[name] = float(de[m].mean()) if m.any() else 0.0

    # --- fill kinds on matched regions
    fills, stops = Counter(), Counter()
    for k in sig:
        if cls[k] != 0:
            continue
        gr, orr = g_regs[k - 1], o_regs[match[k] - 1]
        fills[f"{gr.fill}->{orr.fill}"] += 1
        if gr.fill == "gradient" and orr.fill == "gradient":
            stops[f"{gr.stops}->{orr.stops}"] += 1

    # --- worst regions
    de_by_region = ndimage.sum(de, G, index=np.arange(ng))
    rows = []
    for k in sig:
        gr = g_regs[k - 1]
        rows.append(dict(label=int(k), id=gr.element_id, tag=gr.tag, fill=gr.fill, stops=gr.stops,
                         opacity=round(gr.opacity, 3), stroked=gr.stroked,
                         area_src=float(ag[k] / (scale * scale)),
                         cls=CLASSES[int(cls[k])], match=int(match[k]),
                         ours_fill=(o_regs[match[k] - 1].fill if match[k] > 0 else None),
                         ours_stops=(o_regs[match[k] - 1].stops if match[k] > 0 else None),
                         de_sum=float(de_by_region[k]), de_share=float(de_by_region[k] / total),
                         de_mean=float(de_by_region[k] / ag[k])))
    rows.sort(key=lambda r: -r["de_sum"])

    diff = Diff(stem, size, intake, len(sig), int((ao[1:] >= min_area).sum()), ug, uo,
                dict(classes), class_area, attribution, category_area, category_mean,
                float(de.mean()), d_o2g, d_g2o, dict(fills), dict(stops), rows[:12])
    maps = dict(G=G, O=O, cls=cls, bG=bG, bO=bO, dG=dG, dO=dO, de=de, ref=ref, out=out,
                band=band, scale=scale)
    return diff, maps


# ------------------------------------------------------------------ picture

def _font(size: int):
    for name in ("consola.ttf", "DejaVuSansMono.ttf", "arial.ttf"):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            continue
    return ImageFont.load_default()


TINT = {"absorbed": (225, 40, 40), "shattered": (240, 150, 0), "missing": (150, 0, 200)}


def overlay(diff: Diff, m: dict, panel: int = 512) -> Image.Image:
    size = m["G"].shape[0]
    base = (m["ref"] * 0.35 + 0.65).clip(0, 1)               # faded truth
    img = (base * 255).astype(np.uint8).copy()
    cls_map = m["cls"][m["G"]]
    for name, colour in TINT.items():
        sel = cls_map == CLASSES.index(name)
        img[sel] = (img[sel] * 0.55 + np.array(colour) * 0.45).astype(np.uint8)
    both = m["bG"] & (m["dO"] <= 1.0) | m["bO"] & (m["dG"] <= 1.0)
    img[m["bG"]] = (0, 150, 200)
    img[m["bO"]] = (220, 0, 140)
    img[both] = (40, 40, 40)
    left = Image.fromarray(img).resize((panel, panel), Image.LANCZOS)

    heat = np.clip(m["de"] / 12.0, 0, 1)[..., None]
    # white -> yellow -> red, so the eye reads brightness of hue as amount of error
    h = np.where(heat < 0.5, np.array([1.0, 1.0, 1.0]) * (1 - 2 * heat) + np.array([1.0, 0.85, 0.0]) * 2 * heat,
                 np.array([1.0, 0.85, 0.0]) * (2 - 2 * heat) + np.array([0.8, 0.0, 0.0]) * (2 * heat - 1))
    h = (np.where(heat < 0.02, 1.0, h) * 255).astype(np.uint8)
    right = Image.fromarray(h).resize((panel, panel), Image.LANCZOS)
    ours = Image.fromarray((m["out"] * 255).astype(np.uint8)).resize((panel, panel), Image.LANCZOS)

    strip = 165
    sheet = Image.new("RGB", (panel * 3 + 40, panel + strip + 30), (250, 250, 248))
    sheet.paste(left, (10, 30)); sheet.paste(ours, (panel + 20, 30)); sheet.paste(right, (panel * 2 + 30, 30))
    d = ImageDraw.Draw(sheet)
    f, fs = _font(15), _font(13)
    d.text((10, 8), f"{diff.stem}   truth faded: GT contour cyan, ours magenta, agreeing dark", font=f, fill=(20, 22, 26))
    d.text((panel + 20, 8), "ours", font=f, fill=(20, 22, 26))
    d.text((panel * 2 + 30, 8), "dE00: white 0, yellow 6, red 12+", font=f, fill=(20, 22, 26))
    d.text((10, panel + 32), "tint on the truth: absorbed red, shattered orange, missing violet", font=fs, fill=(90, 92, 96))
    y = panel + 52
    lines = [
        f"GT regions {diff.n_gt}  ours {diff.n_ours}   classes: " +
        "  ".join(f"{c} {diff.classes.get(c, 0)}" for c in CLASSES) +
        f"   (area share absorbed {diff.class_area['absorbed']:.1%}, shattered {diff.class_area['shattered']:.1%}, missing {diff.class_area['missing']:.1%})",
        f"dE00 mean {diff.de_mean:.3f}   share of error: " +
        "  ".join(f"{c} {diff.attribution[c]:.0%}" for c in CATEGORIES) +
        "   (mean dE " + "  ".join(f"{c} {diff.category_mean[c]:.2f}" for c in CATEGORIES) + ")",
        f"contour displacement, source px: ours->GT mean {diff.disp_ours_to_gt['mean']:.2f} p95 {diff.disp_ours_to_gt['p95']:.2f} "
        f">1px {diff.disp_ours_to_gt['frac_gt_1px']:.0%}   GT->ours mean {diff.disp_gt_to_ours['mean']:.2f} p95 {diff.disp_gt_to_ours['p95']:.2f} "
        f">1px {diff.disp_gt_to_ours['frac_gt_1px']:.0%}",
        "fills on matched regions: " + "  ".join(f"{k} {v}" for k, v in sorted(diff.fills.items())) +
        ("   stops: " + "  ".join(f"{k} {v}" for k, v in sorted(diff.stops.items())) if diff.stops else ""),
    ]
    for r in diff.regions[:4]:
        lines.append(f"  worst: #{r['label']} {r['id'] or r['tag']} {r['fill']}{('/' + str(r['stops'])) if r['stops'] else ''} "
                     f"{r['area_src']:.0f}px2 {r['cls']} -> ours {r['ours_fill']}{('/' + str(r['ours_stops'])) if r['ours_stops'] else ''}  "
                     f"dE share {r['de_share']:.0%} mean {r['de_mean']:.2f}")
    for ln in lines:
        d.text((10, y), ln, font=fs, fill=(20, 22, 26)); y += 17
    return sheet


# ------------------------------------------------------------------ driver

def aggregate(diffs: list[Diff]) -> dict:
    n = len(diffs)
    if not n:
        return {}
    agg = dict(images=n, de_mean=float(np.mean([d.de_mean for d in diffs])))
    # Error-weighted attribution: share of the summed dE over the whole set.
    w = np.array([d.de_mean for d in diffs])
    agg["attribution"] = {c: float(sum(d.attribution[c] * d.de_mean for d in diffs) / (w.sum() or 1)) for c in CATEGORIES}
    agg["category_mean_de"] = {c: float(np.mean([d.category_mean[c] for d in diffs])) for c in CATEGORIES}
    agg["category_area"] = {c: float(np.mean([d.category_area[c] for d in diffs])) for c in CATEGORIES}
    agg["classes"] = {c: int(sum(d.classes.get(c, 0) for d in diffs)) for c in CLASSES}
    agg["class_area"] = {c: float(np.mean([d.class_area[c] for d in diffs])) for c in CLASSES}
    agg["disp_ours_to_gt_mean"] = float(np.mean([d.disp_ours_to_gt["mean"] for d in diffs]))
    agg["disp_ours_to_gt_frac_gt_1px"] = float(np.mean([d.disp_ours_to_gt["frac_gt_1px"] for d in diffs]))
    agg["disp_gt_to_ours_mean"] = float(np.mean([d.disp_gt_to_ours["mean"] for d in diffs]))
    agg["disp_gt_to_ours_frac_gt_1px"] = float(np.mean([d.disp_gt_to_ours["frac_gt_1px"] for d in diffs]))
    fills, stops = Counter(), Counter()
    for d in diffs:
        fills.update(d.fills); stops.update(d.stops)
    agg["fills"] = dict(fills); agg["stops"] = dict(stops)
    agg["regions_gt"] = int(sum(d.n_gt for d in diffs)); agg["regions_ours"] = int(sum(d.n_ours for d in diffs))
    return agg


def print_summary(agg: dict) -> None:
    if not agg:
        print("nothing compared"); return
    print(f"\n{agg['images']} images, mean dE00 {agg['de_mean']:.3f}; GT regions {agg['regions_gt']}, ours {agg['regions_ours']}")
    print(f"{'error share':>22}  {'mean dE':>8}  {'area':>6}")
    for c in CATEGORIES:
        print(f"{c:>12} {agg['attribution'][c]:>9.1%}  {agg['category_mean_de'][c]:>8.2f}  {agg['category_area'][c]:>6.1%}")
    print("GT regions by class: " + "  ".join(f"{c} {agg['classes'][c]}" for c in CLASSES) +
          "   area share: " + "  ".join(f"{c} {agg['class_area'][c]:.1%}" for c in CLASSES))
    print(f"contour displacement (source px): ours->GT mean {agg['disp_ours_to_gt_mean']:.2f}, >1px {agg['disp_ours_to_gt_frac_gt_1px']:.1%};"
          f"  GT->ours mean {agg['disp_gt_to_ours_mean']:.2f}, >1px {agg['disp_gt_to_ours_frac_gt_1px']:.1%}")
    print("fill kinds on matched regions (gt->ours): " + "  ".join(f"{k} {v}" for k, v in sorted(agg["fills"].items())))
    if agg["stops"]:
        print("gradient stops (gt->ours): " + "  ".join(f"{k} {v}" for k, v in sorted(agg["stops"].items(), key=lambda kv: -kv[1])[:12]))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--set", default="dev", choices=("dev", "held", "all"))
    ap.add_argument("--stems", nargs="*")
    ap.add_argument("--gt"); ap.add_argument("--svg")
    ap.add_argument("--svg-dir", help="reuse traces: <dir>/<stem>.inkvec.svg or <stem>.svg")
    ap.add_argument("--exe", default=str(ROOT / "target" / "release" / "inkvec.exe"))
    ap.add_argument("--intake", type=int, default=128)
    ap.add_argument("--size", type=int, default=1024)
    ap.add_argument("--out", default=str(ROOT / "output" / "gtdiff"))
    ap.add_argument("--min-area", type=float, default=4.0, help="ignore GT regions under this many source px^2")
    ap.add_argument("--no-png", action="store_true")
    ap.add_argument("rest", nargs="*")
    a = ap.parse_args()
    extra = [x for x in a.rest if x != "--"]
    out = Path(a.out); out.mkdir(parents=True, exist_ok=True)

    if a.gt and a.svg:
        items = [dict(stem=Path(a.svg).stem, gt=Path(a.gt), svg=Path(a.svg))]
    else:
        sets = json.loads((ROOT / "bench" / "devset.json").read_text(encoding="utf-8"))
        intake_dir = str(a.intake)
        v2 = ROOT / "bench" / "devset_v2.json"
        if v2.exists():   # the stratified ~1k corpus: dev / held_a / held_b / full
            d2 = json.loads(v2.read_text(encoding="utf-8"))
            sets = {"dev": d2["dev"], "held": d2["held_a"] + d2["held_b"]}
            if "tier" in d2 and str(a.intake) == "128":
                intake_dir = str(d2["tier"])   # e.g. "128ss": the supersampled intake
            seen = {(x["corpus"], x["stem"]) for x in sets["dev"] + sets["held"]}
            sets["held"] += [x for x in d2["full"] if (x["corpus"], x["stem"]) not in seen]
        chosen = sets["dev"] + sets["held"] if a.set == "all" else sets[a.set]
        if a.stems:
            chosen = [it for it in sets["dev"] + sets["held"] if it["stem"] in set(a.stems)]
        items = []
        for it in chosen:
            gt = ROOT / "bench" / "data" / "corpus_svg" / it["corpus"] / f"{it['stem']}.svg"
            if gt.exists():
                items.append(dict(stem=it["stem"], corpus=it["corpus"], gt=gt, svg=None))

    diffs = []
    for it in items:
        gt_svg = it["gt"].read_text(encoding="utf-8")
        if it.get("svg"):
            our_svg = it["svg"].read_text(encoding="utf-8")
        elif a.svg_dir:
            p = Path(a.svg_dir) / f"{it['stem']}.inkvec.svg"
            if not p.exists():
                p = Path(a.svg_dir) / f"{it['stem']}.svg"
            if not p.exists():
                print(f"  no trace for {it['stem']}"); continue
            our_svg = p.read_text(encoding="utf-8")
        else:
            png = ROOT / "bench" / "data" / "corpus_raster" / it["corpus"] / intake_dir / f"{it['stem']}.png"
            if not png.exists():
                png = out / "_intake" / f"{it['stem']}_{a.intake}.png"
                png.parent.mkdir(exist_ok=True)
                render.save_rgba(render.render(gt_svg, a.intake, a.intake), png)
            tmp = out / "_trace.svg"
            r = subprocess.run([a.exe, str(png), "-o", str(tmp), "--quiet"] + extra, capture_output=True, timeout=900)
            if r.returncode != 0:
                print(f"  FAILED {it['stem']}: {r.stderr.decode('utf8', 'replace')[:200]}"); continue
            our_svg = tmp.read_text(encoding="utf-8")
            (out / f"{it['stem']}.svg").write_text(our_svg, encoding="utf-8")
        t0 = time.time()
        try:
            diff, maps = compare(it["stem"], gt_svg, our_svg, a.size, a.intake, a.min_area)
        except Exception as e:  # noqa: BLE001
            print(f"  FAILED-DIFF {it['stem']}: {type(e).__name__}: {e}"); continue
        diffs.append(diff)
        (out / f"{it['stem']}.json").write_text(json.dumps(diff.__dict__, indent=1), encoding="utf-8")
        if not a.no_png:
            overlay(diff, maps).save(out / f"{it['stem']}.png")
        att = "  ".join(f"{c[:4]} {diff.attribution[c]:.0%}" for c in CATEGORIES)
        cl = "  ".join(f"{c[:4]} {diff.classes.get(c, 0)}" for c in CLASSES)
        print(f"{it['stem']:<44} dE {diff.de_mean:6.3f}  {att}   {cl}   "
              f"disp {diff.disp_ours_to_gt['mean']:.2f}px  unk {diff.unknown_gt:.1%}/{diff.unknown_ours:.1%}  {time.time() - t0:4.1f}s")

    agg = aggregate(diffs)
    print_summary(agg)
    (out / "summary.json").write_text(json.dumps(dict(aggregate=agg, images=[d.__dict__ for d in diffs]), indent=1), encoding="utf-8")
    lines = ["# GT diff, worst first", "",
             "| icon | dE00 | boundary | structure | gradient | flat | absorbed | shattered | missing | disp px |", "|---|---|---|---|---|---|---|---|---|---|"]
    for d in sorted(diffs, key=lambda d: -d.de_mean):
        lines.append(f"| [{d.stem}]({d.stem}.png) | {d.de_mean:.3f} | " +
                     " | ".join(f"{d.attribution[c]:.0%}" for c in CATEGORIES) + " | " +
                     " | ".join(str(d.classes.get(c, 0)) for c in CLASSES[1:]) +
                     f" | {d.disp_ours_to_gt['mean']:.2f} |")
    (out / "INDEX.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
