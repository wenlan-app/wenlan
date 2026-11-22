// SPDX-License-Identifier: Apache-2.0
//! The daemon runs on its own 8 MiB thread, so a small main-thread stack (the
//! Windows default is 1 MiB) cannot overflow in debug builds.

#[allow(dead_code)]
#[path = "common/daemon_binary.rs"]
mod daemon_binary;

#[cfg(unix)]
#[test]
fn version_runs_with_a_small_main_thread_stack() {
    let output = std::process::Command::new("sh")
        .args(["-c", "ulimit -s 256 && exec \"$0\" --version"])
        .arg(daemon_binary::wenlan_server_binary())
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
