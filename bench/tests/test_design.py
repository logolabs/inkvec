"""The design battery (`inkvec_bench/design.py`): each statistic on files whose answer is
known by construction, and the divergence's zero on a file against itself or against the same
drawing written in another frame. numpy, scipy and scikit-image; no renders, no tracer."""
from __future__ import annotations

import math
import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from inkvec_bench import design

HEAD = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vb}">'


def svg(body: str, vb: str = "0 0 24 24") -> str:
    return HEAD.format(vb=vb) + body + "</svg>"


SQUARE = svg('<rect x="4" y="4" width="16" height="16" fill="#123456"/>')
CIRCLE = svg('<circle cx="12" cy="12" r="8" fill="#d22f27"/>')


class ParseTests(unittest.TestCase):
    def test_rect_is_four_right_angled_axis_lines(self):
        p = design.profile(SQUARE)
        self.assertEqual(p["kinds"], [1.0, 0.0, 0.0, 0.0])
        self.assertEqual(p["line_axis"], 1.0)
        self.assertEqual(p["corner_share"], 1.0)
        self.assertEqual(p["right_angle"], 1.0)
        self.assertEqual(p["closed"], 1.0)
        self.assertEqual(p["primitives"], 1.0)
        self.assertEqual(p["nodes_per_subpath"], 4.0)
        self.assertEqual(p["grid_int"], 1.0)
        self.assertEqual(p["grid_level"], 0.0)
        self.assertEqual(p["decimals"], 0.0)
        self.assertEqual(p["mirror"], 1.0)

    def test_circle_is_four_smooth_quarter_arcs_at_the_extrema(self):
        p = design.profile(CIRCLE)
        self.assertEqual(p["kinds"], [0.0, 0.0, 0.0, 1.0])
        self.assertEqual(p["smooth"], 1.0)
        self.assertEqual(p["extremum"], 1.0)
        self.assertEqual(p["wide_curves"], 0.0)
        for s in p["sweep"]:
            self.assertAlmostEqual(s, 90.0, places=6)

    def test_relative_commands_reflections_and_compact_arc_flags(self):
        # The same square as SQUARE, written relative; a compact arc `a4 4 0 014 4`.
        rel = design.profile(svg('<path d="m4 4h16v16h-16z"/>'))
        self.assertEqual(rel["kinds"], [1.0, 0.0, 0.0, 0.0])
        self.assertEqual(rel["nodes_per_subpath"], 4.0)
        arc = design.parse(svg('<path d="M4 8a4 4 0 014-4"/>')).elements[0].subs[0].segs[0]
        self.assertEqual(arc.kind, "A")
        self.assertAlmostEqual(arc.p1[0] * 24, 8.0)
        self.assertAlmostEqual(arc.p1[1] * 24, 4.0)
        # S reflects the previous control point: a symmetric S-curve has two cubics.
        s = design.parse(svg('<path d="M2 12C2 6 8 6 8 12S14 18 14 12"/>')).elements[0].subs[0].segs
        self.assertEqual([x.kind for x in s], ["C", "C"])
        self.assertAlmostEqual(s[1].pts[1][0] * 24, 8.0)
        self.assertAlmostEqual(s[1].pts[1][1] * 24, 18.0)

    def test_transforms_are_applied(self):
        diamond = design.profile(svg('<g transform="rotate(45 12 12)"><rect x="6" y="6" '
                                     'width="12" height="12"/></g>'))
        self.assertEqual(diamond["line_diag"], 1.0)
        self.assertEqual(diamond["line_axis"], 0.0)

    def test_paint_is_inherited_and_strokes_counted(self):
        p = design.profile(svg('<g fill="none" stroke="currentColor" stroke-width="2">'
                               '<path d="M4 4L20 20"/><path d="M4 20L20 4"/></g>'))
        self.assertEqual(p["stroked"], 1.0)
        self.assertEqual(p["elements"], 2)
        self.assertEqual(p["palette"], [[0, 0, 0]])
        self.assertEqual(p["closed"], 0.0)

    def test_a_white_page_is_not_part_of_the_drawing(self):
        page = svg('<path d="M-0.5 -0.5H511.5V511.5H-0.5Z" fill="#ffffff"/>'
                   '<path d="M127.5 127.5H383.5V383.5H127.5Z" fill="#123456"/>',
                   vb="-0.5 -0.5 512 512")
        p = design.profile(page)
        self.assertTrue(p["page"])
        self.assertEqual(p["elements"], 1)
        self.assertFalse(design.profile(SQUARE)["page"])

    def test_half_pixel_grid(self):
        # 4 and 20 units of 24 on a 48 px raster are 8 and 40 px: on the grid. On a 50 px
        # raster, 8.33 and 41.67 px: off it.
        self.assertEqual(design.profile(SQUARE, raster_px=48)["halfpx"], 1.0)
        self.assertEqual(design.profile(SQUARE, raster_px=50)["halfpx"], 0.0)
        self.assertTrue(math.isnan(design.profile(SQUARE)["halfpx"]))
        self.assertIn("halfpx", design.ARTEFACTS)

    def test_nested_and_stacked(self):
        ring = design.profile(svg('<path d="M2 2H22V22H2ZM8 8V16H16V8Z" fill-rule="evenodd"/>'))
        self.assertEqual(ring["nested"], 0.5)
        self.assertEqual(ring["evenodd"], 1.0)
        stack = design.profile(svg('<rect x="2" y="2" width="20" height="20" fill="#fff000"/>'
                                   '<circle cx="12" cy="12" r="4" fill="#000"/>'))
        self.assertEqual(stack["stacked"], 0.5)


class DivergenceTests(unittest.TestCase):
    def test_a_file_against_itself_is_zero(self):
        for f in (SQUARE, CIRCLE):
            p = design.profile(f)
            for k, v in design.divergence(p, p).items():
                self.assertTrue(math.isnan(v) or v == 0.0, (k, v))

    def test_the_same_drawing_in_the_trace_frame_is_zero(self):
        # The square drawn as a 512 px trace writes it: pixel centres at integers, two
        # decimals. Every statistic reads the same, the grid ones within the tolerance.
        k = 512 / 24
        trace = svg(f'<path d="M{4 * k - 0.5:.2f} {4 * k - 0.5:.2f}H{20 * k - 0.5:.2f}'
                    f'V{20 * k - 0.5:.2f}H{4 * k - 0.5:.2f}Z" fill="#123456"/>',
                    vb="-0.5 -0.5 512 512")
        c = design.compare(trace, SQUARE, 512)
        for name, v in c["div"].items():
            if name == "primitives":       # a path, where the artist wrote a <rect>
                self.assertEqual(v, 1.0)
                continue
            self.assertTrue(math.isnan(v) or v < 1e-4, (name, v))   # two decimals

    def test_divergence_kinds(self):
        a = {"kinds": [1.0, 0.0, 0.0, 0.0], "elements": 4, "line_axis": 0.75,
             "sweep": [90.0, 90.0], "palette": [[0, 0, 0]]}
        t = {"kinds": [0.5, 0.5, 0.0, 0.0], "elements": 8, "line_axis": 0.25,
             "sweep": [45.0, 45.0], "palette": [[0, 0, 0], [255, 255, 255]]}
        d = design.divergence(t, a)
        self.assertAlmostEqual(d["kinds"], 0.5)
        self.assertAlmostEqual(d["elements"], math.log(2))
        self.assertAlmostEqual(d["line_axis"], 0.5)
        self.assertAlmostEqual(d["sweep"], 45.0 / 180.0)
        self.assertAlmostEqual(d["palette"], 25.0, places=3)   # white is 100 from black, half the set
        self.assertTrue(math.isnan(d["smooth"]))

    def test_w1_matches_scipy(self):
        from scipy.stats import wasserstein_distance
        rng = np.random.default_rng(1)
        for _ in range(20):
            a, b = rng.normal(size=rng.integers(1, 40)), rng.gamma(2.0, size=rng.integers(1, 40))
            self.assertAlmostEqual(design._w1(a, b), wasserstein_distance(a, b), places=12)

    def test_report_aggregates_and_pruning(self):
        import human_stats
        per = {"a/1": {"div": {"x": 1.0}}, "a/2": {"div": {"x": 3.0}},
               "b/1": {"div": {"x": 10.0}}, "b/2": {"div": {"x": float("nan")}}}
        m = human_stats.family_mean(per, "x", "div")
        self.assertEqual(m, {"a": 2.0, "b": 10.0, "macro": 6.0})
        rng = np.random.default_rng(2)
        base = rng.normal(size=200)
        cols = {"g": rng.normal(size=200), "s1": base, "s2": base * 3 + 0.01 * rng.normal(size=200),
                "s3": rng.normal(size=200)}
        names, rho = human_stats.matrix(cols)
        self.assertAlmostEqual(rho[1, 1], 1.0)
        kept, dropped = human_stats.prune(names, rho, fixed=("g",))
        self.assertEqual(kept, ["g", "s1", "s3"])
        self.assertEqual(dropped["s2"][0], "s1")

    def test_tracer_flags_are_taken_as_values(self):
        import human_stats
        self.assertEqual(human_stats.flag_values(["--exe", "x", "--extra-args", "--editability",
                                                  "--base-args", ""]),
                         ["--exe", "x", "--extra-args=--editability", "--base-args="])

    def test_kept_statistics_are_defined(self):
        self.assertTrue(set(design.KEPT) <= set(design.STATS))
        self.assertEqual(len(set(design.KEPT)), len(design.KEPT))


if __name__ == "__main__":
    unittest.main()
