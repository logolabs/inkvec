"""The gate's turning axis on shapes whose answer is known. Needs svgelements (as CI has)."""
from __future__ import annotations

import math
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from inkvec_bench.turning import turning  # noqa: E402

SQUARE = '<path d="M0,0 L10,0 L10,10 L0,10 Z"/>'


class TurningTests(unittest.TestCase):
    def test_a_closed_square_turns_a_full_circle_over_its_perimeter(self):
        self.assertAlmostEqual(turning(SQUARE), 2 * math.pi / 40)

    def test_relative_commands_and_numbers_without_commas_read_the_same(self):
        self.assertAlmostEqual(turning('<path d="m0 0l10 0l0 10l-10 0z"/>'), turning(SQUARE))

    def test_the_jump_between_subpaths_is_not_turning(self):
        two = '<path d="M0,0 L10,0 L10,10 L0,10 Z M100,100 L110,100 L110,110 L100,110 Z"/>'
        self.assertAlmostEqual(turning(two), turning(SQUARE))

    def test_an_arc_reads_like_the_cubics_it_stands_for(self):
        arcs = '<path d="M10,0 A10,10 0 0,1 -10,0 A10,10 0 0,1 10,0 Z"/>'
        k = 10 * 0.5522847498
        cubics = (f'<path d="M10,0 C10,{k} {k},10 0,10 C-{k},10 -10,{k} -10,0 '
                  f'C-10,-{k} -{k},-10 0,-10 C{k},-10 10,-{k} 10,0 Z"/>')
        self.assertAlmostEqual(turning(arcs), turning(cubics), delta=0.002)

    def test_an_arcs_radii_and_flags_are_not_points(self):
        # The radii and flags of this arc, read as points, used to add three sharp turns.
        arc = '<path d="M0,0 A50,50 0 0,1 10,0"/>'
        self.assertLess(turning(arc), 0.2)

    def test_a_sawtooth_turns_more_than_the_straight_edge_it_follows(self):
        straight = '<path d="M0,0 L40,0"/>'
        saw = '<path d="M0,0 L5,1 L10,0 L15,1 L20,0 L25,1 L30,0 L35,1 L40,0"/>'
        self.assertEqual(turning(straight), 0.0)
        # Seven corners each turning 2 atan(1/5), over eight teeth of length sqrt(26).
        self.assertAlmostEqual(turning(saw), 7 * 2 * math.atan(0.2) / (8 * math.sqrt(26)))

    def test_a_primitive_reads_like_the_path_it_stands_for(self):
        rect = '<rect x="0" y="0" width="10" height="10"/>'
        self.assertAlmostEqual(turning(rect), turning(SQUARE))
        circle = '<circle cx="0" cy="0" r="10"/>'
        arcs = '<path d="M10,0 A10,10 0 0,1 -10,0 A10,10 0 0,1 10,0 Z"/>'
        self.assertAlmostEqual(turning(circle), turning(arcs), delta=0.002)

    def test_no_paths_reads_zero(self):
        self.assertEqual(turning('<svg xmlns="http://www.w3.org/2000/svg"/>'), 0.0)


if __name__ == "__main__":
    unittest.main()
