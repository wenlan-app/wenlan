# Wenlan patch to tauri-plugin-updater 2.10.1

This directory contains the crates.io 2.10.1 source under its original licenses.
The Windows installer path checks `ShellExecuteW` for failure, based on upstream
[PR 3516](https://github.com/tauri-apps/plugins-workspace/pull/3516), and runs
the exit hook only after a successful launch. The latter is necessary because
Tauri's default hook destroys resources and hides windows and cannot be undone
when launching the installer fails. Installer arguments, verification, and the
successful exit path remain unchanged.

Remove this override when upstream both reports launch failures and runs the
exit hook only after successful launch. Verify actual NSIS update/restart and
failure recovery before removing it.
