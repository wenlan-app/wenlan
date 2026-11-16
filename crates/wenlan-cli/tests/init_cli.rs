// SPDX-License-Identifier: Apache-2.0
//! Integration tests for `wenlan init` and the folder lines of `wenlan status`.
//!
//! A scripted stub stands in for the daemon. The tests run with no terminal,
//! so `init` asks nothing: it does what needs no answer and names the command
//! for the rest.

mod common;

use assert_cmd::Command;
use common::spawn_stub;
use serde_json::json;
use std::path::{Path, PathBuf};

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A git repository in a folder called `name`, and an empty home folder
/// beside it so no real `~/.wenlan/spaces.toml` or `~/.cursor` is read.
fn repository(name: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let repo = dir.path().join(name);
    let home = dir.path().join("home");
    std::fs::create_dir_all(&repo).expect("repo dir");
    std::fs::create_dir_all(&home).expect("home dir");
    git(&repo, &["init", "--quiet", "-b", "main"]);
    (dir, repo, home)
}

fn run(host: &str, cwd: &Path, home: &Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::cargo_bin("wenlan")
        .expect("wenlan binary built")
        .env("WENLAN_NO_AUTOSTART", "1")
        .env("WENLAN_HOST", host)
        .env("HOME", home)
        .env("USERPROFILE", home)
        // No GitHub CLI: whose repository it is stays unknown, with no network.
        .env("WENLAN_GH_BIN", home.join("no-gh"))
        .env_remove("WENLAN_SPACE")
        .env_remove("WENLAN_DEFAULT_SPACE")
        .current_dir(cwd)
        .args(["--format", "table"])
        .args(args)
        .output()
        .expect("run wenlan");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn space(name: &str) -> serde_json::Value {
    json!({
        "id": name, "name": name, "description": null, "suggested": false,
        "starred": false, "is_default": false, "sort_order": 0,
        "memory_count": 0, "entity_count": 0, "created_at": 0.0, "updated_at": 0.0,
    })
}

fn binding(space: &str, repo_root: &Path) -> String {
    json!({
        "space": space,
        "repo_root": repo_root.to_string_lossy(),
        "folder": "wiki",
    })
    .to_string()
}

/// `wenlan status` first asks the daemon for its health and for Steep. The
/// stub serves one request per route it was given, so each of those two
/// needs a slot; this one matches neither and answers "no such route".
const STATUS_CALL: (&str, String) = ("(health or Steep)", String::new());

/// The repository's top level as `wenlan` reads it.
fn top(repo: &Path) -> PathBuf {
    std::fs::canonicalize(repo).expect("canonical repo path")
}

#[test]
fn ux_cli_5_init_makes_the_space_ties_the_repository_and_says_what_comes_next() {
    let (_dir, repo, home) = repository("acme-api");
    let (host, requests) = spawn_stub(vec![
        ("GET /api/spaces", "[]".to_string()),
        ("POST /api/spaces", "{}".to_string()),
        ("GET /api/spaces/acme-api/wiki/repo", "null".to_string()),
        (
            "PUT /api/spaces/acme-api/wiki/repo",
            binding("acme-api", &top(&repo)),
        ),
        ("GET /api/agents", "[]".to_string()),
    ]);

    let (ok, stdout, stderr) = run(&host, &repo, &home, &["init"]);

    assert!(ok, "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        stdout.contains("Made Space 'acme-api' for this repository."),
        "{stdout}"
    );
    assert!(stdout.contains("Tied it to"), "{stdout}");
    assert!(
        stdout.contains(
            "the terminal and AI tools started in this folder use Space 'acme-api'. `wenlan status` shows it."
        ),
        "{stdout}"
    );
    let served: Vec<(String, String)> = requests.try_iter().collect();
    let made = served
        .iter()
        .find(|(route, _)| route == "POST /api/spaces")
        .expect("the Space is made");
    assert!(made.1.contains("\"acme-api\""), "{}", made.1);
    let tied = served
        .iter()
        .find(|(route, _)| route == "PUT /api/spaces/acme-api/wiki/repo")
        .expect("the repository is tied");
    assert!(tied.1.contains("repo_root"), "{}", tied.1);
    // No daily run is turned on by setting a repository up.
    assert!(!tied.1.contains("daily"), "{}", tied.1);
}

#[test]
fn init_run_again_changes_nothing() {
    let (_dir, repo, home) = repository("acme-api");
    let (host, requests) = spawn_stub(vec![
        (
            "GET /api/spaces",
            json!([space("acme-api"), space("other")]).to_string(),
        ),
        (
            "GET /api/spaces/acme-api/wiki/repo",
            binding("acme-api", &top(&repo)),
        ),
        ("GET /api/agents", "[]".to_string()),
    ]);

    let (ok, stdout, stderr) = run(&host, &repo, &home, &["init"]);

    assert!(ok, "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        stdout.contains("Space 'acme-api' is already set up for this folder."),
        "{stdout}"
    );
    assert!(stdout.contains("It is already tied to"), "{stdout}");
    let served: Vec<String> = requests.try_iter().map(|(route, _)| route).collect();
    assert!(
        served.iter().all(|route| route.starts_with("GET ")),
        "a second run wrote something: {served:?}"
    );
}

#[test]
fn init_does_not_tie_a_repository_that_is_not_yours_without_a_yes() {
    let (_dir, repo, home) = repository("acme-api");
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme-co/acme-api.git",
        ],
    );
    let (host, requests) = spawn_stub(vec![
        ("GET /api/spaces", "[]".to_string()),
        ("POST /api/spaces", "{}".to_string()),
        ("GET /api/spaces/acme-api/wiki/repo", "null".to_string()),
        ("GET /api/agents", "[]".to_string()),
    ]);

    let (ok, stdout, stderr) = run(&host, &repo, &home, &["init"]);

    assert!(ok, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("Made Space 'acme-api'"), "{stdout}");
    assert!(
        stdout.contains(
            "Not tied to the repository yet: github.com/acme-co/acme-api belongs to acme-co, \
             and pull requests would open there. `wenlan repo bind --confirm-repo acme-co/acme-api` ties it."
        ),
        "{stdout}"
    );
    let served: Vec<String> = requests.try_iter().map(|(route, _)| route).collect();
    assert!(
        !served.iter().any(|route| route.starts_with("PUT ")),
        "the repository was tied without a yes: {served:?}"
    );
}

#[test]
fn ux_cli_4_init_outside_a_repository_names_the_command_that_fits() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("home dir");

    // Nothing listens here: the refusal comes before any call.
    let (ok, _stdout, stderr) = run("http://127.0.0.1:9", dir.path(), &home, &["init"]);

    assert!(!ok);
    assert!(
        stderr.contains("this folder is not inside a git repository"),
        "{stderr}"
    );
    assert!(stderr.contains("`wenlan spaces add <name>`"), "{stderr}");
}

#[test]
fn ux_cli_5_status_shows_the_space_and_repository_of_this_folder() {
    let (_dir, repo, home) = repository("acme-api");
    let (host, _requests) = spawn_stub(vec![
        STATUS_CALL,
        STATUS_CALL,
        ("GET /api/spaces", json!([space("acme-api")]).to_string()),
        (
            "GET /api/spaces/acme-api/wiki/repo",
            binding("acme-api", &top(&repo)),
        ),
    ]);

    let (_ok, stdout, _stderr) = run(&host, &repo, &home, &["status"]);

    assert!(stdout.contains("This folder"), "{stdout}");
    assert!(stdout.contains("  Space: acme-api"), "{stdout}");
    assert!(stdout.contains("  Repository: "), "{stdout}");
}

#[test]
fn status_in_a_repository_wenlan_does_not_know_names_init() {
    let (_dir, repo, home) = repository("acme-api");
    let (host, _requests) = spawn_stub(vec![
        STATUS_CALL,
        STATUS_CALL,
        ("GET /api/spaces", json!([space("other")]).to_string()),
    ]);

    let (_ok, stdout, _stderr) = run(&host, &repo, &home, &["status"]);

    assert!(
        stdout.contains("  Not in Wenlan yet. `wenlan init` sets it up."),
        "{stdout}"
    );
}

/// UX-CLI-2 with the new command: `init` is on the first help page, and
/// `spaces` and `repo`, which it stands in for, moved to the second.
#[test]
fn ux_cli_2_init_is_on_the_first_help_page_and_spaces_and_repo_on_the_second() {
    let help = |args: &[&str]| -> String {
        let output = Command::cargo_bin("wenlan")
            .expect("wenlan binary built")
            .env("WENLAN_NO_AUTOSTART", "1")
            .args(args)
            .output()
            .expect("run wenlan");
        assert!(output.status.success(), "wenlan {args:?} failed");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    let listed = |page: &str, name: &str| page.contains(&format!("\n  {name} "));

    let first = help(&["--help"]);
    assert!(listed(&first, "init"), "{first}");
    assert!(!listed(&first, "spaces"), "{first}");
    assert!(!listed(&first, "repo"), "{first}");

    let all = help(&["help", "all"]);
    for name in ["init", "spaces", "repo"] {
        assert!(listed(&all, name), "{name} is missing:\n{all}");
    }
    // Still there under their own names.
    help(&["spaces", "list", "--help"]);
    help(&["repo", "status", "--help"]);
}
