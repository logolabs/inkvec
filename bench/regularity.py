"""How much regular form does the artist have that we lose?

    python bench/regularity.py --exe target/release/inkvec.exe --set screen

Perceptual form is mostly regularity: a circle that is round, a glyph that is symmetric,
an edge that is level, a shape that repeats. Colour error barely sees any of it, so before
building anything to recover it, this measures how much of it we are actually losing.

Three readings per icon, each on the *rendered* image so it judges what a person sees and
not how either file is written:

* **symmetry residual** - mean absolute difference between the render and its mirror, for
  a vertical axis, a horizontal axis, and a half turn. An icon the artist drew symmetric
  scores near zero; ours scores near zero only if we kept the symmetry.
* **axis alignment** - the share of boundary length running within a degree of horizontal
  or vertical, measured from the path data. Artists snap to the grid; a tracer wobbles.
* **circularity** - the share of closed boundaries whose points sit within a tenth of a
  pixel of their own best-fit circle, again from the path data.

The number that matters is the *gap on the icons where the artist was regular*: an icon
the artist drew asymmetric tells us nothing, and snapping it would be a defect.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "bench"))
import svgeval  # noqa: E402
from inkvec_bench import render  # noqa: E402

SIZE = 256


def symmetry(img: np.ndarray) -> dict:
    """Mean |I - mirror(I)| for the three mirrors, in the render's own units."""
    return {
        "v": float(np.abs(img - img[:, ::-1]).mean()),
        "h": float(np.abs(img - img[::-1, :]).mean()),
        "r": float(np.abs(img - img[::-1, ::-1]).mean()),
    }


NUM = re.compile(r"-?\d*\.?\d+(?:[eE][-+]?\d+)?")


def path_points(svg: str) -> list[list[tuple[float, float]]]:
    """Anchor points of each path, which is all these two readings need."""
    out = []
    for d in re.findall(r'\sd="([^"]+)"', svg):
        pts: list[tuple[float, float]] = []
        cur = (0.0, 0.0)
        for m in re.finditer(r"([MmLlHhVvCcSsQqTtAaZz])([^MmLlHhVvCcSsQqTtAaZz]*)", d):
            cmd, rest = m.group(1), m.group(2)
            v = [float(x) for x in NUM.findall(rest)]
            if cmd in "Mm" and len(v) >= 2:
                cur = (v[0], v[1])
                pts.append(cur)
            elif cmd in "Ll" and len(v) >= 2:
                for i in range(0, len(v) - 1, 2):
                    cur = (v[i], v[i + 1])
                    pts.append(cur)
            elif cmd in "Cc" and len(v) >= 6:
                for i in range(0, len(v) - 5, 6):
                    cur = (v[i + 4], v[i + 5])
                    pts.append(cur)
            elif cmd in "Aa" and len(v) >= 7:
                for i in range(0, len(v) - 6, 7):
                    cur = (v[i + 5], v[i + 6])
                    pts.append(cur)
        if len(pts) >= 3:
            out.append(pts)
    return out


def axis_share(paths) -> float:
    """Share of straight-run length within a degree of an axis."""
    total = aligned = 0.0
    for pts in paths:
        for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
            dx, dy = x1 - x0, y1 - y0
            n = float(np.hypot(dx, dy))
            if n < 1e-9:
                continue
            total += n
            if abs(dx) < 0.0175 * n or abs(dy) < 0.0175 * n:
                aligned += n
    return aligned / total if total > 0 else 0.0


def circle_share(paths) -> float:
    """Share of closed boundaries that are circles to within a tenth of a pixel."""
    if not paths:
        return 0.0
    good = 0
    for pts in paths:
        p = np.array(pts, dtype=float)
        if len(p) < 6:
            continue
        # Algebraic circle fit: x^2+y^2 = 2ax + 2by + c.
        A = np.column_stack([2 * p[:, 0], 2 * p[:, 1], np.ones(len(p))])
        b = (p ** 2).sum(1)
        try:
            sol, *_ = np.linalg.lstsq(A, b, rcond=None)
        except np.linalg.LinAlgError:
            continue
        cx, cy, c = sol
        r2 = c + cx * cx + cy * cy
        if r2 <= 0:
            continue
        r = float(np.sqrt(r2))
        dev = np.abs(np.hypot(p[:, 0] - cx, p[:, 1] - cy) - r)
        if dev.max() < 0.1 and r > 1.0:
            good += 1
    return good / len(paths)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--set", default="screen")
    ap.add_argument("--sym-threshold", type=float, default=0.01,
                    help="below this residual the artist's icon counts as symmetric")
    ap.add_argument("--out", default="out/regularity.json")
    a = ap.parse_args()

    items = svgeval.load_sets()[a.set]
    exe = Path(a.exe).resolve()
    work = svgeval.WORK / "_reg"
    work.mkdir(parents=True, exist_ok=True)
    rows = []
    for n, it in enumerate(items):
        png, gt = svgeval.item_paths(it)
        out = work / f"{it['corpus']}__{it['stem']}.svg"
        r = subprocess.run([str(exe), str(png), "-o", str(out), "--quiet"],
                           capture_output=True)
        if r.returncode != 0:
            continue
        gsvg = gt.read_text(encoding="utf-8")
        osvg = out.read_text(encoding="utf-8")
        try:
            gi = render.composite(render.render(gsvg, SIZE, SIZE))
            oi = render.composite(render.render(osvg, SIZE, SIZE))
        except BaseException:
            continue
        gp, op = path_points(gsvg), path_points(osvg)
        rows.append({
            "corpus": it["corpus"], "stem": it["stem"],
            "gt_sym": symmetry(gi), "our_sym": symmetry(oi),
            "gt_axis": axis_share(gp), "our_axis": axis_share(op),
            "gt_circ": circle_share(gp), "our_circ": circle_share(op),
        })
        if n % 25 == 0:
            print(f"  {n}/{len(items)}", flush=True)

    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    Path(a.out).write_text(json.dumps(rows, indent=1), encoding="utf-8")

    print(f"\n{len(rows)} icons\n")
    for axis, name in (("v", "vertical mirror"), ("h", "horizontal mirror"), ("r", "half turn")):
        sym = [r for r in rows if r["gt_sym"][axis] < a.sym_threshold]
        if not sym:
            continue
        g = np.mean([r["gt_sym"][axis] for r in sym])
        o = np.mean([r["our_sym"][axis] for r in sym])
        kept = sum(1 for r in sym if r["our_sym"][axis] < a.sym_threshold)
        print(f"{name:18s} artist symmetric on {len(sym):3d} icons "
              f"(residual {g:.5f});  ours {o:.5f}, still symmetric on {kept:3d} "
              f"({100 * kept / len(sym):.0f}%)")
    print()
    print(f"{'axis-aligned length':18s} artist {np.mean([r['gt_axis'] for r in rows]):.3f}"
          f"   ours {np.mean([r['our_axis'] for r in rows]):.3f}")
    print(f"{'circular boundaries':18s} artist {np.mean([r['gt_circ'] for r in rows]):.3f}"
          f"   ours {np.mean([r['our_circ'] for r in rows]):.3f}")
    worst = sorted(rows, key=lambda r: r["gt_sym"]["v"] - r["our_sym"]["v"])[:8]
    print("\nlargest vertical-symmetry losses:")
    for r in worst:
        print(f"  {r['corpus']}/{r['stem']:40s} artist {r['gt_sym']['v']:.5f} "
              f"-> ours {r['our_sym']['v']:.5f}")


if __name__ == "__main__":
    main()
