"""bench/cases.py's ratchet on the set of passing cases: newly failing fails, newly passing
tightens, a missing record fails, renamed cases are visible. No tracer needed."""
from __future__ import annotations

import importlib.util
import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

spec = importlib.util.spec_from_file_location("cases_cli", Path(__file__).parents[1] / "cases.py")
cases_cli = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cases_cli)


def rows(passing: set[str], names=("a", "b", "c", "d")) -> list[dict]:
    return [{"name": n, "pass": n in passing} for n in names]


class RatchetTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.budget = Path(self._tmp.name) / "budget.json"

    def tearDown(self):
        self._tmp.cleanup()

    def run_ratchet(self, r, **kw) -> tuple[int, str]:
        kw.setdefault("update", False)
        kw.setdefault("tighten", True)
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = cases_cli.ratchet("quality", r, budget_path=self.budget, **kw)
        return code, buf.getvalue()

    def recorded(self, key: str = "cases:quality"):
        return json.loads(self.budget.read_text(encoding="utf-8")).get(key)

    def test_missing_record_fails_and_update_records(self):
        self.budget.write_text('{"test_functions": 1}', encoding="utf-8")
        self.assertEqual(self.run_ratchet(rows({"a", "b"}))[0], 1)
        self.assertEqual(self.run_ratchet(rows({"a", "b"}), update=True)[0], 0)
        self.assertEqual(self.recorded(), ["a", "b"])
        self.assertEqual(json.loads(self.budget.read_text())["test_functions"], 1)

    def test_newly_failing_case_fails(self):
        self.budget.write_text('{"cases:quality": ["a", "b"]}', encoding="utf-8")
        code, out = self.run_ratchet(rows({"a", "c"}))
        self.assertEqual(code, 1)
        self.assertIn("- b", out)
        self.assertEqual(self.recorded(), ["a", "b"])

    def test_newly_passing_case_tightens_only_when_allowed(self):
        self.budget.write_text('{"cases:quality": ["a"]}', encoding="utf-8")
        self.assertEqual(self.run_ratchet(rows({"a", "c"}), tighten=False)[0], 0)
        self.assertEqual(self.recorded(), ["a"])
        self.assertEqual(self.run_ratchet(rows({"a", "c"}))[0], 0)
        self.assertEqual(self.recorded(), ["a", "c"])

    def test_retired_case_is_dropped_on_tightening(self):
        self.budget.write_text('{"cases:quality": ["a", "gone"]}', encoding="utf-8")
        code, out = self.run_ratchet(rows({"a"}))
        self.assertEqual(code, 0)
        self.assertIn("retired  gone", out)
        self.assertEqual(self.recorded(), ["a"])

    def test_only_judges_the_cases_that_ran(self):
        self.budget.write_text('{"cases:quality": ["a", "b"]}', encoding="utf-8")
        code, _ = self.run_ratchet(rows({"a"}, names=("a",)), tighten=False)
        self.assertEqual(code, 0)
        self.assertEqual(self.recorded(), ["a", "b"])

    def test_platform_record_overrides_the_shared_one(self):
        own = f"cases:quality:{cases_cli.platform_tag()}"
        self.budget.write_text(json.dumps({"cases:quality": ["a", "b"], own: ["a"]}),
                               encoding="utf-8")
        self.assertEqual(self.run_ratchet(rows({"a"}))[0], 0)
        self.assertEqual(cases_cli.ratchet_key("quality", {}), "cases:quality")


if __name__ == "__main__":
    unittest.main()
