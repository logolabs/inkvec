"""Draw the per-case distributions for the engine comparison: colour error and economy.

Reads the per-item JSON produced by the 21-case harness (one row per case x engine) and
writes two assets:

  docs/assets/engine-distribution.png         dE00 (fidelity)
  docs/assets/engine-distribution-coords.png  coordinates emitted (economy)

Two charts rather than one with a second axis: dE00 and coordinate count differ in scale
and meaning, and a dual-scale plot invents a correlation that is not in the data.

One panel per chart with all five engines overlaid on a single shared axis, so the
comparison is direct and no series is drawn as a faded secondary. The LogoLabs palette
carries exactly two categorical hues that clear the checks on charcoal -- copper and mint
(validated: lightness band and chroma floor pass, normal-vision dE00 21.2, CVD 6.7 in the
6-8 band). Colour therefore does the emphasis (copper = Inkvec) and the line style carries
the rest, with the legend and the table doing the naming. Every warm hue in the brand
family (gold, amber, red) sits within dE00 2.5-5.5 of copper under protan/deutan, so they
cannot be added as further series.

Metrics are computed exactly as bench/crosscompare_competitors.py computes them. House
style comes from tools/make_github_readme_assets.py so this sits beside the other README
visuals.
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

from PIL import ImageDraw

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
sys.path.insert(0, str(ROOT / "bench"))

import make_github_readme_assets as house  # noqa: E402

DATA = ROOT / "bench" / "engine_per_item.json"
W, H = 1600, 720
GRID = (42, 39, 36)
INK, MUTED, COPPER = house.CREAM, house.MUTED, house.COPPER
MINT = (42, 168, 122)

# The subject keeps its own hue, solid and heavier. The field shares the second hue and is
# told apart by dash pattern -- colour follows the entity, style carries the variant.
SERIES = [
    ("inkvec", "Inkvec", COPPER, None, 3),
    ("trazor-auto", "Trazor (auto)", MINT, None, 2),
    ("vtracer-default", "VTracer 0.6.15 default", MINT, (8, 5), 2),
    ("vtracer-1.0-default", "VTracer 1.0 default  (coincides with 0.6.15)", MINT, (2, 4), 2),
    ("vtracer-1.0-simplify", "VTracer 1.0 --simplify 1.5", MINT, (11, 4, 2, 4), 2),
]

METRICS = {
    "de00": dict(
        field="de00", asset="engine-distribution.png", x0=0.008, x1=6.0,
        ticks=[0.01, 0.05, 0.1, 0.5, 1.0], fmt="{:.3f}", axis_fmt="{:g}",
        title="Per-case colour error by engine",
        subtitle="21 cases, each traced from the identical 512 px raster · "
                 "CIEDE2000 (dE00) at 1024 px · one dot per case · x is log scale",
        footer="cumulative fraction of cases (0 → 1) against dE00, log scale · "
               "data: bench/engine_per_item.json",
    ),
    "coordinates": dict(
        field="coordinates", asset="engine-distribution-coords.png", x0=40.0, x1=8000.0,
        ticks=[50, 100, 500, 1000, 5000], fmt="{:,.0f}", axis_fmt="{:,.0f}",
        title="Per-case coordinate count by engine",
        subtitle="same 21 cases · geometry parameters emitted · a primitive counts as 2, "
                 "a cubic as 6 · one dot per case · x is log scale",
        footer="cumulative fraction of cases (0 → 1) against coordinates emitted, log "
               "scale · data: bench/engine_per_item.json",
    ),
}

PX0, PY0, PX1, PY1 = 120, 250, 1060, 620
LX = 1110           # legend + table column


def xs(v: float, x0: float, x1: float) -> float:
    lo, hi = math.log10(x0), math.log10(x1)
    return PX0 + (math.log10(max(v, x0)) - lo) / (hi - lo) * (PX1 - PX0)


def ys(i: int, n: int) -> float:
    return PY1 - (PY1 - PY0) * (i / n)


def ecdf(vals: list[float], x0: float, x1: float) -> list[tuple[float, float]]:
    v = sorted(vals)
    n = len(v)
    pts = [(xs(x0, x0, x1), ys(0, n))]
    for i, val in enumerate(v, start=1):
        pts.append((xs(val, x0, x1), ys(i - 1, n)))
        pts.append((xs(val, x0, x1), ys(i, n)))
    pts.append((xs(x1, x0, x1), ys(n, n)))
    return pts


def draw_line(d, pts, colour, width, dash=None):
    """PIL has no dash support, so walk the polyline by arc length emitting on/off runs.

    `dash` is a repeating cycle of run lengths, alternating on/off -- so (8, 5) is a
    dashed line and (11, 4, 2, 4) is dash-dot.
    """
    if dash is None:
        d.line(pts, fill=colour, width=width, joint="curve")
        return
    cyc = list(dash)
    i, run_left, emitting = 0, float(cyc[0]), True
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        seg = math.hypot(x1 - x0, y1 - y0)
        if seg <= 1e-9:
            continue
        pos = 0.0
        while pos < seg:
            take = min(run_left, seg - pos)
            if emitting:
                t0, t1 = pos / seg, (pos + take) / seg
                d.line([(x0 + (x1 - x0) * t0, y0 + (y1 - y0) * t0),
                        (x0 + (x1 - x0) * t1, y0 + (y1 - y0) * t1)],
                       fill=colour, width=width)
            pos += take
            run_left -= take
            if run_left <= 1e-9:
                i = (i + 1) % len(cyc)
                emitting = (i % 2 == 0)
                run_left = float(cyc[i])


def draw(metric_key: str, data: list[dict]) -> None:
    m = METRICS[metric_key]
    field, x0, x1, ticks, fmt = m["field"], m["x0"], m["x1"], m["ticks"], m["fmt"]

    by_engine: dict[str, list[float]] = {}
    for row in data:
        if "error" in row or row.get(field) is None:
            continue
        by_engine.setdefault(row["engine"], []).append(float(row[field]))

    img = house.canvas(W, H)
    d = ImageDraw.Draw(img)
    tick = house.sans(13)

    house.centered_brand(img, 30, 250)
    d.text((120, 100), m["title"], font=house.font(40), fill=INK)
    d.text((120, 154), m["subtitle"], font=house.sans(17), fill=MUTED)

    # Plot frame, solid hairline grid, labelled ticks.
    d.rectangle([PX0, PY0, PX1, PY1], outline=GRID, width=1)
    for frac in (0.0, 0.25, 0.5, 0.75, 1.0):
        gy = ys(int(frac * 21), 21)
        d.line([(PX0, gy), (PX1, gy)], fill=GRID, width=1)
        d.text((PX0 - 36, gy - 8), f"{frac:.2f}", font=tick, fill=MUTED)
    for gv in ticks:
        gx = xs(gv, x0, x1)
        if gx >= PX1:
            continue
        d.line([(gx, PY0), (gx, PY1)], fill=GRID, width=1)
        lab = m["axis_fmt"].format(gv)
        d.text((gx - d.textlength(lab, font=tick) / 2, PY1 + 14), lab, font=tick, fill=MUTED)
    d.text((PX0, PY1 + 44), "dE00 (log)" if metric_key == "de00" else "coordinates (log)",
           font=house.sans(14), fill=MUTED)
    d.text((PX0 - 96, PY0 - 26), "fraction of cases", font=house.sans(14), fill=MUTED)

    # Curves. Every engine on the same axis; nothing drawn as a faded secondary.
    order = [k for k, *_ in SERIES if k != "inkvec"] + ["inkvec"]  # Inkvec last, on top
    for key in order:
        entry = next(s for s in SERIES if s[0] == key)
        _, label, colour, dash, width = entry
        vals = by_engine.get(key, [])
        if not vals:
            continue
        pts = ecdf(vals, x0, x1)
        draw_line(d, pts, colour, width, dash)
        if key == "inkvec":
            for i, v in enumerate(sorted(vals), start=1):
                cx, cy = xs(v, x0, x1), ys(i, len(vals))
                d.ellipse([cx - 4.5, cy - 4.5, cx + 4.5, cy + 4.5], fill=house.BG)
                d.ellipse([cx - 3, cy - 3, cx + 3, cy + 3], fill=colour)

    ink = sorted(by_engine.get("inkvec", []))
    if ink:
        med = ink[len(ink) // 2]
        mx = xs(med, x0, x1)
        d.line([(mx, PY1), (mx, PY1 + 6)], fill=COPPER, width=2)
        d.text((mx + 7, PY0 + 10), f"Inkvec median {fmt.format(med)}",
               font=tick, fill=COPPER)

    # Legend + table view, one row per engine: name, then the numbers.
    # Legend + table view, one row per engine: swatch and name, then the numbers beneath.
    cols = (52, 168, 268)
    for cx, head in zip(cols, ("median", "mean", "max")):
        d.text((LX + cx, PY0 - 26), head, font=house.sans(14), fill=MUTED)
    for r, (key, label, colour, dash, width) in enumerate(SERIES):
        ry = PY0 + 10 + r * 36
        draw_line(d, [(LX, ry + 8), (LX + 42, ry + 8)], colour, width, dash)
        d.text((LX + 54, ry), label, font=house.sans(15), fill=INK)
        vals = sorted(float(v) for v in by_engine.get(key, []))
        if not vals:
            continue
        med = vals[len(vals) // 2]
        for cx, val in zip(cols, (fmt.format(med), fmt.format(sum(vals) / len(vals)),
                                  fmt.format(max(vals)))):
            d.text((LX + cx, ry + 19), val, font=house.sans(15), fill=MUTED)

    d.text((120, 690), m["footer"], font=tick, fill=MUTED)
    out = ROOT / "docs" / "assets" / m["asset"]
    img.save(out)
    print(f"wrote {out}")


def main() -> int:
    data = json.loads(DATA.read_text(encoding="utf-8"))
    for key in METRICS:
        draw(key, data)
    return 0


if __name__ == "__main__":
    sys.exit(main())
