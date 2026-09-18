"""Regression checks for the quality gate itself; no Rust build required."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("quality", Path(__file__).parents[1] / "quality.py")
quality = importlib.util.module_from_spec(spec)
spec.loader.exec_module(quality)


class QualityTests(unittest.TestCase):
    def test_new_lint_category_is_a_regression(self):
        failures, _, tightened = quality.check({"lint:new_warning": 1}, {"test_functions": 147})
        self.assertTrue(failures)
        self.assertNotIn("lint:new_warning", tightened)

    def test_disappearing_lint_retires_its_allowance(self):
        failures, gains, tightened = quality.check(
            {"rustfmt_hunks": 0}, {"lint:missing_docs": 2, "rustfmt_hunks": 0})
        self.assertFalse(failures)
        self.assertTrue(gains)
        self.assertEqual(tightened["lint:missing_docs"], 0)
        self.assertTrue(quality.check({"lint:missing_docs": 1}, tightened)[0])

    def test_fast_measurement_preserves_unmeasured_lints(self):
        _, gains, tightened = quality.check({"test_functions": 147}, {"lint:missing_docs": 2})
        self.assertFalse(gains)
        self.assertEqual(tightened["lint:missing_docs"], 2)

    def test_compiler_failure_is_not_a_measurement(self):
        result = subprocess.CompletedProcess([], 101, "", "compilation failed")
        with patch.object(quality.subprocess, "run", return_value=result):
            with self.assertRaisesRegex(RuntimeError, "compilation failed"):
                quality.lint_counts()

    def test_missing_tool_is_not_a_measurement(self):
        with patch.object(quality.subprocess, "run", side_effect=FileNotFoundError("cargo")):
            with self.assertRaises(RuntimeError):
                quality.cargo("fmt", "--check")

    def test_formatting_diff_is_valid_but_formatter_failure_is_not(self):
        for output, valid in [("Diff in /src/lib.rs:1:\n", True), ("error: rustfmt unavailable", False)]:
            with self.subTest(output=output):
                result = subprocess.CompletedProcess([], 1, output, "")
                with patch.object(quality.subprocess, "run", return_value=result):
                    if valid:
                        self.assertEqual(quality.cargo("fmt", "--check"), output)
                    else:
                        with self.assertRaises(RuntimeError):
                            quality.cargo("fmt", "--check")

    def test_failed_measurement_cannot_rewrite_budget_even_with_update(self):
        with tempfile.TemporaryDirectory() as directory:
            budget = Path(directory) / "budget.json"
            budget.write_text('{"lint:missing_docs": 2}\n', encoding="utf-8")
            before = budget.read_bytes()
            with patch.object(quality, "BUDGET", budget), patch.object(
                quality, "measure", side_effect=RuntimeError("broken compiler")
            ), patch.object(sys, "argv", ["quality.py", "--update"]):
                self.assertEqual(quality.main(), 1)
            self.assertEqual(budget.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
