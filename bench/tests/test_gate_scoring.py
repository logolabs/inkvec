"""The scorer must be a pure function of its inputs: same SVGs, same numbers, whatever the
state of the on-disk caches. Needs numpy, Pillow and resvg-py; no Rust build."""
from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np

BENCH = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BENCH))

import svgeval  # noqa: E402

# A gradient and an anti-aliased circle: a render with many values that are not multiples
# of 1/255, which is exactly where a float and an 8-bit reference disagree.
GT_SVG = (
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24">'
    '<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">'
    '<stop offset="0" stop-color="#ff2a00"/><stop offset="1" stop-color="#0033cc"/>'
    "</linearGradient></defs>"
    '<rect x="1" y="1" width="22" height="22" fill="url(#g)"/>'
    '<circle cx="12" cy="12" r="6.3" fill="#20c060" fill-opacity="0.7"/></svg>'
)


class GtRenderTests(unittest.TestCase):
    def test_cold_and_warm_cache_return_identical_arrays(self):
        with tempfile.TemporaryDirectory() as d:
            gt = Path(d) / "x.svg"
            gt.write_text(GT_SVG, encoding="utf-8")
            with patch.object(svgeval, "CACHE", Path(d) / "cache"), \
                    patch.object(svgeval, "JUDGE_SIZE", 64):
                cold = svgeval.gt_render(gt, "fam", "x")
                cached = Path(d) / "cache" / "gt64" / "fam" / "x.png"
                self.assertTrue(cached.exists())
                warm = svgeval.gt_render(gt, "fam", "x")
                # No temporary file left behind by the atomic write.
                self.assertEqual(sorted(p.name for p in cached.parent.iterdir()), ["x.png"])
        self.assertEqual(cold.dtype, np.float32)
        self.assertEqual(warm.dtype, np.float32)
        self.assertEqual(cold.shape, (64, 64, 3))
        self.assertTrue(np.array_equal(cold, warm), "cold and warm reference renders differ")
        # The reference is the 8-bit image: every value is k/255 exactly.
        k = cold.astype(np.float64) * 255
        self.assertTrue(np.allclose(k, np.round(k), atol=1e-3))
        # ... and not a trivially flat one: the gradient leaves many levels.
        self.assertGreater(len(np.unique(cold)), 20)

    def test_quantisation_rounds_half_up_and_clips(self):
        x = np.array([[[-0.1, 0.0, 0.5 / 255]], [[1.0, 1.2, 254.5 / 255]]], dtype=np.float32)
        self.assertEqual(svgeval._rgb8(x).tolist(), [[[0, 0, 1]], [[255, 255, 255]]])

    def test_score_cache_key_carries_the_scorer_version(self):
        with tempfile.TemporaryDirectory() as d:
            exe = Path(d) / "fake.exe"
            exe.write_bytes(b"not a tracer")
            with patch.object(svgeval, "CACHE", Path(d)):
                a = svgeval._cache_path(exe, ())
                with patch.object(svgeval, "SCORER_VERSION", svgeval.SCORER_VERSION + 1):
                    b = svgeval._cache_path(exe, ())
        self.assertNotEqual(a, b)


if __name__ == "__main__":
    unittest.main()
