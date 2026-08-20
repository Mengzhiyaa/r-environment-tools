# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from wait_for_baseline import find_artifact  # noqa: E402


class ExactBaselineTests(unittest.TestCase):
    def test_requires_exact_commit_and_non_expired_artifact(self):
        responses = [
            {
                "workflow_runs": [
                    {
                        "id": 2,
                        "head_sha": "moving-main",
                        "event": "push",
                        "status": "completed",
                        "conclusion": "success",
                        "run_number": 20,
                    },
                    {
                        "id": 1,
                        "head_sha": "exact-base",
                        "event": "push",
                        "status": "completed",
                        "conclusion": "success",
                        "run_number": 10,
                    },
                ]
            },
            {"artifacts": [{"name": "coverage-baseline", "expired": False}]},
        ]
        with patch("wait_for_baseline.get_json", side_effect=responses) as request:
            self.assertTrue(
                find_artifact(
                    "owner/repository",
                    "coverage-baseline.yml",
                    "exact-base",
                    "coverage-baseline",
                    "token",
                )
            )
        self.assertIn("/runs/1/artifacts", request.call_args_list[1].args[0])

    def test_expired_artifact_is_not_accepted(self):
        responses = [
            {
                "workflow_runs": [
                    {
                        "id": 1,
                        "head_sha": "exact-base",
                        "event": "push",
                        "status": "completed",
                        "conclusion": "success",
                    }
                ]
            },
            {"artifacts": [{"name": "coverage-baseline", "expired": True}]},
        ]
        with patch("wait_for_baseline.get_json", side_effect=responses):
            self.assertFalse(
                find_artifact(
                    "owner/repository",
                    "coverage-baseline.yml",
                    "exact-base",
                    "coverage-baseline",
                    "token",
                )
            )


if __name__ == "__main__":
    unittest.main()
