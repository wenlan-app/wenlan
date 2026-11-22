// SPDX-License-Identifier: Apache-2.0

#[cfg(unix)]
#[test]
fn version_runs_with_a_small_main_thread_stack() {
    let output = std::process::Command::new("sh")
        .args([
            "-c",
            "ulimit -s 512 && exec \"$0\" --version",
            env!("CARGO_BIN_EXE_wenlan"),
        ])
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
