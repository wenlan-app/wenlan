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

    def test_toolchain_provisioning_with_exit_zero_cannot_be_scored(self):
        with tempfile.TemporaryDirectory(prefix="timing-contamination-") as raw:
            prefix = Path(raw) / "observed"
            with self.assertRaisesRegex(RuntimeError, "contaminated"):
                bench.observe([sys.executable, "-c", "print('\\x1b[1minfo\\x1b[0m: syncing channel updates for fixture')"],
                              Path(raw), dict(os.environ), prefix)
            receipt = json.loads(Path(str(prefix) + ".json").read_text())
            self.assertEqual(receipt["exit_code"], 0)
            self.assertTrue(receipt["observation_errors"])
            ordinary = bench.observe([sys.executable, "-c", "import json; print(json.dumps({'message':'info: syncing channel updates for quoted text'}))"],
                                     Path(raw), dict(os.environ), Path(raw) / "ordinary")
            self.assertEqual(ordinary["observation_errors"], [])

    def test_actual_cmake_injection_disables_default_native_cache_and_guard_rejects_on(self):
        with tempfile.TemporaryDirectory(prefix="native-cache-control-") as raw:
            root = Path(raw)
            source = root / "source"
            source.mkdir()
            (source / "bin").mkdir()
            fake = source / "bin" / ("ccache.exe" if os.name == "nt" else "ccache")
            fake.write_bytes(b"fixture is found but never executed")
            fake.chmod(0o755)
            (source / "CMakeLists.txt").write_text('cmake_minimum_required(VERSION 3.20)\nproject(CacheControl NONE)\noption(GGML_CCACHE "native cache" ON)\nif(GGML_CCACHE)\nfind_program(CCACHE_TOOL NAMES ccache PATHS "${CMAKE_CURRENT_SOURCE_DIR}/bin" NO_DEFAULT_PATH REQUIRED)\nendif()\n', encoding="utf-8")
            control = root / "control.cmake"
            control.write_text(bench.NATIVE_CACHE_CONTROL, encoding="utf-8")
            for mode in ("old", "new"):
                build = root / mode / "release/build/llama-cpp-sys-2-fixture/out/build"
                command = ["cmake", "-S", str(source), "-B", str(build)]
                if mode == "new":
                    command.append("-DCMAKE_PROJECT_INCLUDE=" + control.as_posix())
                done = subprocess.run(command, capture_output=True, text=True)
                self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
                incidental = build / "CMakeFiles/TryCompile-fixture/CMakeCache.txt"
                incidental.parent.mkdir(parents=True, exist_ok=True)
                incidental.write_text("UNRELATED:BOOL=ON\n", encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "did not disable"):
                bench.native_cache_evidence(root / "old")
            evidence = bench.native_cache_evidence(root / "new")
            self.assertEqual(len(evidence), 1)
            self.assertIn("GGML_CCACHE:BOOL=OFF", next(iter(evidence.values())))

    def test_failed_observer_bootstrap_preserves_compiler_diagnostics(self):
        with tempfile.TemporaryDirectory(prefix="bootstrap-error-control-") as raw:
            real = subprocess.check_output(["rustup", "which", "cargo"], text=True).strip()
            original = bench.SHIM
            try:
                bench.SHIM = "fn main() { MISSING_BOOTSTRAP_SYMBOL(); }"
                with self.assertRaisesRegex(RuntimeError, "MISSING_BOOTSTRAP_SYMBOL"):
                    bench.compile_observer(Path(raw) / "tool", real, Path(raw) / "receipt")
                receipt = json.loads((Path(raw) / "receipt-1.json").read_text())
                self.assertNotEqual(receipt["exit_code"], 0)
                self.assertIn("MISSING_BOOTSTRAP_SYMBOL", (Path(raw) / "receipt-1.stderr.log").read_text(encoding="utf-8"))
            finally:
                bench.SHIM = original

    def test_native_cargo_shim_preserves_json_and_real_freshness(self):
        with tempfile.TemporaryDirectory(prefix="desktop-observer-test-") as raw:
            root = Path(raw)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text('[package]\nname="observer-fixture"\nversion="0.1.0"\nedition="2021"\n[workspace]\n', encoding="utf-8")
            (root / "src/lib.rs").write_text("pub fn value() -> u32 { 42 }", encoding="utf-8")
            # --offline must not become the ORT-breaking environment variable.
            (root / "build.rs").write_text('fn main() { println!("cargo:rerun-if-changed=build.rs"); assert!(std::env::var_os("CARGO_NET_OFFLINE").is_none()); assert!(std::process::Command::new("rustup").args(["which", "rustc"]).output().unwrap().status.success()); }', encoding="utf-8")
            real = subprocess.check_output(["rustup", "which", "cargo"], text=True).strip()
            shim = bench.compile_observer(root / "observer-tool", real)
            subprocess.run([real, "generate-lockfile", "--offline", "--manifest-path", str(root / "Cargo.toml")], check=True, capture_output=True)
            env = dict(os.environ)
            for key in ("CARGO_NET_OFFLINE", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
                env.pop(key, None)
            env.update(BENCH_PYTHON=sys.executable, BENCH_DRIVER=str(Path(bench.__file__).resolve()),
                       BENCH_REAL_CARGO=real, BENCH_ARM_RECEIPTS=str(root / "receipts"), CARGO_TARGET_DIR=str(root / "target"))
            env["RUSTC"] = subprocess.check_output(["rustup", "which", "rustc"], text=True).strip()
            env["RUSTUP_TOOLCHAIN"] = subprocess.check_output(["rustup", "show", "active-toolchain"], text=True).split()[0]
            # A dependency-local file must not replace the observer's pinned
            # compiler context. The missing custom toolchain fails locally;
            # there is no diagnostic download or installation.
            (root / "rust-toolchain.toml").write_text('[toolchain]\nchannel="fixture-missing-toolchain"\n', encoding="utf-8")
            unpinned = dict(env)
            unpinned.pop("RUSTUP_TOOLCHAIN", None)
            refused = subprocess.run(["rustup", "which", "rustc"], cwd=root, env=unpinned, capture_output=True, text=True, timeout=10)
            self.assertNotEqual(refused.returncode, 0)
            for expected_fresh in (False, True):
                done = subprocess.run([str(shim), "build", "--manifest-path", str(root / "Cargo.toml"), "--release"], env=env, cwd=root, capture_output=True, text=True)
                self.assertEqual(done.returncode, 0, done.stderr)
                messages = [json.loads(line) for line in done.stdout.splitlines()]
                artifact = next(x for x in messages if x.get("reason") == "compiler-artifact" and x["target"]["kind"] == ["lib"])
                self.assertEqual(artifact["fresh"], expected_fresh)
            receipts = [p for p in (root / "receipts").glob("cargo-*.json") if len(p.suffixes) == 1]
            self.assertEqual(len(receipts), 2)
            self.assertTrue(all(json.loads(p.read_text())["exit_code"] == 0 for p in receipts))


if __name__ == "__main__":
    unittest.main()
