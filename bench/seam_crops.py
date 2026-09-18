"""Render vtracer-1.0-default (hierarchical=stacked, the old default) and
vtracer-1.0-simplify (--hierarchical cutout, the new "seam-free mosaic") SVGs
at high resolution for a few multi-shape cases, and save zoomed crops at a
shared internal seam so hairline gaps (anti-aliased background showing through
between two touching fills) are visible if present.

    python bench/seam_crops.py
"""
from __future__ import annotations
import sys
from pathlib import Path
import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT), str(ROOT / 'bench')]
from inkvec_bench import render

CASE_DIR = ROOT / 'out/crosscompare-competitors-2026-09-15/cases'
OUT = ROOT / 'out/crosscompare-competitors-2026-09-15/seam-crops'
RES = 3000

# (case_key, crop_box_fraction (l,t,r,b) of the RES x RES render, label)
TARGETS = [
    ('synthetic__mosaic_pie6', (0.30, 0.30, 0.70, 0.70), 'pie-center (6 wedges meet)'),
    ('brands__sangchaimeter_com', (0.00, 0.00, 1.00, 1.00), 'full logo (many adjacent flats)'),
    ('openmoji__1F334', (0.25, 0.15, 0.85, 0.65), 'chestnut shading boundaries'),
]


def render_crop(svg_path: Path, box, res=RES):
    svg = svg_path.read_text(encoding='utf-8')
    arr = render.composite(render.render(svg, res, res))  # RGB float [0,1] over white
    img = Image.fromarray(np.clip(np.rint(arr * 255), 0, 255).astype('uint8'))
    l, t, r, b = box
    crop = img.crop((int(l * res), int(t * res), int(r * res), int(b * res)))
    return crop


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for key, box, label in TARGETS:
        d = CASE_DIR / key / '512'
        if not d.exists():
            print('MISSING', key); continue
        for engine in ['vtracer-1.0-default', 'vtracer-1.0-simplify']:
            svg_path = d / f'{engine}.svg'
            crop = render_crop(svg_path, box)
            out_path = OUT / f'{key}__{engine}.png'
            crop.save(out_path)
            print(f'{key} [{label}] {engine} -> {out_path}')
        # Side-by-side composite for easy comparison
        a = Image.open(OUT / f'{key}__vtracer-1.0-default.png')
        b = Image.open(OUT / f'{key}__vtracer-1.0-simplify.png')
        w, h = a.size
        combo = Image.new('RGB', (w * 2 + 20, h), 'white')
        combo.paste(a, (0, 0)); combo.paste(b, (w + 20, 0))
        combo.save(OUT / f'{key}__stacked_vs_cutout.png')
    print('DONE')


if __name__ == '__main__':
    main()
