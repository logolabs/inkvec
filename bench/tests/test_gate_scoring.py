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


class OpaqueTierTests(unittest.TestCase):
    def test_base_tier(self):
        self.assertEqual(svgeval.base_tier("512ssop"), "512ss")
        self.assertEqual(svgeval.base_tier("128ss"), "128ss")
        self.assertEqual(svgeval.base_tier("op"), "op")

    def test_flatten_onto_white(self):
        from PIL import Image
        rgba = np.array([[[255, 0, 0, 255], [255, 0, 0, 0]],
                         [[0, 0, 255, 128], [10, 20, 30, 255]]], dtype=np.uint8)
        with tempfile.TemporaryDirectory() as d:
            src, dst = Path(d) / "a.png", Path(d) / "out" / "a.png"
            Image.fromarray(rgba, "RGBA").save(src)
            svgeval.flatten_onto_white(src, dst)
            img = Image.open(dst)
            self.assertEqual(img.mode, "RGB")
            got = np.asarray(img).tolist()
        # Half-covered blue over white: 255 * (1 - 128/255) + 0.5 -> 127 in red and green.
        self.assertEqual(got, [[[255, 0, 0], [255, 255, 255]], [[127, 127, 255], [10, 20, 30]]])

    def test_environment_tier_survives_load_sets(self):
        saved = (svgeval.TIER, svgeval._TIER_RESOLVED)
        try:
            with patch.dict(svgeval.os.environ, {"INKVEC_TIER": "512ss"}):
                svgeval.load_sets()
                self.assertEqual(svgeval.tier(), "512ss")
        finally:
            svgeval.TIER, svgeval._TIER_RESOLVED = saved

    def test_item_paths_derives_the_opaque_raster_once(self):
        it = svgeval.load_sets()["screen"][0]
        saved = (svgeval.TIER, svgeval._TIER_RESOLVED, svgeval.os.environ.get("INKVEC_TIER"))
        try:
            with tempfile.TemporaryDirectory() as d, patch.object(svgeval, "CACHE", Path(d)):
                svgeval.set_tier("128ssop")
                png, gt = svgeval.item_paths(it)
                self.assertTrue(png.exists() and gt.exists())
                self.assertTrue(str(png).startswith(d))
                self.assertEqual(svgeval.os.environ["INKVEC_TIER"], "128ssop")
        finally:
            svgeval.TIER, svgeval._TIER_RESOLVED = saved[0], saved[1]
            if saved[2] is None:
                svgeval.os.environ.pop("INKVEC_TIER", None)
            else:
                svgeval.os.environ["INKVEC_TIER"] = saved[2]


if __name__ == "__main__":
    unittest.main()
