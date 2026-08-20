# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from quality_snapshot import SnapshotError, compare_coverage, parse_lcov  # noqa: E402


class CoverageSnapshotTests(unittest.TestCase):
    def write_lcov(self, path: Path, *, lh: int, lf: int, fnh: int, fnf: int) -> None:
        path.write_text(
            f"SF:example.rs\nLF:{lf}\nLH:{lh}\nFNF:{fnf}\nFNH:{fnh}\nend_of_record\n",
            encoding="utf-8",
        )

    def test_line_and_function_regressions_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            current_path = Path(directory) / "current.info"
            baseline_path = Path(directory) / "baseline.info"
            self.write_lcov(current_path, lh=80, lf=100, fnh=7, fnf=10)
            self.write_lcov(baseline_path, lh=90, lf=100, fnh=8, fnf=10)
            failures = compare_coverage(parse_lcov(current_path), parse_lcov(baseline_path))
            self.assertEqual(len(failures), 2)

    def test_incomplete_snapshot_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "coverage.info"
            path.write_text("LF:10\nLH:9\n", encoding="utf-8")
            with self.assertRaises(SnapshotError):
                parse_lcov(path)

    def test_boundary_budget_passes(self):
        with tempfile.TemporaryDirectory() as directory:
            current_path = Path(directory) / "current.info"
            baseline_path = Path(directory) / "baseline.info"
            self.write_lcov(current_path, lh=9999, lf=10000, fnh=9999, fnf=10000)
            self.write_lcov(baseline_path, lh=10000, lf=10000, fnh=10000, fnf=10000)
            self.assertEqual(
                compare_coverage(parse_lcov(current_path), parse_lcov(baseline_path)), []
            )


if __name__ == "__main__":
    unittest.main()
