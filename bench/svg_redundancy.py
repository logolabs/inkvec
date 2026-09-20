"""Where does an artist SVG's description length hide, beyond its segments?

    python bench/svg_redundancy.py [--n 40] [--size 256]

Per file: how many paths share a fill (a palette index is cheaper than a colour string),
how many `d` strings repeat verbatim (a `<use>` is cheaper than a copy), and how much of
each path's painted area is covered by paths above it (geometry nobody sees costs the same
to describe as geometry everyone sees). Occlusion is measured by rendering each path alone
and comparing with the finished picture at `--size`.
"""
from __future__ import annotations

import argparse
import re
import sys
from collections import Counter
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

import svgeval  # noqa: E402
from inkvec_bench import render  # noqa: E402

PATH_RE = re.compile(r"<path\b[^>]*?/>", re.S)


def alpha(svg: str, size: int) -> np.ndarray:
    img = render.render(svg, size, size)
    return img[..., 3] if img.shape[-1] == 4 else np.ones(img.shape[:2], np.float32)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=40)
    ap.add_argument("--size", type=int, default=256)
    a = ap.parse_args()
    items = svgeval.load_sets()["screen"]
    step = max(1, len(items) // a.n)
    items = items[::step][: a.n]

    per_family: dict[str, list] = {}
    print(f"{'icon':38} {'paths':>5} {'fills':>5} {'dup d':>5} {'hidden area':>11} {'bytes':>7}")
    for it in items:
        _, gt = svgeval.item_paths(it)
        svg = gt.read_text(encoding="utf-8")
        paths = PATH_RE.findall(svg)
        if not paths:
            continue
        fills = Counter(re.search(r'fill="([^"]+)"', p).group(1) if re.search(r'fill="([^"]+)"', p) else "-"
                        for p in paths)
        ds = Counter(m.group(1) for p in paths for m in [re.search(r'(?<![a-zA-Z])d="([^"]+)"', p)] if m)
        dup = sum(c - 1 for c in ds.values() if c > 1)

        # Occlusion: each path alone, against the picture with everything painted.
        hidden_area = total_area = 0.0
        try:
            m = re.search(r"<svg\b[^>]*>", svg)
            head, tail = svg[: m.end()], "</svg>"
            body_end = svg.rfind("</svg>")
            full = alpha(svg, a.size)
            # Everything in document order, accumulated: a path's hidden pixels are those
            # it paints that the paths after it paint over.
            spans = [(mm.start(), mm.end()) for mm in PATH_RE.finditer(svg)]
            defs = svg[m.end(): spans[0][0]] if spans else ""
            for i, (s, e) in enumerate(spans):
                alone = alpha(head + defs + svg[s:e] + tail, a.size)
                after = alpha(head + defs + "".join(svg[ss:ee] for ss, ee in spans[i + 1:]) + tail, a.size)
                painted = alone > 0.5
                covered = painted & (after > 0.99)
                total_area += painted.sum()
                hidden_area += covered.sum()
        except Exception as ex:  # a file the naive splitter cannot rebuild
            print(f"{it['corpus'] + '/' + it['stem']:38} occlusion skipped: {str(ex)[:40]}")
            total_area = 0.0
        hidden = hidden_area / total_area if total_area else float("nan")
        per_family.setdefault(it["corpus"], []).append((len(paths), len(fills), dup, hidden, len(svg.encode())))
        print(f"{it['corpus'] + '/' + it['stem']:38} {len(paths):5} {len(fills):5} {dup:5} {hidden:10.1%} {len(svg.encode()):7}")

    print(f"\n{'family':16} {'n':>3} {'paths/file':>10} {'fills/file':>10} {'paths/fill':>10} {'dup d':>6} {'hidden area':>11}")
    for fam, rows in sorted(per_family.items()):
        p = np.array([r[0] for r in rows]); f = np.array([r[1] for r in rows])
        d = sum(r[2] for r in rows); h = [r[3] for r in rows if r[3] == r[3]]
        print(f"{fam:16} {len(rows):3} {p.mean():10.1f} {f.mean():10.1f} {p.sum() / max(f.sum(), 1):10.1f} {d:6} "
              f"{(np.mean(h) if h else float('nan')):10.1%}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
