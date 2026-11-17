// SPDX-License-Identifier: Apache-2.0
//! `wenlan rules add` and `wenlan rules remove` against a scripted stand-in
//! for the daemon.

mod common;

use assert_cmd::Command;
use common::spawn_stub;
use predicates::prelude::*;
use serde_json::json;

fn cli(base: &str) -> Command {
    let mut cmd = Command::cargo_bin("wenlan").unwrap();
    cmd.env("WENLAN_HOST", base)
        .env("WENLAN_NO_AUTOSTART", "1")
        .env_remove("WENLAN_AGENT_NAME")
        .env_remove("WENLAN_SPACE")
        .env_remove("WENLAN_DEFAULT_SPACE");
    cmd
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
