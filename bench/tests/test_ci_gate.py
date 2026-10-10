"""The regression gate's control flow, with tracing replaced by synthetic scores: legacy
mode, writing a baseline, the paired verdicts, and the rule that a byte-identical icon
keeps the baseline's numbers. numpy only; no Rust build, no renders."""
from __future__ import annotations

import hashlib
import io
import json
import re
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import ci_gate  # noqa: E402

ITEMS = [{"corpus": f, "stem": f"s{i}"} for f in ("lucide", "noto-emoji") for i in range(12)]


def rows(scale: float = 1.0, changed: tuple[str, ...] | None = None) -> dict:
    """Synthetic per-icon scores; icons in `changed` (default: all) get scaled values and a
    different SVG hash."""
    out = {}
    for n, it in enumerate(ITEMS):
        k = f"{it['corpus']}/{it['stem']}"
        hit = changed is None or k in changed
        s = scale if hit else 1.0
        digest = hashlib.sha256(f"{k}-{s if hit else 1.0}".encode()).hexdigest()
        out[k] = {"corpus": it["corpus"], "de00": 0.1 * (1 + n % 5) * s, "turning": 0.04 * s,
                  "ratio": 1.2 * s, "self_res": 0.003, "geom": 0.02 * s, "geom_far": 0.001 * s,
                  "sha256": digest}
    return out


class Run:
    """ci_gate.main() with `score_condition` answering from `scores[condition]`."""

    def __init__(self, tmp: Path, scores: dict):
        self.tmp, self.scores = tmp, scores
        self.exe = tmp / "inkvec.exe"
        self.exe.write_bytes(b"fake")

    def __call__(self, *args: str) -> tuple[int, str]:
        buf = io.StringIO()
        fake = lambda exe, cond, items, workers: (self.scores[cond.name], [])  # noqa: E731
        with patch.object(ci_gate, "score_condition", side_effect=fake), \
                patch.object(ci_gate.svgeval, "load_sets", return_value={"screen": ITEMS}), \
                patch.object(ci_gate, "BASELINE", self.tmp / "legacy.json"), \
                patch.object(ci_gate, "BASELINE_DIR", self.tmp / "baselines"), \
                patch.object(ci_gate, "command_output", return_value=None), \
                patch.dict(ci_gate.os.environ, {"CI": "", "GITHUB_STEP_SUMMARY": ""}), \
                patch.object(sys, "argv", ["ci_gate.py", "--exe", str(self.exe), *args]), \
                redirect_stdout(buf):
            ci_gate.os.environ.pop("GITHUB_ACTIONS", None)
            code = ci_gate.main()
        return code, buf.getvalue()


class GateFlowTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def baseline_path(self) -> Path:
        return self.tmp / "baselines" / f"{ci_gate.platform_tag()}.json"

    def test_platform_tag_shape(self):
        self.assertRegex(ci_gate.platform_tag(), r"^(linux|windows|macos|[a-z0-9]+)-[a-z0-9_]+$")
        self.assertNotIn("amd64", ci_gate.platform_tag())

    def test_pack_round_trip(self):
        r = rows()
        back = ci_gate.unpack_rows(json.loads(ci_gate.dumps({"icons": ci_gate.pack_rows(r)}))["icons"])
        self.assertEqual(set(back), set(r))
        for k in r:
            self.assertEqual(back[k]["corpus"], r[k]["corpus"])
            self.assertEqual(back[k]["sha256"], r[k]["sha256"][:ci_gate.SHA_CHARS])
            for ax in ci_gate.REPORTED_AXES:
                self.assertAlmostEqual(back[k][ax], r[k][ax], delta=abs(r[k][ax]) * 1e-8)

    def test_write_then_identical_build_passes(self):
        run = Run(self.tmp, {c.name: rows() for c in ci_gate.CONDITIONS})
        code, out = run("--write-baseline")
        self.assertEqual(code, 0, out)
        doc = json.loads(self.baseline_path().read_text(encoding="utf-8"))
        self.assertEqual(set(doc["conditions"]), {c.name for c in ci_gate.CONDITIONS})
        self.assertEqual(doc["scorer_version"], ci_gate.svgeval.SCORER_VERSION)
        code, out = run()
        self.assertEqual(code, 0, out)
        # Every gated axis of every condition reads "identical"; the others "reported".
        self.assertEqual(len(re.findall(r"\bidentical\b", out)),
                         len(ci_gate.GATED_AXES) * len(ci_gate.CONDITIONS))

    def test_regression_fails_and_bypass_needs_a_reason(self):
        run = Run(self.tmp, {c.name: rows() for c in ci_gate.CONDITIONS})
        run("--write-baseline")
        run.scores["fast-512ss"] = rows(1.10)
        code, out = run()
        self.assertEqual(code, 1)
        self.assertIn("fast-512ss de00: worse", out)
        self.assertNotIn("quality-128ss de00: worse", out)
        self.assertEqual(run("--bypass-gate", "short")[0], 1)
        self.assertEqual(run("--bypass-gate", "user approved: intended trade-off")[0], 0)

    def test_byte_identical_icons_keep_the_baseline_numbers(self):
        run = Run(self.tmp, {c.name: rows() for c in ci_gate.CONDITIONS})
        run("--write-baseline")
        noisy = rows()
        for r in noisy.values():
            r["de00"] += 1e-9          # scorer float noise, same SVG bytes
        run.scores["quality-512ss"] = noisy
        code, out = run("--conditions", "quality-512ss")
        self.assertEqual(code, 0, out)
        self.assertRegex(out, r"quality-512ss\s+de00 .* identical")

    def test_gain_tightens_locally_but_never_with_no_tighten(self):
        run = Run(self.tmp, {c.name: rows() for c in ci_gate.CONDITIONS})
        run("--write-baseline")
        before = self.baseline_path().read_bytes()
        run.scores = {c.name: rows(0.9) for c in ci_gate.CONDITIONS}
        code, out = run("--no-tighten")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.baseline_path().read_bytes(), before)
        code, out = run()
        self.assertEqual(code, 0, out)
        self.assertNotEqual(self.baseline_path().read_bytes(), before)

    def test_legacy_mode_uses_the_scalar_baseline(self):
        (self.tmp / "legacy.json").write_text(json.dumps(
            {"de00": 0.3, "turning": 0.04, "ratio": 1.2, "self_res": 0.003, "n": 24}), encoding="utf-8")
        run = Run(self.tmp, {c.name: rows() for c in ci_gate.CONDITIONS})
        code, out = run("--no-tighten", "--artifact-dir", str(self.tmp / "art"))
        self.assertEqual(code, 0, out)
        self.assertIn("legacy rule", out)
        self.assertTrue((self.tmp / "art" / f"{ci_gate.platform_tag()}.json").exists())
        run.scores["quality-128ss"] = rows(1.02)       # +2 % > the legacy 1 %
        code, out = run("--no-tighten")
        self.assertEqual(code, 1)
        self.assertIn("WORSE", out)

    def test_stale_scorer_version_refuses_to_compare(self):
        run = Run(self.tmp, {c.name: rows() for c in ci_gate.CONDITIONS})
        run("--write-baseline")
        doc = json.loads(self.baseline_path().read_text(encoding="utf-8"))
        doc["scorer_version"] = -1
        self.baseline_path().write_text(json.dumps(doc), encoding="utf-8")
        code, out = run()
        self.assertEqual(code, 1)
        self.assertIn("not comparable", out)


if __name__ == "__main__":
    unittest.main()
