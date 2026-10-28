# Wenlan patch to tauri-plugin-updater 2.10.1

This directory contains the crates.io 2.10.1 source under its original licenses.
The Windows installer path checks `ShellExecuteW` for failure, based on upstream
[PR 3516](https://github.com/tauri-apps/plugins-workspace/pull/3516), and runs
the exit hook only after a successful launch. The latter is necessary because
Tauri's default hook destroys resources and hides windows and cannot be undone
when launching the installer fails. Installer arguments, verification, and the
successful exit path remain unchanged.

The macOS installer, helpers and corresponding tests are backported from the
published 2.13.2 crate, upstream [PR 3578](https://github.com/tauri-apps/plugins-workspace/pull/3578),
commit `2a9c29e004325db674fd904089749c38f4aac824`. The only API adaptation is
`self.context.run_on_main_thread` to the existing `self.run_on_main_thread` field.
It adds the upstream `PreviousAppNotRestored` error and a macOS libc dependency.
Where supported the bundles swap atomically; otherwise a failed replacement
restores the old bundle. If restoration also fails, the backup is kept and its
path is reported. Temporary directories stay on the app's filesystem, and the
installed bundle root remains accessible to other users (`0755`).

Directly adopting 2.13.2 requires Tauri ^2.12. Its objc2-core-foundation ^0.3.2
conflicts with sysinfo =0.38.3's =0.3.1 requirement. This backport preserves the
current dependency set while fixing the installer failure paths.

The app's signed-fixture macOS regression injects a replacement I/O failure in
a child-only dyld interposer, checks that the previous app was restored, and
retries the update successfully. It also rejects the restoration rename to
check that the backup survives and the error identifies it. It fails against
the unmodified 2.10.1 crate. Wenlan displays these error details in About Wenlan.
Upstream helper tests also cover atomic swap, fallback restore, privileged shell
quoting, and keeping the backup when restoration cannot finish.

Remove this override when the dependency conflict is resolved and upstream both
preserves macOS bundles on failure and runs the Windows exit hook only after
successful launch. Verify native macOS failure/retry/restart and actual Windows
NSIS update/restart and failure recovery before removing it.
