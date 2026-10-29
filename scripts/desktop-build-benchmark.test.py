#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Controls for timing score refusal, archive safety, and the real Cargo observer."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("desktop_bench", Path(__file__).with_name("desktop-build-benchmark.py"))
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


class BenchmarkTests(unittest.TestCase):
    def pair(self):
        return dict(source_sha="a" * 40, target="fixture", order="AB", exclusions=[], arms=[
            dict(arm="A", seconds=100, exit_code=0, observation_valid=True),
            dict(arm="B", seconds=80, exit_code=0, observation_valid=True)])

    def test_saved_time_is_wall_difference_and_can_be_negative(self):
        self.assertEqual(bench.score_pair(self.pair())["saved_seconds"], 20)
        pair = self.pair()
        pair["arms"][1]["seconds"] = 120
        self.assertEqual(bench.score_pair(pair)["saved_seconds"], -20)

    def test_failed_partial_duplicate_and_unobserved_pairs_cannot_score(self):
        for mutate in (lambda x: x["arms"][1].update(exit_code=1),
                       lambda x: x["arms"][1].update(observation_valid=False),
                       lambda x: x["arms"].pop(),
                       lambda x: x["arms"].append(copy.deepcopy(x["arms"][0]))):
            pair = self.pair()
            mutate(pair)
            with self.assertRaises(ValueError):
                bench.score_pair(pair)

    def test_archive_members_cannot_escape(self):
        for path in ("../x", "/etc/x", "a/../../x", "C:/x", "a\\..\\x"):
            with self.assertRaises(ValueError):
                bench.safe_member(path)
        self.assertEqual(str(bench.safe_member("lib/libonnxruntime.a")), "lib/libonnxruntime.a")

    def test_native_cargo_shim_preserves_json_and_real_freshness(self):
        with tempfile.TemporaryDirectory(prefix="desktop-observer-test-") as raw:
            root = Path(raw)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text('[package]\nname="observer-fixture"\nversion="0.1.0"\nedition="2021"\n[workspace]\n', encoding="utf-8")
            (root / "src/lib.rs").write_text("pub fn value() -> u32 { 42 }", encoding="utf-8")
            # --offline must not become the ORT-breaking environment variable.
            (root / "build.rs").write_text('fn main() { println!("cargo:rerun-if-changed=build.rs"); assert!(std::env::var_os("CARGO_NET_OFFLINE").is_none()); }', encoding="utf-8")
            shim = root / ("cargo.exe" if os.name == "nt" else "cargo")
            (root / "shim.rs").write_text(bench.SHIM, encoding="utf-8")
            subprocess.run(["rustc", str(root / "shim.rs"), "-o", str(shim)], check=True, capture_output=True)
            real = subprocess.check_output(["rustup", "which", "cargo"], text=True).strip()
            subprocess.run([real, "generate-lockfile", "--offline", "--manifest-path", str(root / "Cargo.toml")], check=True, capture_output=True)
            env = dict(os.environ)
            for key in ("CARGO_NET_OFFLINE", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
                env.pop(key, None)
            env.update(BENCH_PYTHON=sys.executable, BENCH_DRIVER=str(Path(bench.__file__).resolve()),
                       BENCH_REAL_CARGO=real, BENCH_ARM_RECEIPTS=str(root / "receipts"), CARGO_TARGET_DIR=str(root / "target"))
            for expected_fresh in (False, True):
                done = subprocess.run([str(shim), "build", "--manifest-path", str(root / "Cargo.toml"), "--release"], env=env, cwd=root, capture_output=True, text=True)
                self.assertEqual(done.returncode, 0, done.stderr)
                messages = [json.loads(line) for line in done.stdout.splitlines()]
                artifact = next(x for x in messages if x.get("reason") == "compiler-artifact" and x["target"]["kind"] == ["lib"])
                self.assertEqual(artifact["fresh"], expected_fresh)
            receipts = [p for p in (root / "receipts").glob("cargo-*.json") if not p.name.endswith(".fingerprints.json")]
            self.assertEqual(len(receipts), 2)
            self.assertTrue(all(json.loads(p.read_text())["exit_code"] == 0 for p in receipts))


if __name__ == "__main__":
    unittest.main()
