// SPDX-License-Identifier: Apache-2.0
//! Terminal entry points for saving Space rules and managing personal setup.

mod common;

use assert_cmd::Command;
use common::spawn_stub;
use predicates::prelude::*;
use serde_json::json;
use std::{fs, path::Path};
use wenlan_types::rulesync::AdoptionPlan;

fn cli(base: &str) -> Command {
    let mut cmd = Command::cargo_bin("wenlan").unwrap();
    cmd.env("WENLAN_HOST", base)
        .env("WENLAN_NO_AUTOSTART", "1")
        .env_remove("WENLAN_AGENT_NAME")
        .env_remove("WENLAN_SPACE")
        .env_remove("WENLAN_DEFAULT_SPACE");
    cmd
}

fn personal_rules_cli(state: &Path) -> Command {
    let home = state.join("ai-tools/home");
    let mut cmd = cli("unreachable://no-daemon");
    cmd.env("HOME", &home)
        .env("CODEX_HOME", home.join(".codex"))
        .env("WENLAN_DEV_STATE_DIR", state)
        .env("WENLAN_DATA_DIR", state.join("unused-data-root"))
        .env_remove("WENLAN_RULESYNC")
        .env_remove("WENLAN_SESSION")
        .env_remove("CODEX_THREAD_ID");
    cmd
}

#[test]
fn personal_rules_dry_run_reads_existing_files_without_daemon_or_data_root() {
    let state = tempfile::tempdir().unwrap();
    let home = state.path().join("ai-tools/home");
    fs::create_dir_all(home.join(".agents")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    let master = home.join(".agents/AGENTS.md");
    let codex = home.join(".codex/AGENTS.md");
    fs::write(&master, "Keep shared rules.\n").unwrap();
    fs::write(&codex, "Keep Codex rules.\n").unwrap();

    let output = personal_rules_cli(state.path())
        .env("WENLAN_DEFAULT_SPACE", "Clinic")
        .args(["rules", "adopt", "--dry-run", "--format", "json"])
        .assert()
        .success();
    let plan: AdoptionPlan = serde_json::from_slice(&output.get_output().stdout).unwrap();
    let change = plan
        .files
        .iter()
        .find(|change| Path::new(&change.path) == Path::new(".codex/AGENTS.md"))
        .expect("Codex migration plan");
    assert_eq!((change.added, change.removed), (1, 0));
    assert!(change.diff.contains("+Keep shared rules."));
    assert_eq!(fs::read_to_string(master).unwrap(), "Keep shared rules.\n");
    assert_eq!(fs::read_to_string(codex).unwrap(), "Keep Codex rules.\n");
    assert!(!home.join(".claude").exists());
    assert!(!state.path().join("unused-data-root").exists());
}

#[test]
fn personal_rules_dry_run_labels_skill_sources() {
    let state = tempfile::tempdir().unwrap();
    let home = state.path().join("ai-tools/home");
    for path in [".agents/skills/locked", ".codex/skills/handmade"] {
        let skill = home.join(path);
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "Skill contents\n").unwrap();
    }
    fs::write(home.join(".agents/.skill-lock.json"), br#"{"skills":{"locked":{
        "sourceType":"github","source":"owner/project","sourceUrl":"https://github.com/owner/project.git",
        "skillPath":"library/locked/SKILL.md"
    }}}"#).unwrap();
    personal_rules_cli(state.path())
        .args(["rules", "adopt", "--dry-run", "--format", "table"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            ".agents/skills/locked — tracked upstream from GitHub owner/project (library/locked)",
        ))
        .stdout(predicate::str::contains(
            ".codex/skills/handmade — plain copy in the personal master",
        ));
    assert!(!state.path().join("unused-data-root").exists());
}

#[test]
fn personal_rules_setup_refuses_the_strict_space_pin() {
    let state = tempfile::tempdir().unwrap();
    personal_rules_cli(state.path())
        .env("WENLAN_SPACE", "Clinic")
        .args(["rules", "adopt", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "WENLAN_SPACE limits this terminal",
        ));
}

#[test]
fn personal_rules_setup_requires_interactive_confirmation() {
    let state = tempfile::tempdir().unwrap();
    for args in [
        &["rules", "adopt"][..],
        &["rules", "restore"],
        &["rules", "settings", "local-settings"],
        &[
            "rules",
            "review",
            "card-1",
            "all-tools",
            "--file",
            "rules.md",
        ],
    ] {
        personal_rules_cli(state.path())
            .args(args)
            .write_stdin("yes\n")
            .assert()
            .failure()
            .stderr(predicate::str::contains("from your own terminal"));
        personal_rules_cli(state.path())
            .args(args)
            .arg("--yes")
            .assert()
            .failure()
            .stderr(predicate::str::contains("unexpected argument '--yes'"));
    }
}

#[test]
fn personal_rules_settings_preview_refuses_non_local_daemon() {
    let state = tempfile::tempdir().unwrap();
    personal_rules_cli(state.path())
        .env("WENLAN_HOST", "http://192.0.2.1:17917")
        .args(["rules", "settings", "local-settings", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("daemon on this computer"));
    assert!(!state.path().join("unused-data-root").exists());
}

fn saved() -> String {
    json!({
        "status": "saved",
        "rule_id": "rule0123456789abcdef",
        "space_name": "Clinic",
        "page_id": "page_1",
        "page_title": "Rules I saved — Clinic",
        "saved_from": "terminal",
        "message": "Saved for the Space \"Clinic\". Your AI tools are told this rule from now on.",
    })
    .to_string()
}

/// The Spaces the stand-in daemon knows; the CLI asks before every command
/// that works in a Space.
fn spaces() -> (&'static str, String) {
    (
        "GET /api/spaces",
        json!([{
            "id": "space_1", "name": "Clinic", "description": null,
            "suggested": false, "starred": false, "sort_order": 0,
            "memory_count": 0, "entity_count": 0, "created_at": 0.0, "updated_at": 0.0,
        }])
        .to_string(),
    )
}

/// What the command sent, after asking for the Spaces.
fn sent(
    received: &std::sync::mpsc::Receiver<(String, String)>,
) -> Vec<(String, serde_json::Value)> {
    received
        .try_iter()
        .filter(|(route, _)| route != "GET /api/spaces")
        .map(|(route, body)| {
            let body = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
            (route, body)
        })
        .collect()
}

/// UX-CLI-1: `wenlan rules add` sends the daemon the same request on the
/// same route as an AI tool's `add_rule` (its twin test is in
/// `wenlan-mcp/src/rules_tool_tests.rs`). Only the words asked differ, and
/// where the Space travels: here as the connection's Space, like every
/// other command.
#[test]
fn ux_cli_1_rules_add_matches_add_rule() {
    let (base, received) = spawn_stub(vec![spaces(), ("POST /api/skills/rules", saved())]);
    cli(&base)
        .args(["--format", "table", "--space", "Clinic"])
        .args(["rules", "add", "We release on Tuesdays."])
        .assert()
        .success()
        .stdout("Saved for the Space \"Clinic\". Your AI tools are told this rule from now on.\n");
    assert_eq!(
        sent(&received),
        vec![(
            "POST /api/skills/rules".to_string(),
            json!({
                "text": "We release on Tuesdays.",
                "person_asked": "wenlan rules add \"We release on Tuesdays.\"",
            })
        )]
    );
}

/// UX-SKILL-14: a rule is for every Space only by its own flag, and the
/// flag and a named Space together are refused before anything is sent.
#[test]
fn ux_skill_14_every_space_is_only_ever_explicit() {
    let (base, received) = spawn_stub(vec![spaces(), ("POST /api/skills/rules", saved())]);
    cli(&base)
        .args(["rules", "add", "Answer in plain words.", "--every-space"])
        .assert()
        .success();
    assert_eq!(
        sent(&received),
        vec![(
            "POST /api/skills/rules".to_string(),
            json!({
                "text": "Answer in plain words.",
                "every_space": true,
                "person_asked": "wenlan rules add \"Answer in plain words.\" --every-space",
            })
        )]
    );

    let (base, received) = spawn_stub(vec![spaces(), ("POST /api/skills/rules", saved())]);
    cli(&base)
        .args(["--space", "Clinic"])
        .args(["rules", "add", "Answer in plain words.", "--every-space"])
        .assert()
        .failure();
    assert!(sent(&received).is_empty(), "nothing was sent");
}

/// UX-SKILL-14: `WENLAN_SPACE` limits this terminal to one Space. A rule
/// for every Space is then refused before anything is sent; a rule for that
/// Space goes out under the Space, so the daemon holds it there. Without the
/// limit, a rule for every Space carries no Space at all, whatever Space
/// the folder or the default would give.
#[test]
fn ux_skill_14_every_space_is_refused_under_the_space_lock() {
    let (base, received) = spawn_stub(vec![spaces(), ("POST /api/skills/rules", saved())]);
    cli(&base)
        .env("WENLAN_SPACE", "Clinic")
        .args(["rules", "add", "Answer in plain words.", "--every-space"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "WENLAN_SPACE limits this terminal to the Space 'Clinic'",
        ))
        .stderr(predicate::str::contains("Nothing was saved"));
    assert!(
        received.try_iter().next().is_none(),
        "nothing at all was sent"
    );

    // For its own Space the locked terminal saves, and says which Space.
    let (base, received) = spawn_headers_stub("POST /api/skills/rules", saved());
    cli(&base)
        .env("WENLAN_SPACE", "Clinic")
        .args(["rules", "add", "We release on Tuesdays."])
        .assert()
        .success();
    let headers = received.recv().expect("the save");
    assert!(headers.contains("x-wenlan-space: clinic"), "{headers}");

    // Not locked: every Space carries no Space, even with a default set.
    let (base, received) = spawn_headers_stub("POST /api/skills/rules", saved());
    cli(&base)
        .env("WENLAN_DEFAULT_SPACE", "Clinic")
        .args(["rules", "add", "Answer in plain words.", "--every-space"])
        .assert()
        .success();
    let headers = received.recv().expect("the save");
    assert!(!headers.contains("x-wenlan-space"), "{headers}");
}

/// A stand-in daemon that answers `GET /api/spaces` and one other route,
/// and hands back that other request's headers, lower-cased.
fn spawn_headers_stub(
    route: &'static str,
    payload: String,
) -> (String, std::sync::mpsc::Receiver<String>) {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind stub");
    let base = format!("http://{}", listener.local_addr().expect("stub address"));
    let (sent, received) = std::sync::mpsc::channel();
    let spaces = spaces().1;
    std::thread::spawn(move || {
        for stream in listener.incoming().take(2).flatten() {
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).expect("request line");
            let (mut headers, mut length) = (String::new(), 0usize);
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("header");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                let line = line.to_lowercase();
                if let Some(value) = line.strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                headers.push_str(&line);
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).expect("body");
            let is_route = request_line.starts_with(route);
            let payload = if is_route { &payload } else { &spaces };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            if is_route {
                let _ = sent.send(headers);
            }
            reader
                .get_mut()
                .write_all(response.as_bytes())
                .expect("answer");
            if is_route {
                break;
            }
        }
    });
    (base, received)
}

/// UX-SKILL-14: removing is one command with the id `wenlan rules` prints.
#[test]
fn ux_skill_14_rules_remove_asks_the_daemon_to_remove_that_rule() {
    let removed = json!({
        "status": "removed",
        "id": "rule0123456789abcdef",
        "page_id": "page_1",
        "page_title": "Rules I saved — Clinic",
        "message": "Removed. Your AI tools are no longer told this rule.",
    })
    .to_string();
    let (base, received) = spawn_stub(vec![
        spaces(),
        (
            "POST /api/skills/rules/rule0123456789abcdef/remove",
            removed,
        ),
    ]);
    cli(&base)
        .args(["--format", "table"])
        .args(["rules", "remove", "rule0123456789abcdef"])
        .assert()
        .success()
        .stdout("Removed. Your AI tools are no longer told this rule.\n");
    assert_eq!(
        sent(&received),
        vec![(
            "POST /api/skills/rules/rule0123456789abcdef/remove".to_string(),
            json!({"person_asked": "wenlan rules remove rule0123456789abcdef"})
        )]
    );

    // Something that is not an id never reaches the daemon.
    let (base, received) = spawn_stub(vec![
        spaces(),
        ("POST /api/skills/rules/x/remove", String::new()),
    ]);
    cli(&base)
        .args(["rules", "remove", "../pages"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a rule id"));
    assert!(sent(&received).is_empty(), "nothing was sent");
}

/// Plain `wenlan rules` still reads the rules, and a saved rule says where
/// it was saved from and how to take it out (UX-SKILL-14).
#[test]
fn ux_skill_14_plain_rules_lists_a_saved_rule_with_where_it_came_from() {
    let rules = json!({
        "space_id": "space_1",
        "space_name": "Clinic",
        "lines": [{
            "id": "rule0123456789abcdef",
            "kind": "rule",
            "text": "We release on Tuesdays.",
            "source_page_id": "page_1",
            "source_page_title": "Rules I saved — Clinic",
            "source_revision": 2,
            "source_revision_at": 1_794_000_000,
            "source_quote": "We release on Tuesdays.",
            "saved_from": "Codex",
        }, {
            "id": "rule1123456789abcdef",
            "kind": "rule",
            "text": "Answer in plain words.",
            "source_page_id": "page_2",
            "source_page_title": "Rules I saved",
            "source_revision": 1,
            "source_revision_at": 1_794_000_000,
            "source_quote": "Answer in plain words.",
            "general": true,
            "saved_from": "terminal",
        }],
    })
    .to_string();
    let (base, received) = spawn_stub(vec![spaces(), ("GET /api/skills/rules", rules)]);
    cli(&base)
        .args(["--format", "table", "rules"])
        .assert()
        .success()
        .stdout(
            "Working rules for Clinic\n\n\
             - We release on Tuesdays.\n  \
             saved from Codex, kept in \"Rules I saved — Clinic\"\n  \
             to remove it: wenlan rules remove rule0123456789abcdef\n\
             - Answer in plain words.\n  \
             saved from the terminal, kept in \"Rules I saved\", used in every Space\n  \
             to remove it: wenlan rules remove rule1123456789abcdef\n",
        )
        .stdout(predicate::str::contains("written by you").not());
    assert_eq!(sent(&received)[0].0, "GET /api/skills/rules");
}
