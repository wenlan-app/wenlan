#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Manual, non-publishing, paired desktop build measurement.

The product checkout is immutable. A and B differ only in TARGET_TRIPLE.
No app is launched; no release, external signing, or compilation cache is used.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import lzma
import os
from pathlib import Path, PurePosixPath
import platform
import plistlib
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import tarfile
import urllib.request
import uuid
import zipfile

ENV_KEYS = (
    "TARGET_TRIPLE", "TAURI_ENV_TARGET_TRIPLE", "TAURI_ENV_DEBUG", "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS", "CARGO_TARGET_DIR", "CARGO_BUILD_JOBS",
    "CARGO_BUILD_BUILD_DIR", "CARGO_INCREMENTAL", "CXXFLAGS", "MACOSX_DEPLOYMENT_TARGET",
    "CMAKE_GENERATOR", "CMAKE_C_COMPILER", "CMAKE_CXX_COMPILER", "ORT_LIB_LOCATION",
    "ORT_CACHE_DIR", "ORT_SKIP_DOWNLOAD", "CARGO_NET_OFFLINE",
    "CARGO_PROFILE_RELEASE_LTO", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS",
    "CARGO_PROFILE_RELEASE_STRIP", "CARGO_PROFILE_RELEASE_INCREMENTAL",
    "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER", "OPENSSL_SRC_PERL", "VULKAN_SDK",
)
ORIGINAL_HOOK = "pnpm prepare:sidecars:tauri-build && pnpm build"


def utc():
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def save(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def digest(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as source:
        for data in iter(lambda: source.read(4 * 1024 * 1024), b""):
            h.update(data)
    return h.hexdigest()


def observe(command, cwd, env, prefix, *, shell=False, artifacts=False):
    """Tee stdout/stderr separately; preserve Cargo's stdout JSON for Tauri."""
    prefix = Path(prefix)
    prefix.parent.mkdir(parents=True, exist_ok=True)
    started, wall = utc(), time.perf_counter()
    found, errors = [], []
    process = subprocess.Popen(command, cwd=cwd, env=env, shell=shell,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    def pump(pipe, stream, suffix):
        try:
            with Path(str(prefix) + suffix).open("wb") as log:
                for line in iter(pipe.readline, b""):
                    log.write(line)
                    stream.buffer.write(line)
                    stream.buffer.flush()
                    if artifacts and suffix == ".stdout.log":
                        try:
                            record = json.loads(line)
                        except (ValueError, UnicodeDecodeError):
                            continue
                        if isinstance(record, dict) and record.get("reason") == "compiler-artifact":
                            found.append({key: record.get(key) for key in (
                                "package_id", "target", "profile", "features", "fresh",
                                "filenames", "executable")})
        except BaseException as error:
            errors.append(repr(error))

    threads = [threading.Thread(target=pump, args=(process.stdout, sys.stdout, ".stdout.log")),
               threading.Thread(target=pump, args=(process.stderr, sys.stderr, ".stderr.log"))]
    for worker in threads:
        worker.start()
    code = process.wait()
    for worker in threads:
        worker.join()
    result = dict(command=command, started_at=started, ended_at=utc(),
                  seconds=time.perf_counter() - wall, exit_code=code,
                  observation_errors=errors)
    if artifacts:
        result.update(artifacts=found, compiled=sum(x["fresh"] is False for x in found),
                      fresh=sum(x["fresh"] is True for x in found))
    save(str(prefix) + ".json", result)
    if errors:
        raise RuntimeError(f"incomplete process observation: {errors}")
    return result


def fingerprints(target):
    answer = {}
    for root in (target / "release/.fingerprint",):
        if root.exists():
            for path in root.glob("*/*.json"):
                answer[str(path.relative_to(target))] = json.loads(path.read_text(encoding="utf-8"))
    for root in target.glob("*/release/.fingerprint"):
        for path in root.glob("*/*.json"):
            answer[str(path.relative_to(target))] = json.loads(path.read_text(encoding="utf-8"))
    return answer


def cargo_mode(arguments):
    real = os.environ["BENCH_REAL_CARGO"]
    if not arguments or arguments[0] not in ("build", "rustc", "metadata"):
        return subprocess.call([real, *arguments])
    arguments = list(arguments)
    insertion = arguments.index("--") if "--" in arguments else len(arguments)
    extra = [flag for flag in ("--offline", "--locked") if flag not in arguments]
    if arguments[0] in ("build", "rustc"):
        if not any(x.startswith("--timings") for x in arguments):
            extra.append("--timings")
        if not any(x.startswith("--message-format") for x in arguments):
            extra.append("--message-format=json-render-diagnostics")
        if "--verbose" not in arguments and "-v" not in arguments:
            extra.append("--verbose")
    arguments[insertion:insertion] = extra
    if arguments[0] == "metadata":
        return subprocess.call([real, *arguments])
    receipt_dir = Path(os.environ["BENCH_ARM_RECEIPTS"])
    invocation = receipt_dir / f"cargo-{time.time_ns()}-{uuid.uuid4().hex[:6]}"
    timing_root = Path(os.environ["CARGO_TARGET_DIR"]) / "cargo-timings"
    before_timings = {str(path): digest(path) for path in timing_root.glob("*.html")}
    result = observe([real, *arguments], Path.cwd(), dict(os.environ), invocation, artifacts=True)
    result["environment"] = {k: os.environ.get(k) for k in ENV_KEYS}
    result["cwd"] = str(Path.cwd())
    save(str(invocation) + ".json", result)
    save(str(invocation) + ".fingerprints.json", fingerprints(Path(os.environ["CARGO_TARGET_DIR"])))
    html_files = [path for path in timing_root.glob("*.html") if before_timings.get(str(path)) != digest(path)]
    if result["exit_code"] == 0 and not html_files:
        raise RuntimeError("successful Cargo build has no timing evidence")
    for html in html_files:
        timings = Path(str(invocation) + ".timings")
        timings.mkdir(exist_ok=True)
        shutil.copyfile(html, timings / html.name)
    return result["exit_code"]


def hook_mode():
    if os.environ["BENCH_ORIGINAL_HOOK"] != ORIGINAL_HOOK:
        raise RuntimeError("product beforeBuildCommand changed; measurement needs review")
    directory = Path(os.environ["BENCH_ARM_RECEIPTS"])
    for phase, command in zip(("sidecars-hook", "frontend"), ORIGINAL_HOOK.split(" && ")):
        result = observe(command, Path.cwd(), dict(os.environ), directory / phase, shell=True)
        if result["exit_code"]:
            return result["exit_code"]
    return 0


SHIM = r'''
use std::{env, process::{Command, exit}};
fn main() {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let mut command = if args.first().map(|x| x == "build" || x == "rustc" || x == "metadata").unwrap_or(false) {
        let mut cmd = Command::new(env::var_os("BENCH_PYTHON").expect("BENCH_PYTHON"));
        cmd.arg(env::var_os("BENCH_DRIVER").expect("BENCH_DRIVER")).arg("cargo"); cmd
    } else { Command::new(env::var_os("BENCH_REAL_CARGO").expect("BENCH_REAL_CARGO")) };
    let status = command.args(args).status().expect("cargo observer child could not start");
    exit(status.code().unwrap_or(1));
}
'''


def checked_output(command, cwd):
    return subprocess.check_output(command, cwd=cwd, text=True).strip()


def check_arm(result):
    if result["exit_code"] != 0:
        raise RuntimeError("desktop build failed; retain partial receipt, do not score timing")
    if result["observation_errors"]:
        raise RuntimeError("desktop build observation incomplete")


def run_pair(args):
    repo, output = Path(args.repo).resolve(), Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    expected = args.source_sha
    actual = checked_output(["git", "rev-parse", "HEAD"], repo)
    if actual != expected or len(expected) != 40:
        raise RuntimeError("product checkout does not match the immutable merged source SHA")
    dirty = checked_output(["git", "status", "--porcelain", "--untracked-files=no"], repo)
    if dirty:
        raise RuntimeError("tracked product source is dirty")
    for key in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_BUILD_DIR", "CARGO_NET_OFFLINE", "WENLAN_BACKEND_DIR"):
        if os.environ.get(key):
            raise RuntimeError(f"uncontrolled build input {key} is set")
    product_config = json.loads((repo / "app/tauri.conf.json").read_text(encoding="utf-8"))
    if product_config["build"]["beforeBuildCommand"] != ORIGINAL_HOOK:
        raise RuntimeError("unexpected product hook")
    real_cargo = checked_output(["rustup", "which", "cargo"], repo)
    compiler = checked_output(["rustc", "-vV"], repo)
    if f"host: {args.target}" not in compiler:
        raise RuntimeError("A/B expects native host == explicit target")
    locks = {name: digest(repo / name) for name in ("Cargo.lock", "pnpm-lock.yaml")}
    control = repo / "target/desktop-benchmark-control"
    control.mkdir(parents=True, exist_ok=True)
    driver = control / "driver.py"
    shutil.copyfile(Path(__file__), driver)
    shim_source = control / "cargo-shim.rs"
    shim_source.write_text(SHIM, encoding="utf-8")
    shim_dir = control / "bin"
    shim_dir.mkdir(exist_ok=True)
    subprocess.run(["rustc", str(shim_source), "-o", str(shim_dir / ("cargo.exe" if os.name == "nt" else "cargo"))],
                   cwd=repo, check=True)
    hook_command = subprocess.list2cmdline([sys.executable, str(driver), "hook"]) if os.name == "nt" else \
        __import__("shlex").join([sys.executable, str(driver), "hook"])
    overlay = control / "tauri-measurement.json"
    save(overlay, {"build": {"beforeBuildCommand": hook_command},
                   "bundle": {"createUpdaterArtifacts": False, "macOS": {"signingIdentity": "-"}}})
    manifest = dict(schema=1, source_sha=actual,
                    source_tree=checked_output(["git", "rev-parse", "HEAD^{tree}"], repo),
                    order=args.order, target=args.target, platform=platform.platform(),
                    cpu=platform.processor(), cpu_count=os.cpu_count(), rustc=compiler,
                    cargo=checked_output([real_cargo, "--version"], repo),
                    node=checked_output(["node", "--version"], repo),
                    lock_sha256=locks, original_hook=ORIGINAL_HOOK,
                    exclusions=["updater archives/signatures", "external signing/notarization", "publication", "runner queue"],
                    runner={k: os.environ.get(k) for k in ("RUNNER_OS", "RUNNER_ARCH", "RUNNER_NAME", "ImageOS", "ImageVersion", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")},
                    environment={k: os.environ.get(k) for k in ENV_KEYS}, arms=[])
    save(output / "pair.json", manifest)
    parent = Path(tempfile.mkdtemp(prefix="wenlan-desktop-ab-", dir=os.environ.get("RUNNER_TEMP")))
    for arm in args.order:
        arm_dir = output / arm
        target = parent / f"target-{arm}"
        target.mkdir()
        if any(target.iterdir()):
            raise RuntimeError("cold target directory is not empty")
        env = dict(os.environ)
        env.pop("TARGET_TRIPLE", None)
        if arm == "B":
            env["TARGET_TRIPLE"] = args.target
        env.update(CARGO_TARGET_DIR=str(target), WENLAN_NO_AUTOSTART="1",
                   BENCH_REAL_CARGO=real_cargo, BENCH_PYTHON=sys.executable,
                   BENCH_DRIVER=str(driver), BENCH_ARM_RECEIPTS=str(arm_dir),
                   BENCH_ORIGINAL_HOOK=ORIGINAL_HOOK,
                   PATH=str(shim_dir) + os.pathsep + env["PATH"])
        if sys.platform == "darwin":
            env.update(CXXFLAGS="-std=c++17", APPLE_SIGNING_IDENTITY="-")
        for name in ("APPLE_CERTIFICATE", "APPLE_ID", "APPLE_PASSWORD", "APPLE_TEAM_ID", "TAURI_SIGNING_PRIVATE_KEY"):
            if env.get(name):
                raise RuntimeError(f"publishing/signing credential {name} must not be present")
        suffix = ".exe" if os.name == "nt" else ""
        for stem in ("wenlan", "wenlan-server", "wenlan-mcp"):
            staged = repo / "app/binaries" / f"{stem}-{args.target}{suffix}"
            if staged.is_symlink():
                raise RuntimeError("unexpected staged symlink")
            staged.unlink(missing_ok=True)
        # Only generated TypeScript build receipts are removed; product source is untouched.
        for root in (repo, repo / "node_modules/.tmp"):
            if root.exists():
                for stamp in root.glob("*.tsbuildinfo"):
                    stamp.unlink()
        env_before = {k: env.get(k) for k in ENV_KEYS}
        command = f'pnpm tauri build --target {args.target} --config "{overlay}"'
        result = observe(command, repo, env, arm_dir / "tauri", shell=True)
        result.update(arm=arm, environment=env_before, cold_target=True,
                      disk_free_after_build=shutil.disk_usage(target).free)
        save(arm_dir / "arm.json", result)
        manifest["arms"].append(result)
        save(output / "pair.json", manifest)
        check_arm(result)
        cargo_receipts = [json.loads(path.read_text(encoding="utf-8")) for path in arm_dir.glob("cargo-*.json")
                          if not path.name.endswith(".fingerprints.json")]
        if len(cargo_receipts) < 2 or any(x["exit_code"] or not x["artifacts"] for x in cargo_receipts):
            raise RuntimeError("did not observe successful actual sidecar and app Cargo builds")
        hook = json.loads((arm_dir / "sidecars-hook.json").read_text(encoding="utf-8"))
        frontend = json.loads((arm_dir / "frontend.json").read_text(encoding="utf-8"))
        app = [x for x in cargo_receipts if any("wenlan-app" in a["package_id"] for a in x["artifacts"]) ]
        if len(app) != 1:
            raise RuntimeError("cannot identify the one native app Cargo invocation")
        sidecar_source = target / args.target / "release" if arm == "B" else target / "release"
        outputs = []
        for stem in ("wenlan", "wenlan-server", "wenlan-mcp"):
            source = sidecar_source / f"{stem}{suffix}"
            staged = repo / "app/binaries" / f"{stem}-{args.target}{suffix}"
            if not source.is_file() or source.stat().st_size < 1024 or digest(source) != digest(staged):
                raise RuntimeError("new real sidecar was not staged correctly")
            outputs.append(dict(kind="sidecar", path=str(source.relative_to(target)), size=source.stat().st_size, sha256=digest(source)))
        bundle = target / args.target / "release/bundle"
        if sys.platform == "darwin":
            installers = list((bundle / "dmg").glob("*.dmg"))
            applications = list((bundle / "macos").glob("*.app"))
            if len(installers) != 1 or len(applications) != 1:
                raise RuntimeError("expected both the complete .app and DMG")
            with (applications[0] / "Contents/Info.plist").open("rb") as source:
                executable = plistlib.load(source)["CFBundleExecutable"]
            if Path(executable).name != executable:
                raise RuntimeError("invalid app main executable name")
            app_binaries = [applications[0] / "Contents/MacOS" / executable]
        else:
            installers = list((bundle / "nsis").glob("*.exe")) + list((bundle / "msi").glob("*.msi"))
            app_binaries = [target / args.target / "release/wenlan-app.exe"]
            if len(list((bundle / "nsis").glob("*.exe"))) != 1 or len(list((bundle / "msi").glob("*.msi"))) != 1:
                raise RuntimeError("expected both release-configured NSIS and MSI installers")
        for path in installers + app_binaries:
            if not path.is_file() or path.stat().st_size < 1024:
                raise RuntimeError("desktop output is absent or empty")
            outputs.append(dict(kind="installer" if path in installers else "app-main", path=str(path.relative_to(target)), size=path.stat().st_size, sha256=digest(path)))
        result.update(sidecars_hook_seconds=hook["seconds"], frontend_seconds=frontend["seconds"],
                      app_cargo_seconds=app[0]["seconds"],
                      packaging_and_cli_seconds=result["seconds"] - hook["seconds"] - frontend["seconds"] - app[0]["seconds"],
                      artifacts=outputs, cargo_invocations=len(cargo_receipts),
                      compiled=sum(x["compiled"] for x in cargo_receipts), fresh=sum(x["fresh"] for x in cargo_receipts),
                      observation_valid=True)
        if checked_output(["git", "status", "--porcelain", "--untracked-files=no"], repo):
            raise RuntimeError("tracked product source changed during measured build")
        if locks != {name: digest(repo / name) for name in locks}:
            raise RuntimeError("lockfile changed during measured build")
        save(arm_dir / "arm.json", result)
        save(output / "pair.json", manifest)
        # This exact directory was made above under our own mkdtemp root. Resolve
        # and constrain before any recursive delete, including on Windows.
        if target.resolve().parent != parent.resolve() or target.name != f"target-{arm}":
            raise RuntimeError("refuse cleanup outside owned measurement target")
        shutil.rmtree(target)
    scored = score_pair(manifest)
    save(output / "result.json", scored)
    print(json.dumps(scored, indent=2))
    return 0


def score_pair(manifest):
    arms = {arm["arm"]: arm for arm in manifest["arms"]}
    if set(arms) != {"A", "B"} or len(manifest["arms"]) != 2:
        raise ValueError("paired measurement requires exactly A and B")
    if not all(arm.get("observation_valid") and arm["exit_code"] == 0 for arm in arms.values()):
        raise ValueError("failed/incomplete arms cannot be scored")
    a, b = arms["A"]["seconds"], arms["B"]["seconds"]
    if a <= 0 or b <= 0:
        raise ValueError("duration must be positive")
    return dict(source_sha=manifest["source_sha"], target=manifest["target"], order=manifest["order"],
                old_seconds=a, new_seconds=b, saved_seconds=a - b, saved_fraction=(a - b) / a,
                arms=manifest["arms"], exclusions=manifest["exclusions"])


def safe_member(name):
    path = PurePosixPath(name.replace("\\", "/"))
    if path.is_absolute() or ".." in path.parts or any(":" in part for part in path.parts):
        raise ValueError("unsafe native download archive path")
    return path


def download_verified(url, path, algorithm, expected):
    with urllib.request.urlopen(url, timeout=120) as response, path.open("wb") as target:
        shutil.copyfileobj(response, target)
    actual = hashlib.new(algorithm, path.read_bytes()).hexdigest()
    if actual.lower() != expected.lower():
        raise RuntimeError(f"native tool checksum mismatch: {url}")
    return dict(url=url, algorithm=algorithm, digest=actual, size=path.stat().st_size)


def unzip_checked(archive, destination, strip=None):
    with zipfile.ZipFile(archive) as bundle:
        for member in bundle.infolist():
            path = safe_member(member.filename)
            if strip:
                if path.parts[0] != strip:
                    raise ValueError("unexpected native ZIP root")
                path = PurePosixPath(*path.parts[1:])
            if not path.parts:
                continue
            if (member.external_attr >> 16) & 0o170000 == 0o120000:
                raise ValueError("native ZIP symlink refused")
            target = destination.joinpath(*path.parts)
            if member.is_dir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with bundle.open(member) as source, target.open("wb") as output:
                    shutil.copyfileobj(source, output)


def prefetch_mode():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args(sys.argv[2:])
    repo, output = Path(args.repo).resolve(), Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    native = Path(os.environ["RUNNER_TEMP"]) / "desktop-native-downloads"
    native.mkdir(exist_ok=True)
    downloads = []
    if sys.platform == "darwin":
        pin = "0897a0e1b840566a97e5a49497b02cbc204be2d006815174b639bc99731840f9"
        lock = (repo / "Cargo.lock").read_text(encoding="utf-8")
        if 'name = "ort-sys"\nversion = "2.0.0-rc.11"' not in lock.replace("\r\n", "\n"):
            raise RuntimeError("ORT version changed; prefetch recipe must be reviewed")
        archive = native / "ort.tar.lzma2"
        downloads.append(download_verified(
            "https://cdn.pyke.io/0/pyke:ort-rs/ms@1.23.2/aarch64-apple-darwin.tar.lzma2",
            archive, "sha256", pin))
        cache_parent = native / "dfbin/aarch64-apple-darwin"
        cache_parent.mkdir(parents=True, exist_ok=True)
        scratch = Path(tempfile.mkdtemp(prefix="extract-", dir=cache_parent))
        with lzma.LZMAFile(archive, format=lzma.FORMAT_RAW,
                           filters=[{"id": lzma.FILTER_LZMA2, "dict_size": 1 << 26}]) as stream:
            with tarfile.open(fileobj=stream, mode="r|") as bundle:
                for member in bundle:
                    path = safe_member(member.name)
                    target = scratch.joinpath(*path.parts)
                    if member.isdir():
                        target.mkdir(parents=True, exist_ok=True)
                    elif member.isfile():
                        target.parent.mkdir(parents=True, exist_ok=True)
                        with bundle.extractfile(member) as source, target.open("wb") as destination:
                            shutil.copyfileobj(source, destination)
                    else:
                        raise ValueError("native tar links/devices refused")
        if not list(scratch.rglob("libonnxruntime.a")):
            raise RuntimeError("ORT archive is incomplete")
        scratch.rename(cache_parent / pin)
        with Path(os.environ["GITHUB_ENV"]).open("a", encoding="utf-8") as environment:
            environment.write(f"ORT_CACHE_DIR={native}\n")
    elif os.name == "nt":
        version = checked_output(["node", "-p", "require('@tauri-apps/cli/package.json').version"], repo)
        if version != "2.11.4":
            raise RuntimeError("Tauri CLI changed; prefetch recipe must be reviewed")
        tools = Path(os.environ["LOCALAPPDATA"]) / "tauri"
        tools.mkdir(exist_ok=True)
        archive = native / "nsis.zip"
        downloads.append(download_verified(
            "https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.11/nsis-3.11.zip",
            archive, "sha1", "EF7FF767E5CBD9EDD22ADD3A32C9B8F4500BB10D"))
        unzip_checked(archive, tools / "NSIS", strip="nsis-3.11")
        plugin = tools / "NSIS/Plugins/x86-unicode/additional/nsis_tauri_utils.dll"
        plugin.parent.mkdir(parents=True, exist_ok=True)
        downloads.append(download_verified(
            "https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll",
            plugin, "sha1", "75197FEE3C6A814FE035788D1C34EAD39349B860"))
        for name in ("makensis.exe", "Bin/makensis.exe", "Stubs/lzma-x86-unicode", "Stubs/lzma_solid-x86-unicode",
                     "Plugins/x86-unicode/additional/nsis_tauri_utils.dll", "Include/MUI2.nsh", "Include/FileFunc.nsh",
                     "Include/x64.nsh", "Include/nsDialogs.nsh", "Include/WinMessages.nsh", "Include/Win/COM.nsh",
                     "Include/Win/Propkey.nsh", "Include/Win/RestartManager.nsh"):
            if not (tools / "NSIS" / name).is_file():
                raise RuntimeError(f"incomplete NSIS tool: {name}")
        archive = native / "wix.zip"
        downloads.append(download_verified(
            "https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip",
            archive, "sha256", "6ac824e1642d6f7277d0ed7ea09411a508f6116ba6fae0aa5f2c7daa2ff43d31"))
        unzip_checked(archive, tools / "WixTools314")
        for name in ("candle.exe", "candle.exe.config", "darice.cub", "light.exe", "light.exe.config", "wconsole.dll", "winterop.dll", "wix.dll", "WixUIExtension.dll", "WixUtilExtension.dll"):
            if not (tools / "WixTools314" / name).is_file():
                raise RuntimeError(f"incomplete WiX tool: {name}")
    else:
        raise RuntimeError("desktop benchmark only supports the shipped Windows/macOS hosts")
    save(output / "native-prefetch.json", dict(downloads=downloads, completed_at=utc()))
    return 0


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "prefetch":
        return prefetch_mode()
    if len(sys.argv) > 1 and sys.argv[1] == "cargo":
        return cargo_mode(sys.argv[2:])
    if len(sys.argv) > 1 and sys.argv[1] == "hook":
        return hook_mode()
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--order", choices=("AB", "BA"), required=True)
    return run_pair(parser.parse_args())


if __name__ == "__main__":
    sys.exit(main())
