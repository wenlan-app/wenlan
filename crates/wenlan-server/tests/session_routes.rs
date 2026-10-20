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
    assert_eq!(status, StatusCode::BAD_REQUEST);
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
