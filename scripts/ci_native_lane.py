#!/usr/bin/env python3
"""Decide whether the macOS and Windows lanes run for one CI event.

Hosted macOS runs at most five jobs at once, and the merge queue kicks an
entry whose macOS job waits too long for a runner. Ordinary PR pushes already
skip macOS. In the merge queue, a native lane runs only when a file that woke
one of its filters can change native behavior. Files that never do (the React
frontend and its browser tooling, and this workflow file itself) are dropped
first. The PR run already exercised Windows on the same diff, and the main
push after merge keeps every native job as the backstop for both platforms.

Every other event keeps the lane: main push, manual runs and release-please.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

# Paths that never need a macOS or Windows runner. They are covered by the
# Linux frontend, browser e2e and workflow-contract jobs; the main push still
# runs the native lanes for them after merge.
NATIVE_IRRELEVANT_DIRS = (
    "src/",
    "e2e/",
    "preview/",
    "review/",
    "public/",
    "tooling/",
)
NATIVE_IRRELEVANT_FILES = frozenset(
    {
        ".github/workflows/ci.yml",
        "index.html",
        "vite.config.ts",
        "vitest.config.ts",
        "playwright.config.ts",
        "tsconfig.json",
        "tsconfig.tools.json",
    }
)


class LaneError(Exception):
    pass


def native_relevant(path: str) -> bool:
    if path in NATIVE_IRRELEVANT_FILES:
        return False
    return not path.startswith(NATIVE_IRRELEVANT_DIRS)


def lane_allowed(
    *, platform: str, event_name: str, head_ref: str, files: list[str]
) -> bool:
    if platform not in ("macos", "windows"):
        raise LaneError(f"unknown platform {platform!r}")
    if event_name == "pull_request":
        if head_ref.startswith("release-please--branches--"):
            return True
        # Ordinary PR pushes leave macOS to the merge queue and keep Windows.
        return platform == "windows"
    if event_name == "merge_group":
        return any(native_relevant(path) for path in files)
    if event_name in ("push", "workflow_dispatch"):
        return True
    raise LaneError(f"unsupported event {event_name!r}")


def _load_files(raw_values: list[str]) -> list[str]:
    files: set[str] = set()
    for raw in raw_values:
        value = json.loads(raw or "[]")
        if not isinstance(value, list) or not all(isinstance(p, str) for p in value):
            raise LaneError("each --files-json must be a JSON list of paths")
        files.update(value)
    return sorted(files)


def _main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--event-name", required=True)
    parser.add_argument("--head-ref", default="")
    parser.add_argument("--files-json", action="append", default=[])
    parser.add_argument("--github-output")
    parser.add_argument("--output-name", default="allowed")
    args = parser.parse_args(argv)
    try:
        files = _load_files(args.files_json)
        allowed = lane_allowed(
            platform=args.platform,
            event_name=args.event_name,
            head_ref=args.head_ref,
            files=files,
        )
    except (LaneError, json.JSONDecodeError) as error:
        print(f"ci_native_lane: {error}", file=sys.stderr)
        return 1
    if re.fullmatch(r"[A-Za-z0-9_-]+", args.output_name) is None:
        print("ci_native_lane: bad output name", file=sys.stderr)
        return 1
    line = f"{args.output_name}={'true' if allowed else 'false'}"
    relevant = [path for path in files if native_relevant(path)]
    print(f"{args.platform} lane on {args.event_name}: {line}")
    if args.event_name == "merge_group":
        print(f"native-relevant files: {relevant or 'none'}")
    if args.github_output:
        with Path(args.github_output).open("a", encoding="utf-8") as output:
            output.write(line + "\n")
    else:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(_main(sys.argv[1:]))
