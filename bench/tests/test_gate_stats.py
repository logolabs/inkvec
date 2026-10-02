"""The gate's statistics and decision rule, on inputs with known answers. numpy only."""
from __future__ import annotations

import math
import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import gate_stats as gs  # noqa: E402


def two_families(n_a: int = 40, n_b: int = 10, seed: int = 1):
    rng = np.random.default_rng(seed)
    base, fam = {}, {}
    for f, n, level in (("a", n_a, 0.1), ("b", n_b, 0.4)):
        for i in range(n):
            k = f"{f}/{i}"
            base[k] = float(level * rng.uniform(0.5, 1.5))
            fam[k] = f
    return base, fam


class CompareTests(unittest.TestCase):
    def test_identical_scores_are_identical(self):
        base, fam = two_families()
        c = gs.compare(base, dict(base), fam)
        self.assertEqual((c.changed, c.rel, c.p05, c.p95, c.se), (0, 0.0, 0.0, 0.0, 0.0))
        self.assertEqual(gs.decide(c, 0.0).label, "identical")
        self.assertTrue(gs.decide(c, 0.0).passed)

    def test_aggregates_by_hand(self):
        base = {"a/1": 1.0, "a/2": 3.0, "b/1": 10.0}
        fam = {k: k[0] for k in base}
        cur = dict(base, **{"b/1": 12.0})
        macro = gs.compare(base, cur, fam, "macro")
        self.assertAlmostEqual(macro.base, (2.0 + 10.0) / 2)
        self.assertAlmostEqual(macro.cur, (2.0 + 12.0) / 2)
        micro = gs.compare(base, cur, fam, "micro")
        self.assertAlmostEqual(micro.base, 14.0 / 3)
        self.assertAlmostEqual(micro.cur, 16.0 / 3)
        self.assertAlmostEqual(micro.rel, 16.0 / 14.0 - 1)
        with self.assertRaises(ValueError):
            gs.compare(base, cur, fam, "median")

    def test_proportional_change_has_a_degenerate_interval(self):
        # Every icon 10 % worse: every resample's ratio of means is exactly 1.1.
        base, fam = two_families()
        cur = {k: v * 1.1 for k, v in base.items()}
        for agg in gs.AGGREGATES:
            c = gs.compare(base, cur, fam, agg)
            for q in (c.rel, c.p025, c.p05, c.p95, c.p975):
                self.assertAlmostEqual(q, 0.1, places=9)
            self.assertEqual(c.worse, len(base))
            self.assertEqual(gs.decide(c, 0.02).label, "worse")
            self.assertFalse(gs.decide(c, 0.02).passed)
            self.assertEqual(gs.decide(c, 0.2).label, "non-inferior")

    def test_demonstrable_gain_is_better(self):
        base, fam = two_families()
        cur = {k: v * 0.95 for k, v in base.items()}
        c = gs.compare(base, cur, fam)
        self.assertEqual(gs.decide(c, 0.01).label, "better")
        self.assertEqual(gs.decide(c, 0.01, step=0.001).label, "better")

    def test_negligible_consistent_gain_is_not_better_with_a_step(self):
        # Every icon a hair lower: a degenerate interval just below zero.
        base, fam = two_families()
        cur = {k: v - 1e-6 for k, v in base.items()}
        c = gs.compare(base, cur, fam)
        self.assertLess(c.p95, 0.0)
        self.assertGreater(c.p95, -0.001)
        self.assertEqual(gs.decide(c, 0.02).label, "better")
        self.assertEqual(gs.decide(c, 0.02, step=0.001).label, "non-inferior")

    def test_noisy_null_change_passes_within_noise_at_a_tight_margin(self):
        # A broad edit with no true effect: the interval straddles a 0.5 % margin, which the
        # set cannot resolve, but the upper bound stays below the minimum detectable effect.
        base, fam = two_families(200, 50)
        rng = np.random.default_rng(7)
        cur = {k: v * float(np.exp(rng.normal(0, 0.3))) for k, v in base.items()}
        c = gs.compare(base, cur, fam)
        self.assertLess(c.p05, 0.0)
        self.assertGreater(c.p95, 0.005)
        self.assertLess(c.p95, c.mde)
        v = gs.decide(c, 0.005)
        self.assertEqual(v.label, "within-noise")
        self.assertTrue(v.passed)
        self.assertAlmostEqual(v.effective, c.mde)
        self.assertEqual(gs.decide(c, 1.0).label, "non-inferior")

    def test_broad_regression_past_the_detectable_effect_still_fails(self):
        # The same noise plus a true 15 % rise: the floor must not let a real regression pass.
        base, fam = two_families(200, 50)
        rng = np.random.default_rng(7)
        cur = {k: v * 1.15 * float(np.exp(rng.normal(0, 0.3))) for k, v in base.items()}
        c = gs.compare(base, cur, fam)
        self.assertGreater(c.p95, c.mde)
        v = gs.decide(c, 0.005)
        self.assertEqual(v.label, "worse")
        self.assertFalse(v.passed)

    def test_low_variance_change_keeps_the_strict_margin(self):
        # Every icon worsens by 2 % with little spread: the set resolves this easily (an MDE
        # well under 1 %), so the floor does not apply and the 1 % margin fails it.
        base, fam = two_families()
        rng = np.random.default_rng(5)
        cur = {k: v * 1.02 * float(np.exp(rng.normal(0, 0.002))) for k, v in base.items()}
        c = gs.compare(base, cur, fam)
        self.assertLess(c.mde, 0.01)
        v = gs.decide(c, 0.01)
        self.assertEqual(v.label, "worse")
        self.assertFalse(v.passed)
        self.assertAlmostEqual(v.effective, 0.01)
        self.assertAlmostEqual(c.mde, (gs.Z_95 + gs.Z_80) * c.se)
        self.assertLess(c.p025, c.p05)
        self.assertLess(c.p95, c.p975)

    def test_seeded_and_order_independent(self):
        base, fam = two_families()
        rng = np.random.default_rng(3)
        cur = {k: v + float(rng.normal(0, 0.02)) for k, v in base.items()}
        c1 = gs.compare(base, cur, fam)
        shuffled = dict(reversed(list(cur.items())))
        c2 = gs.compare(dict(reversed(list(base.items()))), shuffled, fam)
        self.assertEqual(c1, c2)

    def test_singleton_family_resamples_to_itself(self):
        # Family b is unchanged and flat, so its resampled mean is always 1.5; family a is
        # one icon, so it always resamples to itself: every replicate is the point estimate.
        base = {"a/0": 1.0, "b/0": 1.5, "b/1": 1.5}
        fam = {k: k[0] for k in base}
        cur = dict(base, **{"a/0": 1.5})
        c = gs.compare(base, cur, fam)
        self.assertAlmostEqual(c.rel, (1.5 + 1.5) / (1.0 + 1.5) - 1)
        self.assertAlmostEqual(c.p05, c.rel)
        self.assertAlmostEqual(c.p95, c.rel)

    def test_unpaired_keys_are_ignored_and_empty_pairing_raises(self):
        base = {"a/0": 1.0, "a/1": 2.0}
        cur = {"a/1": 2.0, "a/2": 9.0}
        c = gs.compare(base, cur, {"a/0": "a", "a/1": "a", "a/2": "a"})
        self.assertEqual((c.n, c.changed), (1, 0))
        with self.assertRaises(ValueError):
            gs.compare({"a/0": 1.0}, {"a/1": 1.0}, {"a/0": "a", "a/1": "a"})

    def test_zero_baseline(self):
        fam = {"a/0": "a", "a/1": "a"}
        c = gs.compare({"a/0": 0.0, "a/1": 0.0}, {"a/0": 0.0, "a/1": 1.0}, fam)
        self.assertTrue(math.isinf(c.rel))
        self.assertFalse(gs.decide(c, 0.05).passed)


if __name__ == "__main__":
    unittest.main()
