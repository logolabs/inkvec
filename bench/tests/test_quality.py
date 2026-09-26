"""Regression checks for the quality gate itself; no Rust build required."""
import importlib.util
import json
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

    def test_coverage_floor_holds_and_rises_by_whole_percent(self):
        key = "coverage:inkvec-trace"
        failures, _, _ = quality.check({key: 79.99}, {key: 80})
        self.assertTrue(failures)
        failures, gains, tightened = quality.check({key: 80.97}, {key: 80})
        self.assertFalse(failures)
        self.assertFalse(gains)
        self.assertEqual(tightened[key], 80)
        failures, gains, tightened = quality.check({key: 82.4}, {key: 80})
        self.assertFalse(failures)
        self.assertTrue(gains)
        self.assertEqual(tightened[key], 82)
        self.assertEqual(quality.budget_value(key, 82.4), 82)
        self.assertEqual(quality.budget_value("mutation:crates/x.rs", 41.2), 41)
        self.assertEqual(quality.budget_value("test_functions", 437), 437)

    def test_unmeasured_floors_are_kept(self):
        _, gains, tightened = quality.check({"test_functions": 500}, {"coverage:inkvec-fit": 86})
        self.assertEqual(tightened["coverage:inkvec-fit"], 86)

    def test_new_unused_dependency_is_a_regression(self):
        failures, _, _ = quality.check({"unused_dependencies": ["inkvec-server: bytes"]},
                                       {"unused_dependencies": []})
        self.assertTrue(failures)

    def test_machete_output_is_parsed_and_its_failure_is_not_zero(self):
        out = ("Analyzing dependencies of crates in this directory...\n"
               "cargo-machete found the following unused dependencies in this directory:\n"
               "inkvec-server -- .\\crates\\inkvec-server\\Cargo.toml:\n\tbytes\n\tserde\n"
               "\nDone!\n")
        with patch.object(quality.subprocess, "run",
                          return_value=subprocess.CompletedProcess([], 1, out, "")):
            self.assertEqual(quality.unused_dependencies(),
                             ["inkvec-server: bytes", "inkvec-server: serde"])
        clean = "Analyzing dependencies...\ncargo-machete didn't find any unused dependencies\n"
        with patch.object(quality.subprocess, "run",
                          return_value=subprocess.CompletedProcess([], 0, clean, "")):
            self.assertEqual(quality.unused_dependencies(), [])
        for result in (subprocess.CompletedProcess([], 2, "", "error: bad manifest"),
                       subprocess.CompletedProcess([], 1, "cargo-machete found\n", "")):
            with patch.object(quality.subprocess, "run", return_value=result):
                with self.assertRaises(RuntimeError):
                    quality.unused_dependencies()

    def test_mutation_scores_count_timeouts_as_kills_and_skip_unviable(self):
        def outcome(file, summary):
            return {"scenario": {"Mutant": {"file": file}}, "summary": summary}
        f = "crates/inkvec-trace/src/color.rs"
        data = {"outcomes": [{"scenario": "Baseline", "summary": "Success"}]
                + [outcome(f, "CaughtMutant")] * 2 + [outcome(f, "Timeout")]
                + [outcome(f, "MissedMutant")] + [outcome(f, "Unviable")] * 5}
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / "outcomes.json").write_text(json.dumps(data), encoding="utf-8")
            self.assertEqual(quality.mutation_scores(Path(directory)), {f"mutation:{f}": 75.0})
            with self.assertRaises(RuntimeError):
                quality.mutation_scores(Path(directory) / "missing")

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
