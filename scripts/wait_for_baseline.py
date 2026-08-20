#!/usr/bin/env python3
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
"""Wait for a successful GitHub Actions artifact at one exact commit."""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from urllib.parse import quote, urlencode
from urllib.request import Request, urlopen


def get_json(url: str, token: str) -> dict:
    request = Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "User-Agent": "r-environment-tools-quality-snapshots",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    with urlopen(request, timeout=30) as response:
        value = json.load(response)
    if not isinstance(value, dict):
        raise RuntimeError("GitHub API response must be an object")
    return value


def find_artifact(repository: str, workflow: str, commit: str, artifact: str, token: str) -> bool:
    repo = "/".join(quote(part, safe="") for part in repository.split("/"))
    if repository.count("/") != 1:
        raise RuntimeError("repository must have owner/name form")
    query = urlencode({"event": "push", "head_sha": commit, "per_page": 100})
    runs_url = f"https://api.github.com/repos/{repo}/actions/workflows/{quote(workflow, safe='')}/runs?{query}"
    runs = get_json(runs_url, token).get("workflow_runs")
    if not isinstance(runs, list):
        raise RuntimeError("workflow_runs response is missing an array")
    matching = [run for run in runs if run.get("head_sha") == commit and run.get("event") == "push"]
    matching.sort(key=lambda run: (run.get("run_number", 0), run.get("run_attempt", 0)), reverse=True)
    if not matching:
        return False
    run = matching[0]
    if run.get("status") != "completed":
        return False
    if run.get("conclusion") != "success":
        raise RuntimeError(f"exact-base workflow concluded with {run.get('conclusion')!r}")
    artifacts_url = f"https://api.github.com/repos/{repo}/actions/runs/{run['id']}/artifacts?per_page=100"
    artifacts = get_json(artifacts_url, token).get("artifacts")
    if not isinstance(artifacts, list):
        raise RuntimeError("artifacts response is missing an array")
    return any(item.get("name") == artifact and item.get("expired") is False for item in artifacts)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", required=True)
    parser.add_argument("--workflow", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--artifact", required=True)
    parser.add_argument("--timeout-seconds", type=int, default=1200)
    parser.add_argument("--poll-seconds", type=int, default=20)
    args = parser.parse_args()
    token = os.environ.get("GITHUB_TOKEN", "")
    if not token:
        print("baseline error: GITHUB_TOKEN is required", file=sys.stderr)
        return 2
    deadline = time.monotonic() + args.timeout_seconds
    while True:
        try:
            if find_artifact(args.repository, args.workflow, args.commit, args.artifact, token):
                print(f"Found {args.artifact} for exact base {args.commit}")
                return 0
        except Exception as error:
            print(f"baseline error: {error}", file=sys.stderr)
            return 2
        if time.monotonic() >= deadline:
            print(f"baseline error: timed out waiting for {args.artifact} at {args.commit}", file=sys.stderr)
            return 1
        time.sleep(args.poll_seconds)


if __name__ == "__main__":
    raise SystemExit(main())
