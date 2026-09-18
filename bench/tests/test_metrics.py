"""Tests for the metric definitions.

A benchmark's credibility rests entirely on its metrics meaning what they claim, so
these test the *definitions* against cases with known answers rather than testing that
the code runs.
"""
from __future__ import annotations

import math

import numpy as np
import pytest

from inkvec_bench import render, svgmodel
from inkvec_bench.metrics import editability as m_edit
from inkvec_bench.report import pareto

R = 40.0
C = 50.0
KAPPA = 0.5522847498307936


def _svg(body: str) -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" '
        f'xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 100 100" '
        f'width="100" height="100">{body}</svg>'
    )


CIRCLE_PRIM = _svg(f'<circle cx="{C}" cy="{C}" r="{R}" fill="#e33"/>')
_k = R * KAPPA
CIRCLE_CUBIC = _svg(
    f'<path d="M{C+R} {C} C{C+R} {C+_k} {C+_k} {C+R} {C} {C+R} '
    f'C{C-_k} {C+R} {C-R} {C+_k} {C-R} {C} '
    f'C{C-R} {C-_k} {C-_k} {C-R} {C} {C-R} '
    f'C{C+_k} {C-R} {C+R} {C-_k} {C+R} {C} Z" fill="#e33"/>'
)


class TestStructuralModel:
    def test_primitive_circle_costs_three_numbers(self):
        d = svgmodel.parse(CIRCLE_PRIM)
        assert d.parse_error is None
        assert d.n_anchors == 0
        assert d.n_params == 3
        assert d.primitive_fraction == pytest.approx(1.0)

    def test_same_circle_as_cubics_costs_eight_times_more(self):
        d = svgmodel.parse(CIRCLE_CUBIC)
        assert d.n_anchors == 4
        assert d.n_params == 24
        assert d.primitive_fraction == pytest.approx(0.0)

    def test_both_circles_render_almost_identically(self):
        """The whole point: pixel metrics cannot separate these two documents."""
        a = render.render(CIRCLE_PRIM, 256, 256)
        b = render.render(CIRCLE_CUBIC, 256, 256)
        assert np.abs(a - b).mean() < 0.002

    def test_boundary_length_is_the_real_circumference(self):
        d = svgmodel.parse(CIRCLE_PRIM)
        assert d.total_length == pytest.approx(2 * math.pi * R, rel=0.01)

    def test_anchor_density_normalises_by_length(self):
        prim = svgmodel.parse(CIRCLE_PRIM)
        cubic = svgmodel.parse(CIRCLE_CUBIC)
        assert prim.anchor_density == 0.0
        assert cubic.anchor_density > 0.0

    def test_malformed_svg_does_not_raise(self):
        d = svgmodel.parse("<svg><path d='M0 0 L'")
        assert d.parse_error is not None
        assert d.n_elements == 0


class TestUseAndDefs:
    """Reuse must be rewarded for what it saves, and no more."""

    REUSED = _svg(
        '<defs><path id="p" d="M10 10 C20 20 30 30 40 10 Z" fill="#f00"/></defs>'
        '<use xlink:href="#p"/>'
        '<use xlink:href="#p" transform="translate(0,40)"/>'
        '<use xlink:href="#p" transform="translate(0,80)"/>'
    )
    COPIED = _svg(
        '<path d="M10 10 C20 20 30 30 40 10 Z" fill="#f00"/>'
        '<path d="M10 50 C20 60 30 70 40 50 Z" fill="#f00"/>'
        '<path d="M10 90 C20 100 30 110 40 90 Z" fill="#f00"/>'
    )

    def test_defs_geometry_is_counted_once_not_zero(self):
        """Hiding geometry behind <use> must not make it free."""
        d = svgmodel.parse(self.REUSED)
        assert d.n_anchors > 0, "defs geometry was invisible — anchors could be hidden"

    def test_reuse_beats_copying_on_anchor_density(self):
        reused = svgmodel.parse(self.REUSED)
        copied = svgmodel.parse(self.COPIED)
        assert reused.n_anchors < copied.n_anchors
        assert reused.anchor_density < copied.anchor_density

    def test_use_still_contributes_boundary_length(self):
        reused = svgmodel.parse(self.REUSED)
        copied = svgmodel.parse(self.COPIED)
        assert reused.total_length == pytest.approx(copied.total_length, rel=0.05)

    def test_svg2_plain_href_is_also_resolved(self):
        """SVG 2 dropped the xlink prefix; emitters use bare ``href``. Both must work."""
        plain = self.REUSED.replace("xlink:href", "href")
        d = svgmodel.parse(plain)
        assert d.parse_error is None
        assert d.n_anchors > 0
        assert d.total_length > 0


class TestOverdraw:
    def test_disjoint_regions_have_overdraw_one(self):
        svg = _svg(
            '<path d="M0 0 L50 0 L50 100 L0 100 Z" fill="#f00"/>'
            '<path d="M50 0 L100 0 L100 100 L50 100 Z" fill="#00f"/>'
        )
        m = m_edit.coverage_metrics(svgmodel.parse(svg))
        assert m["overdraw_ratio"] == pytest.approx(1.0, rel=0.02)

    def test_fully_overlapping_regions_have_overdraw_two(self):
        svg = _svg(
            '<path d="M0 0 L100 0 L100 100 L0 100 Z" fill="#f00"/>'
            '<path d="M0 0 L100 0 L100 100 L0 100 Z" fill="#00f"/>'
        )
        m = m_edit.coverage_metrics(svgmodel.parse(svg))
        assert m["overdraw_ratio"] == pytest.approx(2.0, rel=0.02)

    def test_self_intersecting_ring_is_detected(self):
        svg = _svg('<path d="M0 0 L100 100 L100 0 L0 100 Z" fill="#f00"/>')  # bowtie
        m = m_edit.coverage_metrics(svgmodel.parse(svg))
        assert m["self_intersections"] >= 1


class TestSeam:
    def test_no_seam_when_candidate_covers_reference(self):
        ref = render.render(_svg('<rect x="10" y="10" width="80" height="80" fill="#000"/>'), 64, 64)
        m = m_edit.seam_metrics(ref, ref)
        assert m["seam_fraction"] == pytest.approx(0.0, abs=1e-6)

    def test_gap_between_two_regions_is_detected(self):
        ref = render.render(_svg('<rect x="0" y="0" width="100" height="100" fill="#000"/>'), 64, 64)
        # Two halves separated by a 2-unit gap: background shows through the middle.
        cand = render.render(
            _svg('<rect x="0" y="0" width="49" height="100" fill="#000"/>'
                 '<rect x="51" y="0" width="49" height="100" fill="#000"/>'),
            64, 64,
        )
        m = m_edit.seam_metrics(ref, cand)
        assert m["seam_fraction"] > 0.005


class TestPareto:
    def test_frontier_keeps_only_non_dominated_points(self):
        cost = np.array([1.0, 2.0, 3.0, 4.0])
        quality = np.array([0.5, 0.9, 0.7, 1.0])  # index 2 is dominated by index 1
        mask = pareto.pareto_mask(cost, quality, higher_is_better=True)
        assert list(mask) == [True, True, False, True]

    def test_lower_is_better_direction(self):
        cost = np.array([1.0, 2.0, 3.0])
        err = np.array([0.5, 0.2, 0.3])  # index 2 dominated by index 1
        mask = pareto.pareto_mask(cost, err, higher_is_better=False)
        assert list(mask) == [True, True, False]

    def test_non_finite_points_are_dropped(self):
        cost = np.array([1.0, np.nan, 3.0])
        quality = np.array([0.5, 0.9, np.inf])
        mask = pareto.pareto_mask(cost, quality, higher_is_better=True)
        assert mask[0] and not mask[1] and not mask[2]


class TestPerturbation:
    def test_boundary_warp_is_deterministic_and_moves_pixels(self):
        from inkvec_bench.corpora import perturb

        a = render.render(CIRCLE_PRIM, 128, 128)
        w1 = perturb.warp_boundaries(a, seed=0)
        w2 = perturb.warp_boundaries(a, seed=0)
        assert np.array_equal(w1, w2)
        assert np.abs(w1 - a).mean() > 1e-4

    def test_jpeg_roundtrip_degrades(self):
        from inkvec_bench.corpora import perturb

        a = render.render(CIRCLE_PRIM, 128, 128)
        j = perturb.jpeg_roundtrip(a, quality=40)
        assert j.shape == a.shape
        assert np.abs(render.composite(j) - render.composite(a)).mean() > 1e-4
