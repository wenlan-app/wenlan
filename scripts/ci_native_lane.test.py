#!/usr/bin/env python3

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

from ci_native_lane import LaneError, lane_allowed, native_relevant  # noqa: E402

SCRIPT = ROOT / "scripts" / "ci_native_lane.py"


class NativeLaneTests(unittest.TestCase):
    def test_frontend_and_workflow_paths_never_need_native_runners(self):
        for path in [
            ".github/workflows/ci.yml",
            "src/components/Home.tsx",
            "src/web/main.tsx",
            "e2e/page-canvas-drag.spec.ts",
            "preview/main.tsx",
            "review/index.html",
            "public/fonts/x.woff2",
            "vite.config.ts",
            "tsconfig.json",
        ]:
            self.assertFalse(native_relevant(path), path)

    def test_native_paths_stay_relevant(self):
        for path in [
            "app/src/lib.rs",
            "crates/wenlan-core/src/config.rs",
            "Cargo.lock",
            "package.json",
            "pnpm-lock.yaml",
            "scripts/install-macos-app.sh",
            "scripts/ci_test_plan.py",
            "scripts/platform-contracts/check.sh",
            ".github/workflows/release.yml",
            "vendor/tauri-plugin-updater/src/lib.rs",
            "srcx/file",
            "tooling/vite.review.config.ts",
        ]:
            self.assertTrue(native_relevant(path), path)

    def test_merge_queue_runs_lane_only_for_native_relevant_files(self):
        for platform in ("macos", "windows"):
            self.assertFalse(
                lane_allowed(
                    platform=platform,
                    event_name="merge_group",
                    head_ref="",
                    files=[".github/workflows/ci.yml", "src/App.tsx"],
                )
            )
            self.assertFalse(
                lane_allowed(platform=platform, event_name="merge_group", head_ref="", files=[])
            )
            self.assertTrue(
                lane_allowed(
                    platform=platform,
                    event_name="merge_group",
                    head_ref="",
                    files=[".github/workflows/ci.yml", "app/src/lib.rs"],
                )
            )

    def test_darwin_preflight_keeps_workflow_edits_in_the_queue(self):
        # Main push never runs the darwin release preflight, so a ci.yml edit
        # has no later pre-release run of that leg.
        self.assertTrue(
            lane_allowed(
                platform="darwin-preflight",
                event_name="merge_group",
                head_ref="",
                files=[".github/workflows/ci.yml"],
            )
        )
        self.assertFalse(
            lane_allowed(
                platform="darwin-preflight",
                event_name="merge_group",
                head_ref="",
                files=["src/App.tsx"],
            )
        )
        self.assertFalse(
            lane_allowed(
                platform="darwin-preflight",
                event_name="pull_request",
                head_ref="x",
                files=[".github/workflows/ci.yml"],
            )
        )

    def test_pull_request_keeps_windows_and_defers_macos(self):
        self.assertFalse(
            lane_allowed(platform="macos", event_name="pull_request", head_ref="x", files=["app/a.rs"])
        )
        self.assertTrue(
            lane_allowed(platform="windows", event_name="pull_request", head_ref="x", files=[])
        )
        for platform in ("macos", "windows"):
            self.assertTrue(
                lane_allowed(
                    platform=platform,
                    event_name="pull_request",
                    head_ref="release-please--branches--main",
                    files=[],
                )
            )

    def test_backstop_events_keep_every_lane(self):
        for event in ("push", "workflow_dispatch"):
            for platform in ("macos", "windows"):
                self.assertTrue(
                    lane_allowed(platform=platform, event_name=event, head_ref="", files=[])
                )

    def test_unknown_inputs_fail_closed(self):
        with self.assertRaises(LaneError):
            lane_allowed(platform="linux", event_name="push", head_ref="", files=[])
        with self.assertRaises(LaneError):
            lane_allowed(platform="macos", event_name="schedule", head_ref="", files=[])

    def test_cli_unions_file_lists_and_writes_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "out"
            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--platform",
                    "macos",
                    "--event-name",
                    "merge_group",
                    "--files-json",
                    '["src/a.tsx"]',
                    "--files-json",
                    "",
                    "--files-json",
                    '["crates/wenlan-cli/src/commands/service.rs"]',
                    "--github-output",
                    str(output),
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(output.read_text(), "allowed=true\n")

    def test_cli_rejects_malformed_file_lists(self):
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--platform",
                "macos",
                "--event-name",
                "merge_group",
                "--files-json",
                '{"a": 1}',
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 1)


if __name__ == "__main__":
    unittest.main()
