# Cross-platform support

Platform detail extracted from the root `AGENTS.md` so it loads only when you are
working on platform-conditional code, service registration, or the release matrix.

Supported builds and prebuilt releases cover macOS arm64, Linux x86_64/aarch64 with glibc, and Windows x86_64. macOS x86_64 is not a supported stock source-build target: the pinned ONNX Runtime dependency has no prebuilt Intel macOS binary, so a custom build must compile ONNX Runtime separately and provide it through `ORT_LIB_LOCATION`.

GitHub CI, release builds, and current benchmark baselines pin the standard ARM64 `macos-15` image. GitHub [retires `macos-14` on November 2, 2026](https://github.blog/changelog/2026-10-01-github-actions-macos-14-runner-image-retirement/), with reduced capacity and scheduled brownouts beforehand. The build-host migration preserves `aarch64-apple-darwin`, release asset names, and the desktop configuration's `minimumSystemVersion: 11.0`; that configuration alone does not prove every native dependency's runtime deployment floor. Inspect actual Mach-O deployment metadata and validate older-system runtime compatibility separately when needed.

| OS | Data dir | Service registration |
|---|---|---|
| macOS | `~/Library/Application Support/wenlan/` | launchd via `~/Library/LaunchAgents/com.wenlan.server.plist` (user-level) |
| Linux | `~/.local/share/wenlan/` (or `$XDG_DATA_HOME/wenlan`) | systemd user unit at `~/.config/systemd/user/wenlan-server.service` (qualifier dropped per `ServiceLabel::to_script_name()`). Enable lingering with `loginctl enable-linger` if you want the service alive after logout. |
| Windows | `%LOCALAPPDATA%\wenlan\` | Per-user Task Scheduler ONLOGON task registered via `schtasks.exe /create /tn <task-name> /xml <definition.xml> /f`. The XML separates the executable from its arguments and records the selected data root, bind address, and port. The default root uses `WenlanServer`; custom roots use `WenlanServer-<root-hash>`. |

`wenlan background on` / `wenlan background off` work on macOS, Linux, and Windows,
using launchd, systemd-user, or Task Scheduler for registration and startup.
Stopping uses the daemon's root-checked HTTP shutdown endpoint, verifies that the
selected instance has stopped, and writes its autostart-off marker while preserving
the registration. An unknown or mismatched instance is refused; stopping does not
fall back to a supervisor force-stop. See the ownership checks and legacy upgrade
steps below.

The Windows desktop app installs to `%LOCALAPPDATA%\Programs\Wenlan`:
`app/windows/installer-hooks.nsh` moves the NSIS per-user default off
`%LOCALAPPDATA%\Wenlan`, which is the CLI data root above on a case-insensitive
filesystem. A directory the user picks in the installer is kept, and so is the
directory of an install that predates the move when the in-app updater runs (that
path skips the old uninstaller and keeps the existing shortcuts); running the full
installer moves it.

In sidecar mode (no service registration) a quit or SIGTERM asks the daemon to shut
down over HTTP and kills it through the child handle if it has not released the port
within 3 s. On Windows the sidecar is also bound to a kill-on-close job object, so an
app crash takes it down too. That binding can fail (a restricted token, a job
assignment the OS refuses), and the outcome is recorded on the sidecar handle as
`daemon_start::JobBinding` rather than only logged: `sidecar_job_binding()` answers
`Bound`, `Unbound { reason }`, or `NotSupported`, and never assumes. Startup still
continues on `Unbound` — refusing to start would leave the user with no daemon at all
— but the quit path then verifies its kill against the recorded pid *and* process
start time and reissues it.

**Ending the daemon is best-effort, and which cases are covered is a measurement, not
a promise.** `daemon_start::stop_sidecar` returns a `SidecarStopOutcome` —
`Ended`, `StillRunning { reason }`, `CouldNotMeasure { reason }`, or `NoSidecar` —
and the last one is on the diagnostics wire as `daemon.last_sidecar_stop` (`null`
until the app has tried to stop one; `null` is not `ended`). Concretely:

| Case | Does the daemon end? |
|---|---|
| Windows, `JobBinding::Bound` | Yes, on every exit including a hard kill: the job object ends it when the app's last handle closes. This is the only categorical case. |
| Any platform, app code runs, the daemon answers the shutdown request | Yes, and it is measured: the port is released and the recorded process is confirmed gone. |
| Any platform, app code runs, the daemon must be killed | Usually — the child handle kill, plus a kill by pid when the binding did not take. The outcome says which. |
| `Unbound` **and** the sidecar's start time could not be captured | **No guarantee.** The process cannot be identified, so no kill by pid is issued (killing an unidentified pid is worse than not killing) and the result is `CouldNotMeasure`. |
| `Unbound` and the kill by pid failed | **No.** Reported as `StillRunning`; the next launch meets it as a held port. |
| Hard kill of the app (Task Manager, a crash, `Stop-Process`) while `Unbound`, or on macOS/Linux, which have no job object | **No.** No app code runs, so nothing ends the daemon. |

A launchd, systemd, or Task Scheduler daemon is service-owned and outlives the app by
design.

On macOS the app registers the launchd job first on a fresh install and starts its
own sidecar only when launchd does not end up with a loaded job for the selected
data root (registration skipped for an opted-out user, an unstable app path, or an
isolated data dir, or it failed), so the two never compete for the port. Every owner
decision is made and acted on under one lock, so startup and the "Start Wenlan"
button cannot both spawn a sidecar. Turning on "Run at Login" stops an app-owned
sidecar before it hands the daemon to launchd, and starts one again if the handover
fails.

`lifecycle::launchd_owns_server_daemon` answers `Owns` / `DoesNot` / `Unknown`, and
`Unknown` is a real third answer: `launchctl list` that will not run, exits nonzero,
or exits 0 without printing its `PID/Status/Label` table, or a server plist that
exists and cannot be read. Both owner decisions take a port-health measurement
first, so an unknown owner only ever spawns against a silent port — and when it does,
the app records it: `daemon.sidecar_spawned_on_unknown_owner` on the diagnostics wire
is `true` for the rest of that run.

## llama-cpp-2 backend

macOS builds use Metal, Windows x86_64 builds use Vulkan, and Linux builds remain CPU/OpenMP. The Windows CPU/OpenMP fallback was observed on 2026-07-25 on a machine with a working vendor ICD — forced CPU and an injected bad device index both reloaded the model with zero GPU layers and reported a `fallback_reason`. It has never been observed on a machine with **no** ICD, because that path sits downstream of `LlamaBackend::init()`; the Vulkan SDK is a build-time prerequisite only, but the end-user impact of a missing vendor driver is unproven in both directions. Windows setup, device selection, CI/release prerequisites, physical Qwen live-smoke commands, and the driverless-VM gate that would settle the end-user question are in [`windows-vulkan.md`](windows-vulkan.md).

## ORT (ONNX Runtime) on Windows

If you see `Failed to load onnxruntime.dll` or version-mismatch errors on Windows, set `ORT_DYLIB_PATH` to the bundled `onnxruntime.dll` inside the Wenlan install directory before starting the daemon. The bundled DLL ships in the Windows release zip.

## Manual Windows verification

### Legacy service upgrade

Lifecycle commands require the daemon to report the selected data-root identity.
They also check ownership of a globally named macOS/Linux registration before
controlling its supervisor. Process shutdown uses only the daemon's root-checked
HTTP endpoint; a failed, unresponsive, or mismatched endpoint never authorizes a
force-stop through the supervisor. Before starting or updating a registration,
the CLI independently requires an inactive runtime: Windows checks task state
and running-instance count, Linux checks ActiveState and MainPID, and macOS checks
the loaded job's state and effective launch root. Queued or unknown states refuse
control. A missing identity from an older daemon or an
unmarked old service is an explicit refusal, never permission to stop an unknown
instance. The one exception is a macOS daemon from before 0.20.4, which never
reports its identity: the CLI asks it to shut down only when the loaded
`com.wenlan.server` job is running with exactly one PID, its effective launch
root is the selected root, and that PID is the only listener on the daemon port;
the command still fails if the shutdown response names a different PID. New Linux registrations persist `WENLAN_DATA_DIR` and `WENLAN_BIND_ADDR`.

macOS updates require the previous job to be unloaded, even after its daemon has
exited. The CLI writes the confirmed plist and uses `bootstrap` without implicit
unload/remove behavior; starting a verified inactive loaded job uses `kickstart`
without `-k`. If an update reports that the old job is still loaded, inspect and
boot out that confirmed job using the steps below, then retry. Other root's loaded
configuration cannot be replaced merely by editing the plist on disk.

For a one-time upgrade, first inspect the existing service and confirm its
executable, arguments, and data directory against the store you intend to upgrade:

- Windows: Task Scheduler, or `schtasks /query /tn WenlanServer /xml`. Inspect
  custom `WenlanServer-*` tasks separately. Export the confirmed task XML as a
  backup, stop that task, then delete that stopped legacy task registration in
  Task Scheduler before reinstalling. Keep its data directory.
- macOS: inspect `~/Library/LaunchAgents/com.wenlan.server.plist` and
  `launchctl print gui/$(id -u)/com.wenlan.server`. After confirming the instance,
  boot it out with `launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.wenlan.server.plist`.
- Linux: inspect `systemctl --user cat wenlan-server.service` and
  `systemctl --user show wenlan-server.service -p Environment -p ExecStart -p MainPID`.
  After confirming the instance, use `systemctl --user stop wenlan-server.service`.

On macOS/Linux, back up the confirmed registration. If it has no reliable root marker, move that
registration out of its service directory (and run `systemctl --user daemon-reload`
on Linux). Then run `wenlan background on` with the intended absolute
`WENLAN_DATA_DIR` and loopback `WENLAN_BIND_ADDR`. Do not delete the data directory.
A manually started legacy daemon must be stopped from its own terminal or by its
verified PID before retrying. A root mismatch means the selected endpoint or
registration is wrong; do not stop it merely to silence the error.

The [Windows compatibility audit from 2026-10-28](windows-audit-2026-10-28.md)
records source-confirmed defects, isolated probe results, and the focused
regression gates needed before broader Windows runtime verification.

The CI matrix includes `windows-2022` for Windows-affecting PRs, but hosted runners do not prove physical GPU inference. Follow [`windows-vulkan.md`](windows-vulkan.md) on a real Windows 11 GPU machine and run all three Qwen live-smoke legs: Vulkan/device assertion, forced CPU, and injected CPU fallback.

A fourth leg is defined but **has never been run**: the same install on a clean Windows VM or Windows Sandbox with *no* vendor ICD, from the real signed installer or release ZIP, loading a real GGUF and asserting `/api/status` reports `backend=cpu` and `gpu_layers=0`. The three legs above all ran on a machine with working drivers, so none of them covers it. Until that leg runs, the driverless end-user path is unproven — do not assert either that users are unaffected or that they are affected.

## Linux smoke from macOS

```bash
bash scripts/smoke-linux.sh
```

Builds the multi-arch daemon image (linux/arm64 for native Apple Silicon speed via OrbStack / Docker Desktop), starts a container, exercises the HTTP API, asserts responses, tears down. Runtime ~3 minutes after the first build.

## Microsoft runtime on Windows

Windows installers and the release ZIP ship the x64 MSVC 14.x CRT and OpenMP
DLLs beside the app and three executables. A clean Windows machine does not
necessarily have these libraries: their absence prevents process startup with
`0xC0000135` before Wenlan can log an error or initialize WebView2.

`scripts/stage-msvc-runtime-windows.ps1` extracts Microsoft's signed Visual Studio
2022 x64 redistributable **14.44.35211.0**, pinned to an immutable Microsoft download
URL and SHA-256 `cc0ff0eb1dc3f5188ae6300faef32bf5beeba4bdd6e8e445a9184072096b713b`.
Staging checks the whole bundle's SHA-256 first, then reads the pinned Burn cabinet
slices, verifies their layout, and extracts the x64 runtime cabinet and original
English license using signed-manifest hashes. It never runs the EXE/MSI, copies from
System32, or installs a global runtime.

**Windows build prerequisites.** Every Windows `pnpm tauri dev` and `pnpm tauri build`
runs this staging through `scripts/prepare-sidecars.sh`, so a Windows build host
needs, besides Visual Studio's C++ build tools:

- `powershell.exe` (Windows PowerShell 5.1, present on every supported Windows).
- 7-Zip, either `7z.exe` on `PATH` or the default
  `C:\Program Files\7-Zip\7z.exe` (`winget install --id 7zip.7zip --exact`).
  GitHub's `windows-2022` runners already have it.
- Network access to `download.visualstudio.microsoft.com` for the first staging,
  unless a verified copy of the pinned `vc_redist.x64.exe` is supplied.

Later runs are local. When the destination (`app/binaries` for local builds) already
holds the ten DLLs, the license and an `MSVC-Runtime-Source.txt` written for the
current pin and compiler, and every SHA-256 recorded there still matches its file
and every DLL is still Microsoft-signed at the pinned version, staging reuses them
without 7-Zip or the network. A changed pin, a newer compiler, or a missing or
altered file restages from the bundle.

To keep the 25 MB bundle across worktrees or offline builds, set
`WENLAN_MSVC_REDIST_PATH` to a file path, for example
`%LOCALAPPDATA%\wenlan-build\vc_redist.x64.exe`. `prepare-sidecars.sh` passes it to
the staging script as `-ArchivePath`: an existing file is used instead of the
download after the same hash, signature and version checks, and a missing one is
written with the first download that passes them. The cache cannot override the pin.

The effective version floor is the newer of the local compiler and the prebuilt
ONNX Runtime dependency's deployment floor. ONNX Runtime 1.23.2 reports PE linker
14.40; that metadata does not identify the full compiler patch. We conservatively
require the pinned 14.44.35211.0 runtime for this prebuilt dependency, even when
Wenlan itself is compiled with VS2019 14.29. An older CRT can resolve every imported
symbol yet crash inside the newer STL's mutex initialization. Import/export-name
closure alone therefore does not establish ABI compatibility. Microsoft's
[binary compatibility requirement](https://learn.microsoft.com/en-us/cpp/porting/binary-compat-2015-2017)
applies to the newest toolset used by **any** component; the
[mutex documentation](https://learn.microsoft.com/en-us/cpp/standard-library/mutex-class-stl)
records the constexpr constructor change in VS2022 17.10.

Staging verifies the source bundle and every DLL's valid Microsoft Authenticode
signature, x64 PE architecture, and coherent pinned file version. The ten CRT/OpenMP
DLLs include `vcomp140.dll`; no MFC, debug, ARM64, or OneCore payload is staged.
`MSVC-Runtime-License.rtf` and `MSVC-Runtime-Source.txt` accompany every package with
the original license bytes, source URL, compiler/dependency floors, and SHA-256
receipts. Refresh the URL, hash, version, dependency floor, and pinned cabinet
offsets together when upgrading the runtime or prebuilt ONNX dependency. A local
compiler newer than the pin fails staging until that pin is reviewed and refreshed.
Run `powershell -NoProfile -File scripts/stage-msvc-runtime-windows.test.ps1` for
focused controls and real signed extraction. Native first-run verification must
still reach BGE initialization, daemon health, and the desktop welcome screen;
successful individual DLL loads or packaging checks alone are insufficient.

Build/release hosts must be licensed for the Visual Studio tools they use and
follow Microsoft's [redistribution terms](https://learn.microsoft.com/en-us/visualstudio/releases/2022/redistribution)
([VS2019 terms](https://learn.microsoft.com/en-us/visualstudio/releases/2019/redistribution)).
Microsoft documents [app-local deployment](https://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files)
and the [CRT/OpenMP redistributable folders](https://learn.microsoft.com/en-us/cpp/windows/determining-which-dlls-to-redistribute).
The copied Microsoft DLLs remain unmodified; each local Tauri build re-verifies the
staged copies as described above. Installer/ZIP inventory checks require the DLLs and
notices, and `scripts/release_targets.py` is the single list of the Windows release
payload that the bundle, workflows and tests are checked against.
