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


class FastPathTests(unittest.TestCase):
    """The scorer's shortcuts give the numbers of the plain computations they replace."""

    def test_codes_match_the_arithmetic(self):
        from inkvec_bench import geomatch as gm
        rng = np.random.default_rng(3)
        rgba = rng.integers(0, 256, (37, 41, 4), dtype=np.uint8)
        rgba[::3, :, 3] = 255
        rgba[1::3, :, 3] = 0
        for opaque in (False, True):
            u = rgba.astype(np.uint32)
            if opaque:
                a = u[..., 3:4]
                u[..., :3] = (u[..., :3] * a + 255 * (255 - a) + 127) // 255
                u[..., 3] = 255
            want = (u[..., 0] << 24) | (u[..., 1] << 16) | (u[..., 2] << 8) | u[..., 3]
            got = gm._codes(rgba, opaque)
            self.assertEqual(got.dtype, want.dtype)
            self.assertTrue(np.array_equal(got, want), opaque)

    def test_labels_by_runs_match_unique(self):
        from inkvec_bench import geomatch as gm
        rng = np.random.default_rng(4)
        codes = rng.choice(np.array([0xFF0000FF, 0x00FF00FF, 0x123456FF, 0xFFFFFF00], np.uint32),
                           size=(64, 50)).astype(np.uint32)
        codes[10:30, :] = 0x00FF00FF                    # long runs, as a flat render has
        uniq, counts = np.unique(codes.reshape(-1), return_counts=True)
        pal = gm._palette(gm._features(uniq), counts)[:3]
        _, inv = np.unique(codes.reshape(-1), return_inverse=True)
        d = ((gm._features(uniq)[:, None, :] - pal[None, :, :]) ** 2).sum(-1)
        want = d.argmin(1)[inv.reshape(-1)].reshape(codes.shape)
        self.assertTrue(np.array_equal(gm._labels(codes, pal), want))

    def test_artist_side_cache_hit_equals_miss(self):
        from inkvec_bench import geomatch as gm
        art = ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">'
               '<circle cx="12" cy="12" r="7" fill="#d22f27" fill-opacity="0.6"/>'
               '<rect x="3" y="3" width="6" height="4" fill="#123456"/></svg>')
        ours = art.replace('r="7"', 'r="7.3"')
        saved = gm.CACHE_DIR
        try:
            gm.CACHE_DIR = None
            plain = gm.geomatch(ours, art, 64, True)
            with tempfile.TemporaryDirectory() as d:
                gm.CACHE_DIR = Path(d)
                miss = gm.geomatch(ours, art, 64, True)
                self.assertEqual(len(list(Path(d).glob("*.npz"))), 1)
                hit = gm.geomatch(ours, art, 64, True)
                other = gm.geomatch(ours, art, 64, False)     # another page: another entry
                self.assertEqual(len(list(Path(d).glob("*.npz"))), 2)
        finally:
            gm.CACHE_DIR = saved
        self.assertEqual(plain, miss)
        self.assertEqual(plain, hit)
        self.assertGreater(plain["geom"], 0.0)
        self.assertNotEqual(other, {})

    def test_delta_e00_matches_the_whole_sample(self):
        from inkvec_bench.metrics import color
        from skimage.color import deltaE_ciede2000, rgb2lab
        rng = np.random.default_rng(5)
        a = (rng.integers(0, 256, (600, 700, 3)) / 255).astype(np.float32)
        b = a.copy()
        b[:40] = (rng.integers(0, 256, (40, 700, 3)) / 255).astype(np.float32)
        for x, y in ((a, b), (a, a), (a.astype(np.float64), b.astype(np.float64))):
            xs, ys = np.clip(x, 0, 1).reshape(-1, 3), np.clip(y, 0, 1).reshape(-1, 3)
            idx = np.random.default_rng(0).choice(xs.shape[0], color.MAX_DELTA_E_SAMPLES, replace=False)
            de = deltaE_ciede2000(rgb2lab(xs[idx].reshape(-1, 1, 3)),
                                  rgb2lab(ys[idx].reshape(-1, 1, 3))).reshape(-1)
            got = color.delta_e00(x, y)
            self.assertEqual(got["de00_mean"], float(de.mean()))
            self.assertEqual(got["de00_p95"], float(np.percentile(de, 95)))
            self.assertEqual(got["de00_max"], float(de.max()))


@unittest.skipIf(sys.platform == "win32", "the stand-in tracer is a shell script")
class ReuseTests(unittest.TestCase):
    """An icon whose SVG bytes match the baseline's is traced but not scored again."""

    def test_byte_identical_svg_takes_the_known_numbers(self):
        import hashlib
        import os
        it = svgeval.load_sets()["screen"][0]
        saved = (svgeval.TIER, svgeval._TIER_RESOLVED, os.environ.get("INKVEC_TIER"))
        self.addCleanup(self._restore_tier, saved)
        with tempfile.TemporaryDirectory() as d:
            out = Path(d) / "out.svg"
            out.write_text(GT_SVG, encoding="utf-8")
            exe = Path(d) / "tracer.sh"
            # A stand-in tracer: copies the fixed SVG to the `-o` path.
            exe.write_text(f'#!/bin/sh\ncp "{out}" "$3"\n', encoding="utf-8")
            os.chmod(exe, 0o755)
            sha = hashlib.sha256(out.read_bytes()).hexdigest()
            known = {ax: 0.5 for ax in ("de00", "turning", "ratio", "self_res", "geom", "geom_far")}
            job = svgeval.Job(exe, it, Path(d), (), False, "128ss", (sha[:16], known), True)
            res = svgeval.score_one(job)
            self.assertTrue(res.get("reused"), res)
            self.assertEqual(res["de00"], 0.5)
            self.assertEqual(res["sha256"], sha)
            self.assertIn("div", res["human"])            # the battery still runs
            self.assertEqual(res["cpu"]["score"] < 0.5, True)
            stale = svgeval.Job(exe, it, Path(d), (), False, "128ss", ("0" * 16, known), False)
            with patch.dict(os.environ, {"INKVEC_SKIP_DISTS": "1"}):
                fresh = svgeval.score_one(stale)
            self.assertFalse(fresh.get("reused", False))
            self.assertNotEqual(fresh["de00"], 0.5)

    @staticmethod
    def _restore_tier(saved):
        import os
        svgeval.TIER, svgeval._TIER_RESOLVED = saved[0], saved[1]
        if saved[2] is None:
            os.environ.pop("INKVEC_TIER", None)
        else:
            os.environ["INKVEC_TIER"] = saved[2]


if __name__ == "__main__":
    unittest.main()
