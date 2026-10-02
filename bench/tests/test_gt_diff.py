"""gt_diff's label render must paint every region the artist's file paints, whichever element
the paint was written on. Needs numpy, scipy, Pillow and resvg-py; no Rust build."""
from __future__ import annotations

import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import gt_diff  # noqa: E402

# How lucide writes every icon: paint on the root, none on the paths.
LUCIDE_LIKE = (
    '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" '
    'fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" '
    'stroke-linejoin="round"><path d="M4 6h16"/><circle cx="12" cy="15" r="5"/></svg>'
)


class LabelledDocumentTests(unittest.TestCase):
    def test_stroke_inherited_from_the_root_is_a_stroked_region(self):
        doc, regions = gt_diff.labelled_document(LUCIDE_LIKE)
        self.assertEqual([r.stroked for r in regions], [True, True])
        self.assertEqual([r.fill for r in regions], ["none", "none"])
        lab, unknown = gt_diff.label_map(doc, regions, 96)
        for r in regions:
            self.assertGreater(int((lab == r.label).sum()), 50, f"region {r.label} not painted")
        self.assertLess(unknown, 0.05)

    def test_inherited_stroke_none_and_style_override(self):
        svg = ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">'
               '<g stroke="#f00" stroke-width="2">'
               '<rect x="2" y="2" width="8" height="8" stroke="none"/>'
               '<rect x="14" y="2" width="8" height="8"/>'
               '<g style="stroke:none"><rect x="2" y="14" width="8" height="8"/></g>'
               '</g><rect x="14" y="14" width="8" height="8"/></svg>')
        _, regions = gt_diff.labelled_document(svg)
        self.assertEqual([r.stroked for r in regions], [False, True, False, False])
        self.assertEqual([r.fill for r in regions], ["flat"] * 4)

    def test_a_stroked_icon_matches_itself(self):
        diff, _ = gt_diff.compare("lucide-like", LUCIDE_LIKE, LUCIDE_LIKE, 96, 24, min_area_src=0.5)
        self.assertEqual(diff.n_gt, 2)
        self.assertEqual(diff.classes.get("matched"), 2)
        self.assertAlmostEqual(diff.de_mean, 0.0, places=6)


if __name__ == "__main__":
    unittest.main()
