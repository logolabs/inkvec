"""Design statistics of traced files against the artists' own: is the description human?

Chain R (`docs/theory/chain-representation.md`) asks that a trace be written the way the
artist wrote the drawing: the same kinds of pieces, lines along the axes and the diagonals,
numbers on the design grid, values shared. On damaged input (the gate's `web` tier) the
target is still the artist's clean file, so the statistics of a `web` trace should match the
artist's at least as well as those of a clean trace. This script counts, per family and
over a set:

* **pieces**: the share of lines, cubics, quadratics and arcs among the segments, and of
  primitive elements (`rect`, `circle`, `ellipse`) among the elements;
* **angles**: the share of lines at 0/90° and at 45° (exactly as written, and within half a
  degree), and the rest;
* **grid**: the share of on-curve numbers (segment ends, primitive positions and corners) on
  the artist's half-unit design grid, measured in raster pixels after mapping each file to
  the raster (within 0.02 px for a trace, written to two decimals; exact for the artist);
* **half pixels**: the same share on the raster's half-pixel grid (pixel edges at
  integers), the grid a tracer without a design unit could snap to;
* **ties**: the share of on-curve x and y values that another on-curve value of the same icon
  repeats exactly as written;
* **gate parameters** per icon.

    python3 bench/theory/design_stats.py --trace web --exe target/release/inkvec --out DIR
    python3 bench/theory/design_stats.py --compare artist DIR_512ssop:512ssop DIR_web:web

`--trace` writes one SVG per screen icon (`<family>__<stem>.svg`, quality mode) for a raster
tier; `--compare` prints the table for the artist's files and for each traced directory
(`DIR:TIER`, the tier giving the raster size the trace was made at).
"""

from __future__ import annotations

import argparse
import json
import math
import subprocess
import sys
import xml.etree.ElementTree as ET
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
from fractions import Fraction
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import design_prior as dp  # noqa: E402

#: A trace's number is "on the grid" within this distance, raster px (written to 0.005).
GRID_TOL_PX = 0.02


def screen_items() -> list[dict]:
    import svgeval
    return svgeval.load_sets()["screen"]


def raster_path(it: dict, tier: str) -> Path | None:
    import svgeval
    svgeval.set_tier(tier)
    png, _ = svgeval.item_paths(it)
    return png if png.exists() else None


def trace(tier: str, exe: Path, out: Path, workers: int) -> None:
    out.mkdir(parents=True, exist_ok=True)
    items = screen_items()

    def one(it):
        src = raster_path(it, tier)
        if src is None:
            return
        dst = out / f"{it['corpus']}__{it['stem']}.svg"
        subprocess.run([str(exe), str(src), "-o", str(dst), "--quiet", "--mode", "quality"],
                       capture_output=True, timeout=900)

    with ThreadPoolExecutor(workers) as ex:
        list(ex.map(one, items))


def view_box(text: str) -> tuple[Fraction, Fraction, Fraction]:
    root = ET.fromstring(text)
    vb = (root.get("viewBox") or "0 0 24 24").replace(",", " ").split()
    return dp.frac(vb[0]), dp.frac(vb[1]), max(dp.frac(vb[2]), dp.frac(vb[3]))


def raster_size(it: dict, tier: str) -> int:
    from PIL import Image
    p = raster_path(it, tier)
    return Image.open(p).size[0] if p else 0


def icon_stats(svg_text: str, art_text: str, n_px: int, exact: bool) -> Counter:
    """Counts for one file. `n_px`: the raster size the file is mapped to for the grid test;
    `exact`: the file is the artist's (grid by exact denominators in its own units)."""
    c: Counter = Counter()
    els, _ = dp.parse_doc(svg_text)
    ox, oy, V = view_box(svg_text)
    ax, ay, Va = view_box(art_text)
    for e in els:
        c["elements"] += 1
        c["gate"] += e.gate
        if e.tag in ("rect", "circle", "ellipse"):
            c["prim"] += 1
        for sp in e.subs:
            for s in sp["segs"]:
                k, letter = s[0], s[1]
                if letter in "Zz" or not dp.seg_length_nonzero(s):
                    continue
                c["seg"] += 1
                c["seg_" + k] += 1
                if k == "L":
                    dx, dy = dp.vec(s[2], s[3])
                    ang = math.degrees(math.atan2(abs(float(dy)), abs(float(dx))))
                    off_axis = min(ang, 90 - ang)
                    if dx == 0 or dy == 0:
                        c["line_axis"] += 1
                    elif abs(dx) == abs(dy):
                        c["line_45"] += 1
                    elif off_axis <= 0.5:
                        c["line_near_axis"] += 1
                    elif abs(ang - 45) <= 0.5:
                        c["line_near_45"] += 1
                    else:
                        c["line_other"] += 1
    # on-curve numbers: in the file's units, then on the artist's grid
    coords = on_curve_coords(els)
    xs = [v for a, v in coords if a == "x"]
    ys = [v for a, v in coords if a == "y"]
    for vals in (xs, ys):
        cnt = Counter(vals)
        c["tie_n"] += len(vals)
        c["tie_shared"] += sum(n for n in cnt.values() if n >= 2)
    for a, v in coords:
        c["grid_n"] += 1
        # the half-pixel grid of the raster, pixel edges at integers
        o = ox if a == "x" else oy
        X = (float(v) - float(o)) * n_px / float(V)
        c["halfpx_on"] += int(abs(2 * X - round(2 * X)) / 2 <= GRID_TOL_PX)
        if exact:
            c["grid_on"] += int(dp.denominator_class(v - (ax if a == "x" else ay))[0] <= 1)
        else:
            # raster px with pixel edges at integers -> artist units -> nearest half unit
            o = ox if a == "x" else oy
            X = (float(v) - float(o)) * n_px / float(V)
            u = float(ax if a == "x" else ay) + X * float(Va) / n_px
            half = round(2 * u) / 2
            dist_px = abs(u - half) * n_px / float(Va)
            c["grid_on"] += int(dist_px <= GRID_TOL_PX)
    return c


def on_curve_coords(els) -> list[tuple[str, Fraction]]:
    out = []
    for e in els:
        if e.tag == "rect" and e.prim:
            p = e.prim
            out += [("x", p["x"]), ("x", p["x"] + p["w"]), ("y", p["y"]), ("y", p["y"] + p["h"])]
        elif e.tag in ("circle", "ellipse") and e.prim:
            out += [("x", e.prim["cx"]), ("y", e.prim["cy"])]
        for sp in e.subs:
            pts = [sp["start"]] + [s[-1] for s in sp["segs"] if s[1] not in "Zz"]
            for p in pts:
                out += [("x", p[0]), ("y", p[1])]
    return out


def summarize(rows: list[Counter]) -> dict:
    t: Counter = Counter()
    for r in rows:
        t.update(r)
    lines = max(1, t["seg_L"])
    seg = max(1, t["seg"])
    return {
        "icons": len(rows),
        "gate_per_icon": t["gate"] / max(1, len(rows)),
        "prim_share": t["prim"] / max(1, t["elements"]),
        "line": t["seg_L"] / seg, "cubic": t["seg_C"] / seg, "quad": t["seg_Q"] / seg,
        "arc": t["seg_A"] / seg,
        "axis": t["line_axis"] / lines, "axis_near": t["line_near_axis"] / lines,
        "d45": t["line_45"] / lines, "d45_near": t["line_near_45"] / lines,
        "other": t["line_other"] / lines,
        "grid": t["grid_on"] / max(1, t["grid_n"]),
        "halfpx": t["halfpx_on"] / max(1, t["grid_n"]),
        "ties": t["tie_shared"] / max(1, t["tie_n"]),
    }


COLS = ["icons", "gate_per_icon", "prim_share", "line", "cubic", "arc", "axis", "axis_near",
        "d45", "other", "grid", "halfpx", "ties"]


def compare(sources: list[str], families: bool) -> dict:
    items = screen_items()
    table: dict = {}
    for src in sources:
        per_fam: dict[str, list[Counter]] = defaultdict(list)
        for it in items:
            art = (ROOT / "bench/data/corpus_svg" / it["corpus"] / f"{it['stem']}.svg").read_text(
                encoding="utf-8")
            if src == "artist":
                text, n_px, exact = art, 512, True
            else:
                d, tier = src.split(":")
                f = Path(d) / f"{it['corpus']}__{it['stem']}.svg"
                if not f.exists():
                    continue
                text, n_px, exact = f.read_text(encoding="utf-8"), raster_size(it, tier), False
            try:
                per_fam[it["corpus"]].append(icon_stats(text, art, n_px, exact))
            except Exception as ex:  # noqa: BLE001
                print(f"  skip {it['corpus']}/{it['stem']} ({src}): {ex}", file=sys.stderr)
        table[src] = {"ALL": summarize([r for rs in per_fam.values() for r in rs])}
        if families:
            for fam, rs in sorted(per_fam.items()):
                table[src][fam] = summarize(rs)
    return table


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--trace", metavar="TIER")
    ap.add_argument("--exe", type=Path)
    ap.add_argument("--out", type=Path)
    ap.add_argument("--workers", type=int, default=1)
    ap.add_argument("--compare", nargs="+", metavar="SOURCE")
    ap.add_argument("--families", action="store_true")
    ap.add_argument("--json", type=Path)
    a = ap.parse_args()
    if a.trace:
        trace(a.trace, a.exe, a.out, a.workers)
    if a.compare:
        table = compare(a.compare, a.families)
        print(f"{'source':34s} {'scope':15s} " + " ".join(f"{c:>8s}" for c in COLS))
        for src, scopes in table.items():
            for scope, row in scopes.items():
                cells = []
                for c in COLS:
                    v = row[c]
                    cells.append(f"{v:8d}" if isinstance(v, int) else f"{v:8.3f}")
                print(f"{src[-34:]:34s} {scope:15s} " + " ".join(cells))
        if a.json:
            a.json.write_text(json.dumps(table, indent=1))


if __name__ == "__main__":
    main()
