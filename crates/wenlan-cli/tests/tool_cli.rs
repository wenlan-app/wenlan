// SPDX-License-Identifier: Apache-2.0
//! `wenlan tool` and the commands built on it, against a scripted stand-in for
//! `wenlan-mcp`: a shell script that answers each request with the next canned
//! reply and records what it was sent. Unix only (the stand-in is `/bin/sh`);
//! the argument and formatting logic has platform-neutral unit tests.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};

/// A stand-in `wenlan-mcp`. Reply `n` (1-based) answers request id `n + 1`,
/// since id 1 is `initialize`.
fn stand_in(dir: &Path, replies: &[Value]) -> PathBuf {
    for (index, reply) in replies.iter().enumerate() {
        std::fs::write(
            dir.join(format!("reply-{}.json", index + 2)),
            reply.to_string(),
        )
        .unwrap();
    }
    let script = dir.join("wenlan-mcp");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
dir='{dir}'
echo "$@" > "$dir/argv"
echo "${{WENLAN_DEFAULT_SPACE:-}}" > "$dir/space"
id=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$dir/received"
  case "$line" in
    *'"id":'*) ;;
    *) continue ;;
  esac
  id=$((id + 1))
  if [ "$id" = 1 ]; then
    echo '{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":"2025-06-18","capabilities":{{}},"serverInfo":{{"name":"stand-in","version":"0"}}}}}}'
  elif [ -f "$dir/reply-$id.json" ]; then
    echo '{{"jsonrpc":"2.0","method":"notifications/message","params":{{}}}}'
    printf '{{"jsonrpc":"2.0","id":%s,"result":%s}}\n' "$id" "$(cat "$dir/reply-$id.json")"
  else
    printf '{{"jsonrpc":"2.0","id":%s,"error":{{"code":-32602,"message":"tool not found"}}}}\n' "$id"
  fi
done
"#,
            dir = dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn text(body: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": body }] })
}

fn cli(mcp: &Path) -> Command {
    let mut cmd = Command::cargo_bin("wenlan").unwrap();
    cmd.env("WENLAN_MCP_BIN", mcp)
        .env("WENLAN_NO_AUTOSTART", "1")
        .env("WENLAN_HOST", "http://127.0.0.1:9")
        .env_remove("WENLAN_AGENT_NAME")
        .env_remove("WENLAN_SPACE")
        .env_remove("WENLAN_DEFAULT_SPACE");
    cmd
}

/// The `tools/call` requests the stand-in received, in order.
fn calls(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(dir.join("received"))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|message| message["method"] == "tools/call")
        .map(|message| message["params"].clone())
        .collect()
}

#[test]
fn tool_list_prints_one_line_per_tool_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let mcp = stand_in(
        dir.path(),
        &[json!({ "tools": [
            { "name": "recall", "description": "Search memories by query. More text." },
            { "name": "capture", "description": "Capture a memory." },
        ] })],
    );
    cli(&mcp)
        .args(["--format", "table", "tool", "list"])
        .assert()
        .success()
        .stdout("capture  Capture a memory.\nrecall   Search memories by query.\n");
}

#[test]
fn ux_cli_1_tool_call_sends_typed_arguments_and_the_daemon_address() {
    let dir = tempfile::tempdir().unwrap();
    let mcp = stand_in(dir.path(), &[text("{\"count\":0,\"pages\":[]}")]);
    cli(&mcp)
        .args([
            "--format",
            "json",
            "--space",
            "Product",
            "tool",
            "call",
            "list_pages",
            "limit=5",
            "status=active",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"count\": 0"));

    let sent = calls(dir.path());
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["name"], "list_pages");
    assert_eq!(
        sent[0]["arguments"],
        json!({ "limit": 5, "status": "active" })
    );
    let argv = std::fs::read_to_string(dir.path().join("argv")).unwrap();
    assert!(argv.contains("--origin-url http://127.0.0.1:9"), "{argv}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("space"))
            .unwrap()
            .trim(),
        "Product"
    );
}

#[test]
fn ux_cli_4_a_tool_that_reports_an_error_fails_with_the_tools_own_sentence() {
    let dir = tempfile::tempdir().unwrap();
    let mut refusal = text("Delete operations are not available over remote connections.");
    refusal["isError"] = json!(true);
    let mcp = stand_in(dir.path(), &[refusal]);
    cli(&mcp)
        .args(["tool", "call", "delete_page", "page_id=page_a"])
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains(
            "not available over remote connections",
        ));
}

#[test]
fn an_unknown_tool_says_how_to_list_them() {
    let dir = tempfile::tempdir().unwrap();
    let mcp = stand_in(dir.path(), &[]);
    cli(&mcp)
        .args(["tool", "call", "nope"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("no tool named 'nope'")
                .and(predicate::str::contains("wenlan tool list")),
        );
}

#[test]
fn a_missing_wenlan_mcp_names_the_fix() {
    let dir = tempfile::tempdir().unwrap();
    cli(&dir.path().join("not-there"))
        .args(["tool", "list"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not-there"));
}

#[test]
fn pages_rename_finds_the_page_by_title_and_sends_its_current_version() {
    let dir = tempfile::tempdir().unwrap();
    let mcp = stand_in(
        dir.path(),
        &[
            text(&json!({ "count": 2, "pages": [
                { "page_id": "page_a", "title": "Install notes" },
                { "page_id": "page_b", "title": "Team plan" },
            ] }).to_string()),
            text(&json!({ "page": { "id": "page_a", "title": "Install notes", "version": 7 }, "sources": [] }).to_string()),
            text("Renamed page page_a to \"Install guide\" (version 8)."),
        ],
    );
    cli(&mcp)
        .args([
            "--format",
            "table",
            "pages",
            "rename",
            "install",
            "Install guide",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Renamed page page_a"));

    let sent = calls(dir.path());
    let names: Vec<&str> = sent
        .iter()
        .map(|call| call["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["list_pages", "read_page", "rename_page"]);
    assert_eq!(
        sent[2]["arguments"],
        json!({ "page_id": "page_a", "title": "Install guide", "expected_version": 7 })
    );
}

#[test]
fn pages_write_on_an_existing_page_keeps_its_sources_and_version() {
    let dir = tempfile::tempdir().unwrap();
    let mcp = stand_in(
        dir.path(),
        &[
            text(
                &json!({ "page": {
                "id": "page_a", "title": "Install notes", "version": 3,
                "source_memory_ids": ["mem_1", "mem_2"],
            }, "sources": [] })
                .to_string(),
            ),
            text("Refreshed page page_a"),
        ],
    );
    cli(&mcp)
        .args(["pages", "write", "page_a"])
        .write_stdin("New text.\n")
        .assert()
        .success();

    let sent = calls(dir.path());
    assert_eq!(sent[1]["name"], "write_page");
    assert_eq!(
        sent[1]["arguments"],
        json!({
            "content": "New text.\n",
            "page_id": "page_a",
            "expected_version": 3,
            "source_memory_ids": ["mem_1", "mem_2"],
        })
    );
}

#[test]
fn pages_write_without_a_page_creates_one_in_the_named_space() {
    let dir = tempfile::tempdir().unwrap();
    let mcp = stand_in(dir.path(), &[text("Created page page_n")]);
    cli(&mcp)
        .args(["--space", "Product", "pages", "write", "--title", "Roadmap"])
        .write_stdin("Body.\n")
        .assert()
        .success();
    assert_eq!(
        calls(dir.path())[0]["arguments"],
        json!({ "content": "Body.\n", "title": "Roadmap", "space": "Product" })
    );

    let empty = tempfile::tempdir().unwrap();
    let mcp = stand_in(empty.path(), &[]);
    cli(&mcp)
        .args(["pages", "write"])
        .write_stdin("Body.\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--title"));
}

#[test]
fn pages_delete_without_a_terminal_refuses_unless_yes() {
    let dir = tempfile::tempdir().unwrap();
    let page = text(&json!({ "page": { "id": "page_a", "title": "Install notes", "version": 1 }, "sources": [] }).to_string());
    let mcp = stand_in(dir.path(), std::slice::from_ref(&page));
    cli(&mcp)
        .args(["pages", "delete", "page_a"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("Delete the page \"Install notes\"?")
                .and(predicate::str::contains("--yes")),
        );
    assert!(calls(dir.path())
        .iter()
        .all(|call| call["name"] != "delete_page"));

    let dir = tempfile::tempdir().unwrap();
    let mcp = stand_in(dir.path(), &[page, text("Page page_a archived.")]);
    cli(&mcp)
        .args(["pages", "delete", "page_a", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("archived"));
    assert_eq!(calls(dir.path())[1]["name"], "delete_page");
}

#[test]
fn pages_move_makes_the_missing_folder_first() {
    let dir = tempfile::tempdir().unwrap();
    let mcp = stand_in(
        dir.path(),
        &[
            text(&json!({ "page": { "id": "page_a", "title": "Install notes", "version": 1, "storage_path": "install-notes.md" }, "sources": [] }).to_string()),
            text(&json!({ "folders": [{ "path": "guides" }], "truncated": false }).to_string()),
            text("{\"path\":\"guides/mac\"}"),
            text("{\"storage_path\":\"guides/mac/install-notes.md\"}"),
        ],
    );
    cli(&mcp)
        .args(["--format", "table", "pages", "move", "page_a", "guides/mac"])
        .assert()
        .success()
        .stdout("Moved \"Install notes\" to guides/mac/install-notes.md\n");

    let sent = calls(dir.path());
    let names: Vec<&str> = sent
        .iter()
        .map(|call| call["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["read_page", "list_folders", "create_folder", "move_page"]
    );
    assert_eq!(
        sent[2]["arguments"],
        json!({ "name": "mac", "parent_path": "guides" })
    );
    assert_eq!(sent[3]["arguments"]["folder_path"], "guides/mac");
    assert_eq!(
        sent[3]["arguments"]["expected_storage_path"],
        "install-notes.md"
    );
}

#[test]
fn memories_forget_asks_and_confirm_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let mcp = stand_in(dir.path(), &[]);
    cli(&mcp)
        .args(["memories", "forget", "mem_1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--yes"));

    let dir = tempfile::tempdir().unwrap();
    let mcp = stand_in(dir.path(), &[text("Memory mem_1 confirmed.")]);
    cli(&mcp)
        .args(["memories", "confirm", "mem_1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("confirmed"));
    assert_eq!(
        calls(dir.path())[0]["arguments"],
        json!({ "memory_id": "mem_1" })
    );
}

#[test]
fn sources_read_needs_a_space_and_sends_the_kind() {
    let dir = tempfile::tempdir().unwrap();
    let mcp = stand_in(dir.path(), &[text("the text")]);
    cli(&mcp)
        .args(["sources", "read", "src_1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--space"));

    cli(&mcp)
        .args([
            "--space", "Product", "sources", "read", "src_1", "--kind", "webpage", "--limit", "100",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("the text"));
    assert_eq!(
        calls(dir.path())[0]["arguments"],
        json!({ "source": "webpage", "source_id": "src_1", "space": "Product", "offset": 0, "limit": 100 })
    );
}

#[test]
fn ux_cli_2_help_keeps_advanced_commands_on_a_second_page() {
    let dir = tempfile::tempdir().unwrap();
    let mcp = stand_in(dir.path(), &[]);
    cli(&mcp).arg("--help").assert().success().stdout(
        predicate::str::contains("wenlan help all")
            .and(predicate::str::contains("\n  start "))
            .and(predicate::str::contains("\n  models ").not()),
    );
    cli(&mcp)
        .args(["help", "all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\n  models ").and(predicate::str::contains("\n  tool ")));
    // Earlier names keep working after a rename.
    cli(&mcp)
        .args(["team", "host", "--help"])
        .assert()
        .success();
    cli(&mcp)
        .args(["wiki", "status", "--help"])
        .assert()
        .success();
    cli(&mcp)
        .args(["background", "on", "--help"])
        .assert()
        .success();
    cli(&mcp)
        .args(["team", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\n  share "));
}
