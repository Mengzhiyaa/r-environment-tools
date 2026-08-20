#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
"""Compare RET quality snapshots and fail on incomplete input or regression."""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass
from pathlib import Path


COVERAGE_BUDGET_PERCENTAGE_POINTS = 0.01


class SnapshotError(ValueError):
    pass


@dataclass(frozen=True)
class Coverage:
    lines_hit: int
    lines_found: int
    functions_hit: int
    functions_found: int

    @property
    def line_percent(self) -> float:
        return self.lines_hit * 100 / self.lines_found

    @property
    def function_percent(self) -> float:
        return self.functions_hit * 100 / self.functions_found


def parse_lcov(path: Path) -> Coverage:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except FileNotFoundError as error:
        raise SnapshotError(f"Coverage snapshot does not exist: {path}") from error

    totals = {"LF": 0, "LH": 0, "FNF": 0, "FNH": 0}
    seen = set()
    for line in lines:
        key, separator, raw = line.partition(":")
        if separator and key in totals:
            try:
                totals[key] += int(raw)
            except ValueError as error:
                raise SnapshotError(f"Invalid {key} value in {path}: {raw!r}") from error
            seen.add(key)
    missing = sorted(set(totals) - seen)
    if missing:
        raise SnapshotError(f"Coverage snapshot {path} is missing: {', '.join(missing)}")
    if totals["LF"] <= 0 or totals["FNF"] <= 0:
        raise SnapshotError(f"Coverage snapshot {path} has no line or function data")
    if totals["LH"] > totals["LF"] or totals["FNH"] > totals["FNF"]:
        raise SnapshotError(f"Coverage snapshot {path} has hit counts above found counts")
    return Coverage(totals["LH"], totals["LF"], totals["FNH"], totals["FNF"])


def compare_coverage(current: Coverage, baseline: Coverage) -> list[str]:
    failures = []
    line_drop = baseline.line_percent - current.line_percent
    function_drop = baseline.function_percent - current.function_percent
    if line_drop - COVERAGE_BUDGET_PERCENTAGE_POINTS > 1e-9:
        failures.append(f"line coverage dropped by {line_drop:.4f} percentage points")
    if function_drop - COVERAGE_BUDGET_PERCENTAGE_POINTS > 1e-9:
        failures.append(f"function coverage dropped by {function_drop:.4f} percentage points")
    return failures


def coverage_report(current: Coverage, baseline: Coverage, platform: str, failures: list[str]) -> str:
    status = "failed" if failures else "passed"
    rows = [
        f"## Coverage Report ({platform}) — {status}",
        "",
        "| Metric | Current | Exact base | Delta |",
        "| --- | ---: | ---: | ---: |",
        f"| Lines | {current.line_percent:.4f}% | {baseline.line_percent:.4f}% | {current.line_percent - baseline.line_percent:+.4f} pp |",
        f"| Functions | {current.function_percent:.4f}% | {baseline.function_percent:.4f}% | {current.function_percent - baseline.function_percent:+.4f} pp |",
        "",
        f"Regression budget: {COVERAGE_BUDGET_PERCENTAGE_POINTS:.2f} percentage points.",
    ]
    if failures:
        rows.extend(["", "Failures:", *[f"- {failure}" for failure in failures]])
    return "\n".join(rows) + "\n"


def run_coverage(args: argparse.Namespace) -> int:
    current = parse_lcov(args.current)
    baseline = parse_lcov(args.baseline)
    failures = compare_coverage(current, baseline)
    report = coverage_report(current, baseline, args.platform, failures)
    args.report.write_text(report, encoding="utf-8")
    if args.summary:
        with args.summary.open("a", encoding="utf-8") as stream:
            stream.write(report)
    print(report, end="")
    return 1 if failures else 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    coverage = subparsers.add_parser("coverage")
    coverage.add_argument("--current", type=Path, required=True)
    coverage.add_argument("--baseline", type=Path, required=True)
    coverage.add_argument("--platform", required=True)
    coverage.add_argument("--report", type=Path, required=True)
    coverage.add_argument("--summary", type=Path)
    coverage.set_defaults(run=run_coverage)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        return args.run(args)
    except SnapshotError as error:
        print(f"quality snapshot error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
