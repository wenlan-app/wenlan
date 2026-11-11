// SPDX-License-Identifier: Apache-2.0
//! Argument-parsing tests for `wenlan team`. Daemon behavior is covered by
//! wenlan-server tests; nothing here reaches a daemon.

use assert_cmd::Command;

fn cli() -> Command {
    let mut command = Command::cargo_bin("wenlan").expect("wenlan binary built");
    command.env("WENLAN_NO_AUTOSTART", "1");
    command
}

#[test]
fn team_subcommands_help_exits_zero() {
    for args in [
        &["team", "--help"][..],
        &["team", "status", "--help"][..],
        &["team", "add", "--help"][..],
        &["team", "update", "--help"][..],
        &["team", "remove", "--help"][..],
        &["team", "new-key", "--help"][..],
        &["team", "host", "--help"][..],
        &["team", "join", "--help"][..],
    ] {
        cli().args(args).assert().success();
    }
}

#[test]
fn team_add_accepts_repeated_spaces_with_read_suffix() {
    // Parsing succeeds; the run then fails only because no daemon answers.
    let out = cli()
        .env("WENLAN_HOST", "http://127.0.0.1:1")
        .args([
            "team",
            "add",
            "Ana",
            "--access",
            "Work",
            "--access",
            "Notes:read",
            "--url",
            "https://wenlan.example.com",
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).to_string();
    assert!(!stderr.contains("unexpected argument"), "{stderr}");
    assert!(!stderr.contains("invalid value"), "{stderr}");
}

#[test]
fn team_add_rejects_empty_space_name() {
    cli()
        .args(["team", "add", "Ana", "--access", ":read"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("the Space name is empty"));
}

#[test]
fn team_add_and_remove_require_their_arguments() {
    cli().args(["team", "add"]).assert().failure();
    cli().args(["team", "remove"]).assert().failure();
    cli().args(["team", "new-key"]).assert().failure();
    cli().args(["team", "host"]).assert().failure();
    cli().args(["team", "join"]).assert().failure();
}

#[test]
fn team_update_without_changes_is_refused_before_any_call() {
    cli()
        .args(["team", "update", "abc"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("nothing to change"));
}

#[test]
fn team_join_as_requires_space() {
    cli()
        .args(["team", "join", "https://x#wlt_a", "--as", "Local"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("--server-space"));
}
