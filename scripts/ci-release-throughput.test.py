#!/usr/bin/env python3
"""Offline regressions for release proof routing and Cargo dependency reuse."""

from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


CONTRACT = load("throughput_workflow_contract", "release-workflow-contract.test.py")
PROMOTION = load("throughput_promotion", "release-promotion.py")


class ReleaseThroughputTests(unittest.TestCase):
    def test_desktop_recipes_share_the_explicit_cargo_target(self):
        release = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        for job_name, step_name, target in [
            ("app-bundle", "Build macOS desktop app bundle", "aarch64-apple-darwin"),
            ("app-bundle-windows", "Build Windows desktop app bundle", "x86_64-pc-windows-msvc"),
        ]:
            step = CONTRACT.named_step_body(CONTRACT.job_body(release, job_name), step_name)
            self.assertIn(f"TARGET_TRIPLE: {target}", step)
            self.assertIn(f"pnpm tauri build --target {target}", step)
        self.assertIn("TARGET_TRIPLE: x86_64-pc-windows-msvc", CONTRACT.job_body(ci, "app-windows-bundle"))

    def test_queue_skip_requires_trusted_proof_and_cannot_mint_a_main_receipt(self):
        ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        detect = CONTRACT.job_body(ci, "detect-changes")
        queue = CONTRACT.named_step_body(detect, "Verify reusable release queue")
        self.assertIn("if: github.event_name == 'merge_group'", queue)
        self.assertIn("verifier=trusted-release-gate/scripts/release-promotion.py", queue)
        self.assertIn('python3 "$verifier" gate-queue', queue)
        self.assertIn('echo "verified-release-merge=false"', queue)
        self.assertNotIn("continue-on-error", queue)
        self.assertNotIn("--plan-output", queue)
        self.assertIn("actions: read", detect)
        self.assertIn("steps.queue-proof.outputs.verified-release-merge == 'true'", detect)
        upload = CONTRACT.named_step_body(detect, "Upload main release promotion receipt")
        self.assertIn("if: steps.release-proof.outputs.release-gate-state == 'validated'", upload)
        self.assertNotIn("queue-proof", upload)

    def test_observed_healthy_main_survives_new_budget_but_not_old_budget(self):
        ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        proof = CONTRACT.job_body(ci, "release-base-proof")
        budget = int(re.search(r"--wait-seconds (\d+)", proof).group(1))
        job_limit = int(re.search(r"timeout-minutes: (\d+)", proof).group(1)) * 60
        self.assertGreater(job_limit, budget)
        for limit, succeeds in [(2400, False), (budget, True)]:
            clock = [0.0]
            def sleep(seconds):
                clock[0] += seconds
            def upstream(*args):
                # Oct 28: still pending at 40m; healthy main finished shortly after.
                if clock[0] < 45 * 60:
                    raise PROMOTION.PendingEvidence("exact main CI is in_progress")
                return {"id": 100, "conclusion": "success"}
            api = mock.Mock()
            api.get_json.return_value = {"id": 2, "full_name": "wenlan-app/wenlan"}
            with mock.patch.object(PROMOTION, "_verify_main_ci_once", side_effect=upstream):
                if succeeds:
                    result = PROMOTION.verify_main_ci(
                        api, "wenlan-app/wenlan", "a" * 40, wait_seconds=limit,
                        sleep=sleep, monotonic=lambda: clock[0],
                    )
                    self.assertEqual(result["conclusion"], "success")
                else:
                    with self.assertRaisesRegex(PROMOTION.PromotionError, "timed out"):
                        PROMOTION.verify_main_ci(
                            api, "wenlan-app/wenlan", "a" * 40, wait_seconds=limit,
                            sleep=sleep, monotonic=lambda: clock[0],
                        )

    def test_explicit_target_reuses_shared_dependency_in_real_cargo(self):
        host = re.search(r"^host: (.+)$", subprocess.check_output(
            ["rustc", "-vV"], text=True), re.MULTILINE).group(1)
        with tempfile.TemporaryDirectory(prefix="desktop-reuse-") as raw:
            root = Path(raw)
            (root / "Cargo.toml").write_text(
                '[workspace]\nmembers=["shared", "sidecar", "app"]\nresolver="3"\n', encoding="utf-8")
            for name in ["shared", "sidecar", "app"]:
                folder = root / name
                (folder / "src").mkdir(parents=True)
                manifest = f'[package]\nname="fixture-{name}"\nversion="0.1.0"\nedition="2024"\n'
                if name != "shared":
                    manifest += '[dependencies]\nfixture-shared={path="../shared"}\n'
                (folder / "Cargo.toml").write_text(manifest, encoding="utf-8")
                body = 'pub fn value() -> u32 { 42 }' if name == "shared" else 'pub fn value() -> u32 { fixture_shared::value() }'
                (folder / "src/lib.rs").write_text(body, encoding="utf-8")
            env = dict(os.environ)
            for key in ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"]:
                env.pop(key, None)
            env["WENLAN_NO_AUTOSTART"] = "1"
            def build(name, target_dir, explicit):
                command = ["cargo", "build", "--offline", "--release", "--message-format=json",
                           "--manifest-path", str(root / "Cargo.toml"),
                           "--target-dir", str(target_dir), "-p", f"fixture-{name}"]
                if explicit:
                    command += ["--target", host]
                result = subprocess.run(command, env=env, capture_output=True, text=True, check=True)
                return [item for line in result.stdout.splitlines()
                        if (item := json.loads(line)).get("reason") == "compiler-artifact"
                        and item.get("target", {}).get("name") == "fixture_shared"]
            baseline = root / "baseline"
            fixed = root / "fixed"
            build("sidecar", baseline, False)
            old_app = build("app", baseline, True)
            build("sidecar", fixed, True)
            new_app = build("app", fixed, True)
            self.assertEqual([item["fresh"] for item in old_app], [False])
            self.assertEqual([item["fresh"] for item in new_app], [True])
            print("Cargo control: shared dependency compiled twice before, once with matching targets")


if __name__ == "__main__":
    unittest.main(verbosity=2)
