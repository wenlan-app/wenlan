// SPDX-License-Identifier: Apache-2.0

/// A relocated nextest archive (the macOS CI slices) runs the binary from
/// NEXTEST_BIN_EXE_wenlan; the compile-time Cargo path no longer exists there.
fn wenlan_binary() -> std::ffi::OsString {
    std::env::var_os("NEXTEST_BIN_EXE_wenlan")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_wenlan").into())
}

/// Unix: shrink the main-thread stack with `ulimit -s`. Windows: the main
/// thread already has the 1 MiB default this test guards against.
fn version_command() -> std::process::Command {
    #[cfg(unix)]
    {
        let mut command = std::process::Command::new("sh");
        command
            .args(["-c", "ulimit -s 512 && exec \"$0\" --version"])
            .arg(wenlan_binary());
        command
    }
    #[cfg(not(unix))]
    {
        let mut command = std::process::Command::new(wenlan_binary());
        command.arg("--version");
        command
    }
}

#[test]
fn version_runs_with_a_small_main_thread_stack() {
    let output = version_command()
        .env("WENLAN_NO_AUTOSTART", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "status: {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains(env!("CARGO_PKG_VERSION")));
}
