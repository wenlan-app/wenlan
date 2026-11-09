// SPDX-License-Identifier: Apache-2.0
//! Integration tests for `wenlan spaces` subcommands.
//!
//! Daemon behavior is covered by wenlan-server integration tests. These tests
//! keep the process-level CLI contract focused on argument parsing, including
//! the daemon-backed Default clear surface.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;

fn cli() -> Command {
    let mut command = Command::cargo_bin("wenlan").expect("origin binary built");
    command.env("WENLAN_NO_AUTOSTART", "1");
    command
}

// ---------------------------------------------------------------------------
// Argument parsing — all `spaces` subcommands must accept --help
// ---------------------------------------------------------------------------

#[test]
fn spaces_subcommands_help_exits_zero() {
    for args in [
        &["spaces", "--help"][..],
        &["spaces", "list", "--help"][..],
        &["spaces", "add", "--help"][..],
        &["spaces", "default", "--help"][..],
        &["spaces", "move", "--help"][..],
        &["spaces", "show", "--help"][..],
        &["spaces", "share-sync", "--help"][..],
    ] {
        cli().args(args).assert().success();
    }
}

#[test]
fn spaces_default_clear_conflicts_with_name() {
    cli()
        .args(["spaces", "default", "work", "--clear"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("cannot be used with"));
}

#[test]
fn global_space_and_all_spaces_conflict() {
    cli()
        .args(["--space", "work", "--all-spaces", "recall", "canary"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("cannot be used with"));
}

#[test]
fn global_agent_and_space_options_are_visible() {
    cli()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("--agent-name"))
        .stdout(predicates::str::contains("--all-spaces"))
        .stdout(predicates::str::contains("--space"));
}

#[test]
fn spaces_share_sync_needs_a_folder() {
    // `--name` may be left out: the daemon then uses the profile's display
    // name.
    cli()
        .args(["spaces", "share-sync", "Wedding", "--name", "Ana"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("--folder"));
}

/// The Space argument once shared clap's id with the global `--space` flag, so
/// every real `share-sync` call failed with "--space/--all-spaces are not
/// supported by this command" before reaching the daemon.
#[test]
fn spaces_share_sync_space_argument_is_not_taken_for_the_global_space_flag() {
    cli()
        .env("WENLAN_HOST", "http://127.0.0.1:9")
        .args([
            "spaces",
            "share-sync",
            "Wedding",
            "--folder",
            "/tmp/shared",
            "--name",
            "Ana",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("--space/--all-spaces").not());
}
