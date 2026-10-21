// SPDX-License-Identifier: Apache-2.0

mod common;

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use serde_json::{json, Value};
use tower::ServiceExt;
use wenlan_core::db::sessions::session_row_id;
use wenlan_server::router::AppRouter;
use wenlan_types::{SessionRef, SESSION_HEADER};

const SESSION_A: &str = "claude-code:00893aaf-19fa-41d2-8238-13269b9b3ca0";
const SESSION_B: &str = "codex:019a-thread";

fn session_id(session: &str) -> String {
    session_row_id(&SessionRef::parse(session).unwrap())
}

async fn send(
    app: &AppRouter,
    method: Method,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn store_memory(app: &AppRouter, content: &str, session: Option<&str>) -> String {
    let mut headers = Vec::new();
    if let Some(session) = session {
        headers.push((SESSION_HEADER, session));
    }
    let (status, body) = send(
        app,
        Method::POST,
        "/api/memory/store",
        Some(json!({"content": content, "memory_type": "fact"})),
        &headers,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["source_id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn store_with_session_header_stamps_the_memory() {
    let (app, _temp, db) = common::test_app_no_gate().await;
    let source_id = store_memory(&app, "stamped by a claude code session", Some(SESSION_A)).await;

    let (status, detail) = send(
        &app,
        Method::GET,
        &format!("/api/sessions/{}", session_id(SESSION_A)),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["session"]["tool"], "claude-code");
    assert_eq!(detail["memories"][0]["source_id"], source_id.as_str());

    let memory = db.get_memory_detail(&source_id).await.unwrap().unwrap();
    assert_eq!(
        memory.session_id.as_deref(),
        Some(session_id(SESSION_A).as_str())
    );
}

#[tokio::test]
async fn malformed_session_header_still_stores_the_memory() {
    let (app, _temp, db) = common::test_app_no_gate().await;
    let source_id = store_memory(&app, "stored despite a bad header", Some("not a session")).await;

    let memory = db.get_memory_detail(&source_id).await.unwrap().unwrap();
    assert_eq!(memory.session_id, None);
    let (status, _) = send(
        &app,
        Method::GET,
        &format!("/api/sessions/{}", session_id("not-a-session:x")),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn upsert_overwrites_present_fields_and_is_not_a_write() {
    let (app, _temp, _db) = common::test_app_no_gate().await;
    let id = session_id(SESSION_A);

    let (status, first) = send(
        &app,
        Method::POST,
        "/api/sessions",
        Some(json!({
            "session": SESSION_A,
            "cwd": "/work/wenlan",
            "repo_branch": "main",
            "transcript_path": "/home/u/.claude/projects/x/y.jsonl"
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["id"], id.as_str());
    assert_eq!(first["cwd"], "/work/wenlan");

    let (_, second) = send(
        &app,
        Method::POST,
        "/api/sessions",
        Some(json!({"session": SESSION_A, "repo_branch": "feature", "title": "Fix the thing"})),
        &[],
    )
    .await;
    assert_eq!(second["repo_branch"], "feature");
    assert_eq!(second["title"], "Fix the thing");
    // absent fields keep their value
    assert_eq!(second["cwd"], "/work/wenlan");
    assert_eq!(
        second["transcript_path"],
        "/home/u/.claude/projects/x/y.jsonl"
    );
    assert_eq!(second["last_write_at"], first["last_write_at"]);

    let (status, _) = send(
        &app,
        Method::POST,
        "/api/sessions",
        Some(json!({"session": "bad session"})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn session_detail_lists_memory_note_and_handoff() {
    let (app, _temp, _db) = common::test_app_no_gate().await;
    let headers = [(SESSION_HEADER, SESSION_A)];

    let source_id = store_memory(&app, "a memory written in the session", Some(SESSION_A)).await;
    let (status, page) = send(
        &app,
        Method::POST,
        "/api/pages",
        Some(json!({
            "title": "Session note",
            "content": "Written by the session.",
            "creation_kind": "authored"
        })),
        &headers,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let page_id = page["id"].as_str().unwrap().to_string();

    let (status, receipt) = send(
        &app,
        Method::PATCH,
        "/api/brief",
        Some(json!({
            "space": "work",
            "caller_id": "session-test",
            "operation_id": "op-1",
            "summary": {"text": "Shipped the session work", "expected_version": 0},
        })),
        &headers,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");

    let (status, detail) = send(
        &app,
        Method::GET,
        &format!("/api/sessions/{}", session_id(SESSION_A)),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["memories"][0]["source_id"], source_id.as_str());
    assert_eq!(detail["pages"][0]["id"], page_id.as_str());
    assert_eq!(detail["pages"][0]["title"], "Session note");
    assert_eq!(detail["handoffs"][0]["space"], "work");
    assert_eq!(detail["handoffs"][0]["summary"], "Shipped the session work");

    let (_, history) = send(
        &app,
        Method::GET,
        "/api/brief/history?space=work",
        None,
        &[],
    )
    .await;
    assert_eq!(
        history["entries"][0]["session_id"],
        session_id(SESSION_A).as_str()
    );

    // A caller locked to another Space does not learn the session exists.
    let (status, _) = send(
        &app,
        Method::GET,
        &format!("/api/sessions/{}", session_id(SESSION_A)),
        None,
        &[("x-wenlan-space", "elsewhere")],
    )
    .await;
    assert_ne!(status, StatusCode::OK);
}

#[tokio::test]
async fn page_sessions_report_writers_and_cited_sources() {
    let (app, _temp, _db) = common::test_app_no_gate().await;

    // Session A wrote the memory; session B wrote the note that cites it.
    let source_id = store_memory(&app, "a source memory from session a", Some(SESSION_A)).await;
    let (status, page) = send(
        &app,
        Method::POST,
        "/api/pages",
        Some(json!({
            "title": "Cites the memory",
            "content": "Built on a memory.",
            "creation_kind": "authored",
            "source_memory_ids": [source_id]
        })),
        &[(SESSION_HEADER, SESSION_B)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let page_id = page["id"].as_str().unwrap();

    let (status, sessions) = send(
        &app,
        Method::GET,
        &format!("/api/pages/{page_id}/sessions"),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sessions}");
    let find = |id: &str| {
        sessions["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["session"]["id"] == id)
            .unwrap_or_else(|| panic!("session {id} missing from {sessions}"))
            .clone()
    };
    assert_eq!(find(&session_id(SESSION_B))["via"], "wrote");
    assert_eq!(find(&session_id(SESSION_A))["via"], "source");

    let (status, _) = send(
        &app,
        Method::GET,
        "/api/pages/missing-page/sessions",
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn session_detail_reports_whether_the_transcript_file_exists() {
    let (app, _temp, _db) = common::test_app_no_gate().await;
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("session.jsonl");
    std::fs::write(&transcript, "{}").unwrap();
    store_memory(
        &app,
        "a memory from a session with a transcript",
        Some(SESSION_A),
    )
    .await;
    let (status, _) = send(
        &app,
        Method::POST,
        "/api/sessions",
        Some(json!({"session": SESSION_A, "transcript_path": transcript.to_str().unwrap()})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let uri = format!("/api/sessions/{}", session_id(SESSION_A));

    let (_, present) = send(&app, Method::GET, &uri, None, &[]).await;
    assert_eq!(present["transcript_available"], true, "{present}");

    std::fs::remove_file(&transcript).unwrap();
    let (_, pruned) = send(&app, Method::GET, &uri, None, &[]).await;
    assert_eq!(pruned["transcript_available"], false, "{pruned}");
    assert!(pruned["session"]["transcript_path"].is_string());

    // No stored path: nothing to be available.
    store_memory(
        &app,
        "a memory from an import with no transcript",
        Some(SESSION_B),
    )
    .await;
    let (_, none) = send(
        &app,
        Method::GET,
        &format!("/api/sessions/{}", session_id(SESSION_B)),
        None,
        &[],
    )
    .await;
    assert_eq!(none["transcript_available"], false, "{none}");
    assert!(none["session"]["transcript_path"].is_null());
}

// ---------------------------------------------------------------------------
// Conversation title, lookups, and notes that later used the session's memories.
// ---------------------------------------------------------------------------

async fn session_detail(app: &AppRouter, session: &str) -> Value {
    let (status, detail) = send(
        app,
        Method::GET,
        &format!("/api/sessions/{}", session_id(session)),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail
}

/// Lookups are recorded on a spawned task after the read answers: wait for the
/// detail to list `count` of them.
async fn detail_with_reads(app: &AppRouter, session: &str, count: usize) -> Value {
    for _ in 0..100 {
        let detail = session_detail(app, session).await;
        if detail["reads"].as_array().map_or(0, Vec::len) >= count {
            return detail;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    session_detail(app, session).await
}

async fn announce(app: &AppRouter, session: &str, extra: Value) {
    let mut body = json!({ "session": session });
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let (status, response) = send(app, Method::POST, "/api/sessions", Some(body), &[]).await;
    assert_eq!(status, StatusCode::OK, "{response}");
}

fn transcript_file(dir: &tempfile::TempDir, body: &str) -> String {
    let path = dir.path().join("session.jsonl");
    std::fs::write(&path, body).unwrap();
    path.to_str().unwrap().to_string()
}

const TRANSCRIPT: &str = concat!(
    "{\"type\":\"user\",\"message\":\"hi\"}\n",
    "{\"type\":\"ai-title\",\"aiTitle\":\"Early title\",\"sessionId\":\"s\"}\n",
    "not json at all\n",
    "{\"type\":\"ai-title\",\"aiTitle\":\"文兰 AI 会话资源\",\"sessionId\":\"s\"}\n",
);

#[tokio::test]
async fn claude_code_title_is_read_from_the_transcript_each_time() {
    let (app, _temp, db) = common::test_app_no_gate().await;
    let dir = tempfile::tempdir().unwrap();
    let path = transcript_file(&dir, TRANSCRIPT);
    store_memory(&app, "a memory so the session is visible", Some(SESSION_A)).await;
    announce(&app, SESSION_A, json!({ "transcript_path": path })).await;

    let detail = session_detail(&app, SESSION_A).await;
    assert_eq!(detail["session"]["title"], "文兰 AI 会话资源");
    // Shown, not stored: the read stays write-free.
    let stored = db
        .get_session(&session_id(SESSION_A))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.title, None);

    // A later rename in the transcript shows up.
    std::fs::write(
        &path,
        format!("{TRANSCRIPT}{{\"type\":\"ai-title\",\"aiTitle\":\"Newer\"}}\n"),
    )
    .unwrap();
    let detail = session_detail(&app, SESSION_A).await;
    assert_eq!(detail["session"]["title"], "Newer");
}

#[tokio::test]
async fn a_title_the_agent_sent_wins_over_the_transcript() {
    let (app, _temp, _db) = common::test_app_no_gate().await;
    let dir = tempfile::tempdir().unwrap();
    let path = transcript_file(&dir, TRANSCRIPT);
    store_memory(&app, "a memory so the session is visible", Some(SESSION_A)).await;
    announce(
        &app,
        SESSION_A,
        json!({ "transcript_path": path, "title": "Sent by the agent" }),
    )
    .await;

    let detail = session_detail(&app, SESSION_A).await;
    assert_eq!(detail["session"]["title"], "Sent by the agent");
}

#[tokio::test]
async fn other_tools_and_missing_transcripts_keep_no_title() {
    let (app, _temp, _db) = common::test_app_no_gate().await;
    let dir = tempfile::tempdir().unwrap();
    let path = transcript_file(&dir, TRANSCRIPT);
    // Codex keeps using the title it sends, even with such lines around.
    store_memory(&app, "a codex memory", Some(SESSION_B)).await;
    announce(&app, SESSION_B, json!({ "transcript_path": path })).await;
    let detail = session_detail(&app, SESSION_B).await;
    assert!(detail["session"].get("title").is_none(), "{detail}");

    // A Claude Code session whose transcript was pruned has none either.
    store_memory(&app, "a claude memory", Some(SESSION_A)).await;
    announce(
        &app,
        SESSION_A,
        json!({ "transcript_path": dir.path().join("gone.jsonl").to_str().unwrap() }),
    )
    .await;
    let detail = session_detail(&app, SESSION_A).await;
    assert!(detail["session"].get("title").is_none(), "{detail}");
}

#[tokio::test]
async fn lookups_that_name_the_session_are_listed_with_note_titles() {
    let (app, _temp, _db) = common::test_app_no_gate().await;
    let headers = [(SESSION_HEADER, SESSION_A)];
    store_memory(
        &app,
        "the sync design uses a single writer",
        Some(SESSION_A),
    )
    .await;
    announce(&app, SESSION_A, json!({})).await;
    let (status, page) = send(
        &app,
        Method::POST,
        "/api/pages",
        Some(json!({
            "title": "Sync design",
            "content": "One writer.",
            "creation_kind": "authored"
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let page_id = page["id"].as_str().unwrap().to_string();
    let (status, _) = send(
        &app,
        Method::PATCH,
        "/api/brief",
        Some(json!({
            "space": "work",
            "caller_id": "session-test",
            "operation_id": "op-reads",
            "summary": {"text": "Handed over", "expected_version": 0},
        })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A lookup with no header, and one with a malformed header, are not listed
    // and do not fail.
    let (status, _) = send(
        &app,
        Method::POST,
        "/api/memory/search",
        Some(json!({"query": "unattributed"})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &app,
        Method::POST,
        "/api/memory/search",
        Some(json!({"query": "bad header"})),
        &[(SESSION_HEADER, "not a session")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    for (index, (method, uri, body)) in [
        (
            Method::POST,
            "/api/memory/search".to_string(),
            Some(json!({"query": "single writer"})),
        ),
        (
            Method::POST,
            "/api/brief".to_string(),
            Some(json!({"space": "work"})),
        ),
        (Method::GET, format!("/api/pages/{page_id}"), None),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, body) = send(&app, method, &uri, body, &headers).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        // Wait for each lookup to land before the next, so the newest-first
        // order is deterministic.
        let seen = detail_with_reads(&app, SESSION_A, index + 1).await["reads_total"]
            .as_i64()
            .unwrap();
        assert_eq!(seen as usize, index + 1, "{uri}");
    }

    let detail = detail_with_reads(&app, SESSION_A, 3).await;
    let reads = detail["reads"].as_array().unwrap();
    assert_eq!(detail["reads_total"], 3, "{detail}");
    assert_eq!(reads.len(), 3, "{detail}");
    assert_eq!(reads[0]["kind"], "read_page");
    assert_eq!(reads[0]["label"], page_id.as_str());
    assert_eq!(reads[0]["title"], "Sync design");
    assert_eq!(reads[1]["kind"], "brief");
    assert_eq!(reads[1]["label"], "work");
    assert_eq!(reads[2]["kind"], "recall");
    assert_eq!(reads[2]["label"], "single writer");
}

#[tokio::test]
async fn a_lookup_never_creates_a_session() {
    let (app, _temp, db) = common::test_app_no_gate().await;
    let (status, _) = send(
        &app,
        Method::POST,
        "/api/memory/search",
        Some(json!({"query": "from a stranger"})),
        &[(SESSION_HEADER, SESSION_B)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(db
        .get_session(&session_id(SESSION_B))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn notes_that_cite_the_sessions_memories_are_listed_unless_it_wrote_them() {
    let (app, _temp, _db) = common::test_app_no_gate().await;
    let source_id = store_memory(&app, "a source memory from session a", Some(SESSION_A)).await;
    let cite = |title: &str| {
        json!({
            "title": title,
            "content": "Built on a memory.",
            "creation_kind": "authored",
            "source_memory_ids": [source_id]
        })
    };
    let (status, theirs) = send(
        &app,
        Method::POST,
        "/api/pages",
        Some(cite("Cited by another session")),
        &[(SESSION_HEADER, SESSION_B)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{theirs}");
    let (status, own) = send(
        &app,
        Method::POST,
        "/api/pages",
        Some(cite("Written by session a")),
        &[(SESSION_HEADER, SESSION_A)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{own}");

    let detail = session_detail(&app, SESSION_A).await;
    let cited: Vec<&str> = detail["cited_by"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note["id"].as_str().unwrap())
        .collect();
    assert_eq!(cited, [theirs["id"].as_str().unwrap()], "{detail}");
    let pages: Vec<&str> = detail["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note["id"].as_str().unwrap())
        .collect();
    assert_eq!(pages, [own["id"].as_str().unwrap()]);

    // A caller locked to an unrelated Space cannot see the session at all.
    let (status, _) = send(
        &app,
        Method::GET,
        &format!("/api/sessions/{}", session_id(SESSION_A)),
        None,
        &[("x-wenlan-space", "elsewhere")],
    )
    .await;
    assert_ne!(status, StatusCode::OK);
}
