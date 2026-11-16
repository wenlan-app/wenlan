#!/usr/bin/env python3
"""Decide whether the macOS and Windows lanes run for one CI event.

Hosted macOS runs at most five jobs at once, and the merge queue kicks an
entry whose macOS job waits too long for a runner. Ordinary PR pushes already
skip macOS. In the merge queue:

- macOS runs only for macOS-specific changes: the desktop app and updater,
  packaging and release scripts, native C/ObjC sources, build.rs, the
  toolchain, and Rust files with `target_os = "macos"` code. Shared Rust and
  dependency bumps are proved on Linux (and Windows) before merge and on macOS
  by the main push after merge.
- Windows runs when a file that woke its filters can change native behavior.
  The React frontend and its browser tooling, and this workflow file itself,
  never do.

The main push after merge keeps every native job as the backstop.

Every other event keeps the lane: main push, manual runs and release-please.

`darwin-preflight` is the darwin release preflight alone. Main push does not
run it, so a queue entry that edits ci.yml keeps it: that edit has no other
pre-release run of the darwin release leg.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

# Paths that never need a macOS or Windows runner. They are covered by the
# Linux frontend, browser e2e, web build and workflow-contract jobs; the main
# push still runs the native lanes for them after merge. tooling/ is NOT here:
# its Vite configs decide where the Tauri and Review bundles are written.
NATIVE_IRRELEVANT_DIRS = (
    "src/",
    "e2e/",
    "preview/",
    "review/",
    "public/",
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


# macOS-specific paths. In the merge queue the macOS lane runs only for these;
# shared Rust, dependency bumps and every other macOS-filter owner wait for the
# main push, which runs the full macOS suite after merge.
MACOS_SPECIFIC_DIRS = ("app/", "vendor/tauri-plugin-updater/")
MACOS_SPECIFIC_FILES = frozenset(
    {
        ".github/workflows/release.yml",
        "install.sh",
        "rust-toolchain.toml",
        "scripts/prepare-sidecars.sh",
        "scripts/prepare-tauri-build-sidecars.sh",
    }
)
NATIVE_SOURCE_SUFFIXES = (".c", ".cc", ".cpp", ".cxx", ".h", ".hh", ".hpp", ".hxx", ".m", ".mm")
MACOS_CFG = re.compile(r'target_os\s*=\s*"macos"|target_vendor\s*=\s*"apple"')


def macos_specific(path: str, root: Path) -> bool:
    if path in MACOS_SPECIFIC_FILES or path.startswith(MACOS_SPECIFIC_DIRS):
        return True
    name = path.rsplit("/", 1)[-1]
    if path.startswith("scripts/") and (
        "macos" in name
        or name.startswith("build-release-binaries")
        or name.startswith("release_targets")
    ):
        return True
    if path.startswith("crates/") and (
        name == "build.rs" or name.endswith(NATIVE_SOURCE_SUFFIXES) or "macos" in name
    ):
        return True
    if path.endswith(".rs"):
        try:
            text = (root / path).read_text(encoding="utf-8", errors="replace")
        except FileNotFoundError:
            # A deleted Rust file may have held macOS code; fail toward running.
            return True
        return MACOS_CFG.search(text) is not None
    return False


def lane_allowed(
    *,
    platform: str,
    event_name: str,
    head_ref: str,
    files: list[str],
    root: Path | None = None,
) -> bool:
    if platform not in ("macos", "windows", "darwin-preflight"):
        raise LaneError(f"unknown platform {platform!r}")
    if event_name == "pull_request":
        if head_ref.startswith("release-please--branches--"):
            return True
        # Ordinary PR pushes leave macOS to the merge queue and keep Windows.
        return platform == "windows"
    if event_name == "merge_group":
        if platform == "darwin-preflight" and ".github/workflows/ci.yml" in files:
            return True
        if platform == "macos":
            base = root if root is not None else Path.cwd()
            return any(macos_specific(path, base) for path in files)
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
    parser.add_argument("--repo-root", default=".")
    args = parser.parse_args(argv)
    try:
        files = _load_files(args.files_json)
        allowed = lane_allowed(
            platform=args.platform,
            event_name=args.event_name,
            head_ref=args.head_ref,
            files=files,
            root=Path(args.repo_root),
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
