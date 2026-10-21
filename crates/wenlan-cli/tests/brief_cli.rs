// SPDX-License-Identifier: Apache-2.0

use assert_cmd::Command;
use predicates::prelude::*;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{mpsc, Arc, Mutex},
    thread,
};

#[derive(Debug)]
struct Recorded {
    method: String,
    path: String,
    body: String,
    session_header: Option<String>,
}

fn cli() -> Command {
    let mut command = Command::cargo_bin("wenlan").unwrap();
    command.env("WENLAN_NO_AUTOSTART", "1");
    command.env_remove("WENLAN_SESSION");
    command.env_remove("CODEX_THREAD_ID");
    command
}

fn response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn spawn_stub(responses: Vec<String>) -> (String, Arc<Mutex<Vec<Recorded>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = format!("http://{}", listener.local_addr().unwrap());
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let recorded_for_thread = Arc::clone(&recorded);
    thread::spawn(move || {
        for response in responses {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap().to_string();
            let path = parts.next().unwrap().to_string();
            let mut content_length = 0usize;
            let mut session_header = None;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line.to_lowercase().strip_prefix("x-wenlan-session:") {
                    session_header = Some(value.trim().to_string());
                }
                if let Some(value) = line
                    .strip_prefix("content-length:")
                    .or_else(|| line.strip_prefix("Content-Length:"))
                {
                    content_length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; content_length];
            reader.read_exact(&mut body).unwrap();
            recorded_for_thread.lock().unwrap().push(Recorded {
                method,
                path,
                body: String::from_utf8(body).unwrap(),
                session_header,
            });
            reader.get_mut().write_all(response.as_bytes()).unwrap();
        }
    });
    (host, recorded)
}

fn space_list() -> String {
    serde_json::json!([{
        "id": "space-id",
        "name": "work",
        "description": null,
        "suggested": false,
        "created_at": 1.0,
        "updated_at": 1.0,
        "memory_count": 0,
        "entity_count": 0,
        "sort_order": 0,
        "starred": false,
        "is_default": false
    }])
    .to_string()
}

fn brief_response() -> String {
    serde_json::json!({
        "state": "ready",
        "space": "work",
        "brief": {
            "space_id": "space-machine-id",
            "space": "work",
            "last_session_summary": "finished daemon routes",
            "last_handoff_at": 1,
            "version": 3,
            "active": [{
                "id": "item-machine-id",
                "text": "open the PR",
                "state": "active",
                "added_at": 1,
                "version": 2
            }],
            "backlog": []
        }
    })
    .to_string()
}

fn session_response() -> String {
    serde_json::json!({
        "id": "ses_test",
        "tool": "claude-code",
        "external_id": "abc-123",
        "first_write_at": 1,
        "last_write_at": 1
    })
    .to_string()
}

#[test]
fn brief_update_records_the_session_once_then_stamps_the_write() {
    let temp = tempfile::tempdir().unwrap();
    let update = temp.path().join("update.json");
    std::fs::write(
        &update,
        serde_json::json!({
            "space": "work",
            "caller_id": "handoff",
            "operation_id": "operation-1",
            "mutations": [{"kind": "add", "text": "publish PR", "state": "active"}]
        })
        .to_string(),
    )
    .unwrap();
    let receipt = serde_json::json!({
        "space": "work",
        "brief_version": 1,
        "applied": [],
        "conflicts": [],
        "projection_path": "/tmp/work.md",
        "warnings": []
    })
    .to_string();
    let (host, recorded) = spawn_stub(vec![response(&session_response()), response(&receipt)]);

    cli()
        .env("WENLAN_HOST", host)
        .env("WENLAN_SESSION", "claude-code:abc-123")
        .args([
            "--space",
            "work",
            "brief",
            "update",
            "--file",
            update.to_str().unwrap(),
        ])
        .assert()
        .success();

    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(
        (recorded[0].method.as_str(), recorded[0].path.as_str()),
        ("POST", "/api/sessions")
    );
    let announced: serde_json::Value = serde_json::from_str(&recorded[0].body).unwrap();
    assert_eq!(announced["session"], "claude-code:abc-123");
    assert!(announced["cwd"].is_string());
    assert_eq!(
        (recorded[1].method.as_str(), recorded[1].path.as_str()),
        ("PATCH", "/api/brief")
    );
    assert_eq!(
        recorded[1].session_header.as_deref(),
        Some("claude-code:abc-123")
    );
}

#[test]
fn brief_update_without_a_session_sends_no_header_and_no_announcement() {
    let temp = tempfile::tempdir().unwrap();
    let update = temp.path().join("update.json");
    std::fs::write(
        &update,
        serde_json::json!({
            "space": "work",
            "caller_id": "handoff",
            "operation_id": "operation-2",
            "mutations": [{"kind": "add", "text": "publish PR", "state": "active"}]
        })
        .to_string(),
    )
    .unwrap();
    let receipt = serde_json::json!({
        "space": "work",
        "brief_version": 1,
        "applied": [],
        "conflicts": [],
        "projection_path": "/tmp/work.md",
        "warnings": []
    })
    .to_string();
    let (host, recorded) = spawn_stub(vec![response(&receipt)]);

    cli()
        .env("WENLAN_HOST", host)
        .args([
            "--space",
            "work",
            "brief",
            "update",
            "--file",
            update.to_str().unwrap(),
        ])
        .assert()
        .success();

    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].path, "/api/brief");
    assert_eq!(recorded[0].session_header, None);
}

#[test]
fn brief_read_posts_topic_and_json_keeps_machine_ids() {
    let (host, recorded) = spawn_stub(vec![response(&space_list()), response(&brief_response())]);

    cli()
        .env("WENLAN_HOST", host)
        .args([
            "--space",
            "work",
            "--format",
            "json",
            "brief",
            "release gate",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("item-machine-id"))
        .stdout(predicate::str::contains("space-machine-id"));

    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(
        (recorded[1].method.as_str(), recorded[1].path.as_str()),
        ("POST", "/api/brief")
    );
    let body: serde_json::Value = serde_json::from_str(&recorded[1].body).unwrap();
    assert_eq!(body["topic"], "release gate");
}

#[test]
fn brief_human_output_omits_machine_ids() {
    let (host, _recorded) = spawn_stub(vec![response(&space_list()), response(&brief_response())]);

    cli()
        .env("WENLAN_HOST", host)
        .args(["--space", "work", "--format", "table", "brief"])
        .assert()
        .success()
        .stdout(predicate::str::contains("open the PR"))
        .stdout(predicate::str::contains("item-machine-id").not())
        .stdout(predicate::str::contains("space-machine-id").not());
}

#[test]
fn brief_update_uses_patch_with_the_typed_file() {
    let temp = tempfile::tempdir().unwrap();
    let update = temp.path().join("update.json");
    std::fs::write(
        &update,
        serde_json::json!({
            "space": "work",
            "caller_id": "handoff",
            "operation_id": "operation-1",
            "mutations": [{
                "kind": "add",
                "text": "publish PR",
                "state": "active"
            }]
        })
        .to_string(),
    )
    .unwrap();
    let receipt = serde_json::json!({
        "space": "work",
        "brief_version": 1,
        "applied": [],
        "conflicts": [],
        "projection_path": "/tmp/work.md",
        "warnings": []
    })
    .to_string();
    let (host, recorded) = spawn_stub(vec![response(&receipt)]);

    cli()
        .env("WENLAN_HOST", host)
        .args([
            "--space",
            "work",
            "brief",
            "update",
            "--file",
            update.to_str().unwrap(),
        ])
        .assert()
        .success();

    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        (recorded[0].method.as_str(), recorded[0].path.as_str()),
        ("PATCH", "/api/brief")
    );
    let body: serde_json::Value = serde_json::from_str(&recorded[0].body).unwrap();
    assert_eq!(body["operation_id"], "operation-1");
}

#[test]
fn invalid_update_fails_before_any_network_request() {
    let temp = tempfile::tempdir().unwrap();
    let update = temp.path().join("invalid.json");
    std::fs::write(&update, "{not json").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let host = format!("http://{}", listener.local_addr().unwrap());

    cli()
        .env("WENLAN_HOST", host)
        .args(["brief", "update", "--file", update.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid Brief update JSON"));

    let (sent, received) = mpsc::channel();
    thread::spawn(move || {
        thread::sleep(std::time::Duration::from_millis(100));
        sent.send(listener.accept().is_ok()).unwrap();
    });
    assert!(!received.recv().unwrap(), "invalid JSON sent a request");
}

#[test]
fn recall_uses_memory_search_instead_of_legacy_context() {
    let search = serde_json::json!({
        "results": [],
        "took_ms": 1.0
    })
    .to_string();
    let (host, recorded) = spawn_stub(vec![response(&space_list()), response(&search)]);

    cli()
        .env("WENLAN_HOST", host)
        .args(["--space", "work", "recall", "release gate"])
        .assert()
        .success();

    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[1].path, "/api/memory/search");
    assert!(recorded
        .iter()
        .all(|request| request.path != "/api/context"));
}

fn history_response() -> String {
    serde_json::json!({
        "space": "work",
        "entries": [{
            "version": 2,
            "created_at": 1_700_000_100,
            "caller_id": "handoff",
            "operation_id": "op-2",
            "summary": "second summary",
            "mutation_count": 1,
            "mutations": [{"mutation_index": 0, "kind": "add", "item_id": "item-x", "version": 1}]
        }]
    })
    .to_string()
}

#[test]
fn brief_history_gets_the_limited_list_in_the_resolved_space() {
    let (host, recorded) = spawn_stub(vec![response(&space_list()), response(&history_response())]);

    cli()
        .env("WENLAN_HOST", host)
        .args([
            "--space", "work", "--format", "table", "brief", "history", "-l", "5",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("second summary"))
        .stdout(predicate::str::contains("handoff"));

    let recorded = recorded.lock().unwrap();
    assert_eq!(
        (recorded[1].method.as_str(), recorded[1].path.as_str()),
        ("GET", "/api/brief/history?limit=5")
    );
}

#[test]
fn brief_history_json_keeps_the_typed_entries() {
    let (host, _recorded) =
        spawn_stub(vec![response(&space_list()), response(&history_response())]);

    cli()
        .env("WENLAN_HOST", host)
        .args(["--space", "work", "--format", "json", "brief", "history"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"operation_id\": \"op-2\""));
}

#[test]
fn brief_show_gets_one_version_and_prints_the_brief_layout() {
    let body = serde_json::json!({
        "entry": {
            "version": 1,
            "created_at": 1_700_000_000,
            "caller_id": "handoff",
            "operation_id": "op-1",
            "summary": "old summary",
            "mutation_count": 1,
            "mutations": []
        },
        "brief": serde_json::from_str::<serde_json::Value>(&brief_response()).unwrap()["brief"].clone()
    })
    .to_string();
    let (host, recorded) = spawn_stub(vec![response(&space_list()), response(&body)]);

    cli()
        .env("WENLAN_HOST", host)
        .args([
            "--space",
            "work",
            "--format",
            "table",
            "brief",
            "show",
            "--version",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Version 1"))
        .stdout(predicate::str::contains("open the PR"))
        .stdout(predicate::str::contains("item-machine-id").not());

    let recorded = recorded.lock().unwrap();
    assert_eq!(
        (recorded[1].method.as_str(), recorded[1].path.as_str()),
        ("GET", "/api/brief/history/1")
    );
}
