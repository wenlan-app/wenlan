// SPDX-License-Identifier: Apache-2.0
use crate::state::ServerState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;
use wenlan_core::config::{save_config, Config};
use wenlan_types::sources::{SourceType, SyncStatus};

struct DataDirGuard {
    previous: Option<std::ffi::OsString>,
    tmp: tempfile::TempDir,
}

impl DataDirGuard {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("WENLAN_DATA_DIR");
        std::env::set_var("WENLAN_DATA_DIR", tmp.path());
        Self { previous, tmp }
    }
}

impl Drop for DataDirGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("WENLAN_DATA_DIR", value),
            None => std::env::remove_var("WENLAN_DATA_DIR"),
        }
    }
}

/// A data dir with one vault `notes` holding `Work/plan.md`, and its router.
fn setup(guard: &DataDirGuard) -> (std::path::PathBuf, crate::router::AppRouter) {
    let root = guard.tmp.path().join("My notes");
    std::fs::create_dir_all(root.join("Work")).unwrap();
    std::fs::write(root.join("Work/plan.md"), "Launch in September.\n").unwrap();
    let mut config = Config {
        knowledge_path: Some(guard.tmp.path().join("pages")),
        setup_completed: true,
        ..Config::default()
    };
    config.sources.push(wenlan_core::sources::Source {
        id: "notes".into(),
        source_type: SourceType::Obsidian,
        path: root.clone(),
        status: SyncStatus::Active,
        last_sync: None,
        file_count: 0,
        memory_count: 0,
        last_sync_errors: 0,
        last_sync_error_detail: None,
        space: None,
        queued_files: 0,
        waiting_files: 0,
    });
    save_config(&config).unwrap();
    let state = Arc::new(RwLock::new(ServerState::default()));
    (root, crate::router::build_router(state))
}

async fn send(
    app: &mut crate::router::AppRouter,
    method: &str,
    uri: &str,
    agent: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(agent) = agent {
        request = request.header("x-agent-name", agent);
    }
    let request = match body {
        Some(body) => request
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .unwrap();
    let response = tower::ServiceExt::oneshot(&mut *app, request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

async fn lock() -> tokio::sync::MutexGuard<'static, ()> {
    crate::TEST_DATA_DIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

#[tokio::test(flavor = "current_thread")]
async fn a_persons_notes_start_at_never_and_only_the_person_changes_that() {
    let _lock = lock().await;
    let guard = DataDirGuard::new();
    let (_, mut app) = setup(&guard);

    let (status, body) = send(
        &mut app,
        "GET",
        "/api/sources/notes/ai-changes?path=Work/plan.md",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"path": "Work/plan.md", "value": "never", "from": "default"})
    );

    let ask = json!({"path": "Work/", "value": "ask_first"});
    let (status, body) = send(
        &mut app,
        "PUT",
        "/api/sources/notes/ai-changes",
        Some("claude-code"),
        Some(ask.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.to_string().contains("note_needs_the_person"), "{body}");

    let (status, body) = send(
        &mut app,
        "PUT",
        "/api/sources/notes/ai-changes",
        None,
        Some(ask),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({"path": "Work/", "value": "ask_first", "from": "folder"})
    );
    let (_, body) = send(
        &mut app,
        "GET",
        "/api/sources/notes/ai-changes?path=Work/plan.md",
        None,
        None,
    )
    .await;
    assert_eq!(body["from"], "folder");
    assert_eq!(body["value"], "ask_first");

    let (status, _) = send(
        &mut app,
        "PUT",
        "/api/sources/notes/ai-changes",
        None,
        Some(json!({"path": "../x", "value": "never"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = send(
        &mut app,
        "PUT",
        "/api/sources/notes/ai-changes",
        None,
        Some(json!({"path": "", "value": "allowed"})),
    )
    .await;
    assert!(
        status.is_client_error(),
        "no direct AI edits of a person's notes yet"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_ai_change_lands_only_after_the_person_accepts_it() {
    let _lock = lock().await;
    let guard = DataDirGuard::new();
    let (root, mut app) = setup(&guard);
    let file = root.join("Work/plan.md");
    let propose = json!({
        "file": file.to_string_lossy(),
        "content": "Launch in October.\n",
        "reason": "the planning call moved it",
    });

    // Never: refused, nothing recorded, the note untouched.
    let (status, body) = send(
        &mut app,
        "POST",
        "/api/notes/proposals",
        Some("claude-code"),
        Some(propose.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.to_string().contains("note_ai_changes_never"), "{body}");

    send(
        &mut app,
        "PUT",
        "/api/sources/notes/ai-changes",
        None,
        Some(json!({"path": "", "value": "ask_first"})),
    )
    .await;
    let (status, proposal) = send(
        &mut app,
        "POST",
        "/api/notes/proposals",
        Some("claude-code"),
        Some(propose),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{proposal}");
    assert_eq!(proposal["path"], "Work/plan.md");
    assert_eq!(proposal["agent"], "claude-code");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "Launch in September.\n"
    );
    let id = proposal["id"].as_str().unwrap().to_string();

    let (_, list) = send(
        &mut app,
        "GET",
        "/api/notes/proposals?source_id=notes",
        None,
        None,
    )
    .await;
    assert_eq!(list["proposals"].as_array().unwrap().len(), 1);
    let (_, view) = send(
        &mut app,
        "GET",
        &format!("/api/notes/proposals/{id}"),
        None,
        None,
    )
    .await;
    assert_eq!(view["current"], "Launch in September.\n");
    assert_eq!(view["proposal"]["content"], "Launch in October.\n");

    // The AI cannot accept its own change.
    let (status, _) = send(
        &mut app,
        "POST",
        &format!("/api/notes/proposals/{id}/accept"),
        Some("claude-code"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "Launch in September.\n"
    );

    let (status, body) = send(
        &mut app,
        "POST",
        &format!("/api/notes/proposals/{id}/accept"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["file"], file.to_string_lossy().as_ref());
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "Launch in October.\n"
    );
    let (_, list) = send(&mut app, "GET", "/api/notes/proposals", None, None).await;
    assert!(list["proposals"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn declining_or_switching_to_never_leaves_the_note_alone() {
    let _lock = lock().await;
    let guard = DataDirGuard::new();
    let (root, mut app) = setup(&guard);
    let file = root.join("Work/plan.md");
    send(
        &mut app,
        "PUT",
        "/api/sources/notes/ai-changes",
        None,
        Some(json!({"path": "Work/", "value": "ask_first"})),
    )
    .await;
    let propose = json!({"file": file.to_string_lossy(), "content": "AI text\n"});

    let (_, proposal) = send(
        &mut app,
        "POST",
        "/api/notes/proposals",
        Some("codex"),
        Some(propose.clone()),
    )
    .await;
    let id = proposal["id"].as_str().unwrap().to_string();
    let (status, _) = send(
        &mut app,
        "DELETE",
        &format!("/api/notes/proposals/{id}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &mut app,
        "DELETE",
        &format!("/api/notes/proposals/{id}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    send(
        &mut app,
        "POST",
        "/api/notes/proposals",
        Some("codex"),
        Some(propose),
    )
    .await;
    send(
        &mut app,
        "PUT",
        "/api/sources/notes/ai-changes",
        None,
        Some(json!({"path": "Work/plan.md", "value": "never"})),
    )
    .await;
    let (_, list) = send(&mut app, "GET", "/api/notes/proposals", None, None).await;
    assert!(list["proposals"].as_array().unwrap().is_empty(), "{list}");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "Launch in September.\n"
    );

    // A file outside every vault is not a note.
    let outside =
        json!({"file": guard.tmp.path().join("elsewhere.md").to_string_lossy(), "content": "x"});
    let (status, body) = send(
        &mut app,
        "POST",
        "/api/notes/proposals",
        Some("codex"),
        Some(outside),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.to_string().contains("note_outside_vault"), "{body}");
}

#[tokio::test(flavor = "current_thread")]
async fn an_ai_tool_cannot_forget_replace_or_edit_what_came_from_a_note() {
    let _lock = lock().await;
    let guard = DataDirGuard::new();
    let (root, mut app) = setup(&guard);
    let chunk = format!("notes::{}::0", root.join("Work/plan.md").display());
    let encoded = chunk.replace('/', "%2F").replace(' ', "%20");

    let (status, body) = send(
        &mut app,
        "DELETE",
        &format!("/api/memory/delete/{encoded}"),
        Some("claude-code"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body.to_string().contains("note_change_needs_proposal"),
        "{body}"
    );
    let (status, body) = send(
        &mut app,
        "PUT",
        &format!("/api/memory/{encoded}/update"),
        Some("claude-code"),
        Some(json!({"content": "x"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    let headers = |agent: Option<&str>| {
        let mut map = axum::http::HeaderMap::new();
        if let Some(agent) = agent {
            map.insert("x-agent-name", agent.parse().unwrap());
        }
        map
    };
    assert!(super::refuse_ai_change_to_note(&headers(Some("claude-code")), &chunk).is_err());
    assert!(
        super::refuse_ai_change_to_note(&headers(None), &chunk).is_ok(),
        "the app may"
    );
    assert!(super::refuse_ai_change_to_note(&headers(Some("claude-code")), "mem_abc").is_ok());
    assert!(
        super::refuse_ai_change_to_note(&headers(Some("claude-code")), "notes-2::x::0").is_ok()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_ai_can_name_the_note_by_a_memory_it_read_from_it() {
    let _lock = lock().await;
    let guard = DataDirGuard::new();
    let (root, mut app) = setup(&guard);
    let file = root.join("Work/plan.md");
    send(
        &mut app,
        "PUT",
        "/api/sources/notes/ai-changes",
        None,
        Some(json!({"path": "", "value": "ask_first"})),
    )
    .await;
    let memory = format!("notes::{}::2", file.display());
    let (status, proposal) = send(
        &mut app,
        "POST",
        "/api/notes/proposals",
        Some("codex"),
        Some(json!({"file": memory, "content": "New\n"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{proposal}");
    assert_eq!(proposal["path"], "Work/plan.md");
}
