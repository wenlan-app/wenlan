// SPDX-License-Identifier: Apache-2.0
//! Removing a page or a memory over HTTP: an AI tool's delete archives or
//! forgets, so the person can undo it; it never removes the person's work; and
//! every removal leaves an `agent_activity` record. All state is scratch.
mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;
use wenlan_core::db::MemoryDB;
use wenlan_types::requests::{CALLER_HEADER, PERSON_CALLER_VALUE};
use wenlan_types::responses::{
    ForgetMemoryResponse, PageRemovalResponse, PageRemovalStatus, RestoreMemoryResponse,
};

/// Who sends a request.
#[derive(Clone, Copy)]
enum Caller {
    /// An AI tool that names itself.
    Agent(&'static str),
    /// A caller that sends no name at all.
    Unnamed,
    /// The desktop app, for a gesture of the person.
    Person,
    /// A named tool that also sends the person marker: still a tool.
    AgentWithPersonMarker(&'static str),
}

/// The permanent path reads the knowledge path from config; point it at
/// scratch so no test can touch a real pages folder.
fn data_dir_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

struct ScratchDataDir {
    previous: Option<std::ffi::OsString>,
    _tmp: tempfile::TempDir,
}

impl ScratchDataDir {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let pages = tmp.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(
            tmp.path().join("config.json"),
            json!({ "knowledge_path": pages.to_string_lossy() }).to_string(),
        )
        .unwrap();
        let previous = std::env::var_os("WENLAN_DATA_DIR");
        std::env::set_var("WENLAN_DATA_DIR", tmp.path());
        Self {
            previous,
            _tmp: tmp,
        }
    }
}

impl Drop for ScratchDataDir {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("WENLAN_DATA_DIR", value),
            None => std::env::remove_var("WENLAN_DATA_DIR"),
        }
    }
}

async fn send(
    app: &common::AppRouter,
    method: Method,
    path: &str,
    caller: Caller,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path);
    match caller {
        Caller::Agent(name) => request = request.header("x-agent-name", name),
        Caller::Unnamed => {}
        Caller::Person => request = request.header(CALLER_HEADER, PERSON_CALLER_VALUE),
        Caller::AgentWithPersonMarker(name) => {
            request = request
                .header("x-agent-name", name)
                .header(CALLER_HEADER, PERSON_CALLER_VALUE)
        }
    }
    if body.is_some() {
        request = request
            .header("content-type", "application/json")
            .header("x-wenlan-truth-contract", "1")
            .header("x-wenlan-reader-intent", "explicit");
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(
                    body.map(|value| Body::from(value.to_string()))
                        .unwrap_or_else(Body::empty),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

/// A page the AI wrote from a memory, that nobody has touched.
async fn ai_page(db: &Arc<MemoryDB>, title: &str) -> String {
    let source = format!("src-{}", title.to_lowercase().replace(' ', "-"));
    common::insert_memory(
        db,
        &source,
        "Ferns reproduce by spores carried on the wind.",
        "memory",
        Some("claude-code"),
        None,
        false,
        1,
    )
    .await;
    common::create_page_fixture(
        db,
        title,
        "Ferns reproduce by spores carried on the wind.",
        None,
        &[source.as_str()],
        "distilled",
    )
    .await
}

/// An AI's page the person then edited by hand in the app.
async fn person_edited_page(app: &common::AppRouter, db: &Arc<MemoryDB>) -> String {
    let id = ai_page(db, "Edited ferns").await;
    // The app's manual edit route, as the app sends it.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/memory/{id}/update-page"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"content": "Ferns reproduce by spores, not seeds."}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page = db.get_page(&id).await.unwrap().unwrap();
    assert!(
        db.list_page_history(&id, 20)
            .await
            .unwrap()
            .iter()
            .any(|entry| entry.edited_by == "manual_edit"),
        "fixture: the person's edit is in the history"
    );
    assert_eq!(page.status, "active");
    id
}

fn removal(value: Value) -> PageRemovalResponse {
    serde_json::from_value(value).expect("a typed page removal response")
}

#[tokio::test]
async fn ux_who_7_an_ai_delete_archives_the_page_and_restore_brings_it_back_with_its_history() {
    let _lock = data_dir_lock().lock().await;
    let _data = ScratchDataDir::new();
    let (app, _tmp, db) = common::test_app().await;
    let id = ai_page(&db, "Spore ferns").await;
    let before = db.get_page(&id).await.unwrap().unwrap();
    let history_before = db.list_page_history(&id, 50).await.unwrap();
    assert!(!history_before.is_empty());

    let (status, value) = send(
        &app,
        Method::DELETE,
        &format!("/api/pages/{id}"),
        Caller::Agent("claude-code"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let removed = removal(value);
    assert_eq!(removed.status, PageRemovalStatus::Archived);
    assert!(removed.restorable);
    let archived = db.get_page(&id).await.unwrap().expect("the row is kept");
    assert_eq!(archived.status, "archived");
    assert_eq!(removed.version, Some(archived.version));
    let history_archived = db.list_page_history(&id, 50).await.unwrap();
    for entry in &history_before {
        assert!(
            history_archived.contains(entry),
            "archive keeps every earlier version: {entry:?}"
        );
    }

    let (status, value) = send(
        &app,
        Method::POST,
        &format!("/api/pages/{id}/restore"),
        Caller::Person,
        Some(json!({"expected_version": archived.version})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let restored = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(restored.status, "active");
    assert_eq!(restored.content, before.content);
    assert_eq!(restored.title, before.title);
    let history_restored = db.list_page_history(&id, 50).await.unwrap();
    for entry in &history_before {
        assert!(history_restored.contains(entry), "{entry:?}");
    }
}

#[tokio::test]
async fn ux_who_7_every_page_archive_and_delete_writes_an_activity_record() {
    let _lock = data_dir_lock().lock().await;
    let _data = ScratchDataDir::new();
    let (app, _tmp, db) = common::test_app().await;
    let archived_id = ai_page(&db, "Archived by codex").await;
    let deleted_id = ai_page(&db, "Deleted by the person").await;
    let deleted_version = db.get_page(&deleted_id).await.unwrap().unwrap().version;

    let (status, value) = send(
        &app,
        Method::DELETE,
        &format!("/api/pages/{archived_id}"),
        Caller::Agent("codex"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let archived_version = removal(value).version.unwrap();

    let (status, value) = send(
        &app,
        Method::DELETE,
        &format!("/api/pages/{deleted_id}?permanent=true"),
        Caller::Person,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let deleted = removal(value);
    assert_eq!(deleted.status, PageRemovalStatus::Deleted);
    assert!(!deleted.restorable);
    assert!(db.get_page(&deleted_id).await.unwrap().is_none());

    let rows = db.list_agent_activity(100, None, None).await.unwrap();
    let archive = rows
        .iter()
        .find(|row| row.action == "page_archive")
        .expect("an archive record");
    assert_eq!(archive.agent_name, "codex");
    assert_eq!(
        archive
            .memory_ids
            .as_deref()
            .map(|ids| ids.contains(&archived_id)),
        Some(true),
        "{archive:?}"
    );
    assert_eq!(
        archive.detail.as_deref(),
        Some(format!("archived \"Archived by codex\" at version {archived_version}").as_str())
    );
    assert!(archive.timestamp > 0);
    let delete = rows
        .iter()
        .find(|row| row.action == "page_delete")
        .expect("a delete record");
    assert_eq!(delete.agent_name, "person");
    assert_eq!(
        delete
            .memory_ids
            .as_deref()
            .map(|ids| ids.contains(&deleted_id)),
        Some(true),
        "{delete:?}"
    );
    assert!(
        delete
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains(&format!(
                "\"Deleted by the person\" at version {deleted_version}"
            )),
        "{delete:?}"
    );
}

#[tokio::test]
async fn ux_who_2_an_ai_cannot_archive_or_delete_a_page_the_person_edited() {
    let _lock = data_dir_lock().lock().await;
    let _data = ScratchDataDir::new();
    let (app, _tmp, db) = common::test_app().await;
    let edited = person_edited_page(&app, &db).await;
    let authored =
        common::create_page_fixture(&db, "My own page", "I wrote this.", None, &[], "authored")
            .await;

    for id in [&edited, &authored] {
        let version = db.get_page(id).await.unwrap().unwrap().version;
        for (method, path, caller) in [
            (
                Method::DELETE,
                format!("/api/pages/{id}"),
                Caller::Agent("claude-code"),
            ),
            (
                Method::POST,
                format!("/api/pages/{id}/archive"),
                Caller::Agent("claude-code"),
            ),
            // No name is not the person.
            (Method::DELETE, format!("/api/pages/{id}"), Caller::Unnamed),
            (
                Method::POST,
                format!("/api/pages/{id}/archive"),
                Caller::Unnamed,
            ),
            // A name wins over the marker, even the name "unknown".
            (
                Method::DELETE,
                format!("/api/pages/{id}"),
                Caller::AgentWithPersonMarker("claude-code"),
            ),
            (
                Method::POST,
                format!("/api/pages/{id}/archive"),
                Caller::AgentWithPersonMarker("unknown"),
            ),
        ] {
            let (status, value) = send(&app, method.clone(), &path, caller, None).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {value}");
            assert_eq!(value["code"], "page_is_the_persons", "{value}");
        }
        let page = db.get_page(id).await.unwrap().unwrap();
        assert_eq!(page.status, "active", "the refused page stays put");
        assert_eq!(page.version, version);
    }
    let rows = db.list_agent_activity(100, None, None).await.unwrap();
    assert!(
        rows.iter()
            .all(|row| row.action != "page_archive" && row.action != "page_delete"),
        "a refusal records no removal"
    );

    // The person may archive their own page from the app.
    let (status, value) = send(
        &app,
        Method::POST,
        &format!("/api/pages/{edited}/archive"),
        Caller::Person,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(removal(value).status, PageRemovalStatus::Archived);
}

#[tokio::test]
async fn ux_who_7_only_the_person_can_delete_a_page_for_good() {
    let _lock = data_dir_lock().lock().await;
    let _data = ScratchDataDir::new();
    let (app, _tmp, db) = common::test_app().await;
    let id = ai_page(&db, "Kept ferns").await;
    for caller in [Caller::Agent("claude-code"), Caller::Unnamed] {
        let (status, value) = send(
            &app,
            Method::DELETE,
            &format!("/api/pages/{id}?permanent=true"),
            caller,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{value}");
        assert_eq!(value["code"], "permanent_delete_needs_person", "{value}");
    }
    assert_eq!(db.get_page(&id).await.unwrap().unwrap().status, "active");
}

/// A diagram an AI tool kept by itself in a live session is the AI's own
/// work: the AI may archive it (the person can restore it), never delete it
/// for good. What the person imported stays theirs.
#[tokio::test]
async fn ux_who_7_an_ai_may_archive_an_original_it_kept_but_not_delete_it_for_good() {
    let _lock = data_dir_lock().lock().await;
    let _data = ScratchDataDir::new();
    let (app, _tmp, db) = common::test_app().await;
    let mut kept = Vec::new();
    for (title, content) in [
        (
            "Test flow",
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>",
        ),
        (
            "Test flow two",
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><circle/></svg>",
        ),
    ] {
        // The keep route as an AI tool calls it, with only its name.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/originals")
                    .header("x-agent-name", "claude-code")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"title": title, "content": content, "format": "svg"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::OK, "{value}");
        kept.push(value["page_id"].as_str().unwrap().to_string());
    }

    let (status, value) = send(
        &app,
        Method::DELETE,
        &format!("/api/pages/{}", kept[0]),
        Caller::Agent("claude-code"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(removal(value).status, PageRemovalStatus::Archived);
    let page = db.get_page(&kept[0]).await.unwrap().unwrap();
    assert_eq!(page.status, "archived");

    let (status, value) = send(
        &app,
        Method::DELETE,
        &format!("/api/pages/{}?permanent=true", kept[1]),
        Caller::Agent("claude-code"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{value}");
    assert_eq!(value["code"], "permanent_delete_needs_person", "{value}");
    let page = db.get_page(&kept[1]).await.unwrap().unwrap();
    assert_eq!(page.status, "active");
}

#[tokio::test]
async fn ux_who_7_deleting_a_page_that_does_not_exist_is_not_found() {
    let _lock = data_dir_lock().lock().await;
    let _data = ScratchDataDir::new();
    let (app, _tmp, db) = common::test_app().await;
    for (path, caller) in [
        ("/api/pages/page_missing", Caller::Agent("claude-code")),
        ("/api/pages/page_missing", Caller::Unnamed),
        ("/api/pages/page_missing?permanent=true", Caller::Person),
    ] {
        let (status, value) = send(&app, Method::DELETE, path, caller, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {value}");
    }

    // A page that is already archived is still there, so deleting it again is
    // not a 404: it changes nothing (no new version) and stays restorable.
    let id = ai_page(&db, "Twice archived ferns").await;
    for first in [true, false] {
        let (status, value) = send(
            &app,
            Method::DELETE,
            &format!("/api/pages/{id}"),
            Caller::Agent("claude-code"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
        let removed = removal(value);
        assert_eq!(removed.status, PageRemovalStatus::Archived);
        assert!(removed.restorable);
        assert_eq!(removed.version.is_some(), first);
    }
}

#[tokio::test]
async fn ux_who_7_an_unnamed_or_ai_memory_delete_forgets_and_restore_brings_it_back() {
    let _lock = data_dir_lock().lock().await;
    let _data = ScratchDataDir::new();
    let (app, tmp, db) = common::test_app().await;
    for (caller, agent) in [
        (Caller::Agent("codex"), "codex"),
        (Caller::Unnamed, "unknown"),
    ] {
        let source_id = format!("mem-forget-{agent}");
        common::insert_memory(
            &db,
            &source_id,
            "The amber kestrel marker is used for client reviews.",
            "memory",
            Some("codex"),
            None,
            false,
            1,
        )
        .await;
        let (status, value) = send(
            &app,
            Method::DELETE,
            &format!("/api/memory/delete/{source_id}"),
            caller,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
        let forgotten: ForgetMemoryResponse = serde_json::from_value(value).unwrap();
        assert!(forgotten.deleted && forgotten.restorable);
        let modes = memory_modes(&tmp, &source_id).await;
        assert!(!modes.is_empty(), "forget keeps the rows");
        assert!(
            modes.iter().all(|mode| mode.as_deref() == Some("evicted")),
            "{modes:?}"
        );
        let rows = db
            .list_agent_activity(100, Some(agent), None)
            .await
            .unwrap();
        assert!(
            rows.iter().any(|row| row.action == "forget"
                && row
                    .memory_ids
                    .as_deref()
                    .unwrap_or_default()
                    .contains(&source_id)),
            "{rows:?}"
        );

        // A permanent delete is the person's alone.
        let (status, value) = send(
            &app,
            Method::DELETE,
            &format!("/api/memory/delete/{source_id}?permanent=true"),
            caller,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{value}");
        assert_eq!(value["code"], "permanent_delete_needs_person", "{value}");

        let (status, value) = send(
            &app,
            Method::POST,
            &format!("/api/memory/restore/{source_id}"),
            Caller::Person,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
        let restored: RestoreMemoryResponse = serde_json::from_value(value).unwrap();
        assert!(restored.restored);
        let modes = memory_modes(&tmp, &source_id).await;
        assert!(!modes.is_empty());
        assert!(
            modes.iter().all(|mode| mode.as_deref() != Some("evicted")),
            "{modes:?}"
        );
    }
}

#[tokio::test]
async fn ux_who_7_the_persons_memory_delete_from_the_app_still_destroys_it() {
    let _lock = data_dir_lock().lock().await;
    let _data = ScratchDataDir::new();
    let (app, tmp, db) = common::test_app().await;
    common::insert_memory(
        &db,
        "mem-destroy",
        "A memory the person removes for good.",
        "memory",
        Some("codex"),
        None,
        false,
        1,
    )
    .await;
    let (status, value) = send(
        &app,
        Method::DELETE,
        "/api/memory/delete/mem-destroy?permanent=true",
        Caller::Person,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let destroyed: ForgetMemoryResponse = serde_json::from_value(value).unwrap();
    assert!(destroyed.deleted && !destroyed.restorable);
    assert!(memory_modes(&tmp, "mem-destroy").await.is_empty());
    assert_eq!(
        common::count_activity_for_action_and_agent(&db, "forget", "person").await,
        1
    );
}

/// The `supersede_mode` of every row memory `source_id` has.
async fn memory_modes(tmp: &tempfile::TempDir, source_id: &str) -> Vec<Option<String>> {
    let raw = libsql::Builder::new_local(tmp.path().join("origin_memory.db").to_str().unwrap())
        .build()
        .await
        .unwrap();
    let conn = raw.connect().unwrap();
    let mut rows = conn
        .query(
            "SELECT supersede_mode FROM memories WHERE source = 'memory' AND source_id = ?1",
            libsql::params![source_id],
        )
        .await
        .unwrap();
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        out.push(row.get::<Option<String>>(0).ok().flatten());
    }
    out
}

/// The `session_id` of version `version` of page `page_id`.
async fn history_session(tmp: &tempfile::TempDir, page_id: &str, version: i64) -> Option<String> {
    let raw = libsql::Builder::new_local(tmp.path().join("origin_memory.db").to_str().unwrap())
        .build()
        .await
        .unwrap();
    let conn = raw.connect().unwrap();
    let mut rows = conn
        .query(
            "SELECT session_id FROM page_history WHERE page_id = ?1 AND version = ?2",
            libsql::params![page_id, version],
        )
        .await
        .unwrap();
    let row = rows
        .next()
        .await
        .unwrap()
        .expect("the version has a history row");
    row.get::<Option<String>>(0).unwrap()
}

/// Add a history row with no session to `page_id`, as another write made at
/// `created_at` would leave.
async fn add_sessionless_history_row(
    tmp: &tempfile::TempDir,
    page_id: &str,
    version: i64,
    created_at: i64,
) {
    let raw = libsql::Builder::new_local(tmp.path().join("origin_memory.db").to_str().unwrap())
        .build()
        .await
        .unwrap();
    let conn = raw.connect().unwrap();
    conn.execute(
        "INSERT INTO page_history (page_id, version, content, edited_by, created_at) \
         VALUES (?1, ?2, 'another write', 'codex', ?3)",
        libsql::params![page_id, version, created_at],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn an_archive_names_the_session_that_asked_for_it() {
    let _lock = data_dir_lock().lock().await;
    let _data = ScratchDataDir::new();
    let (app, tmp, db) = common::test_app().await;
    let session = "claude-code:00893aaf-19fa-41d2-8238-13269b9b3ca0";

    for route in ["delete", "archive"] {
        let id = ai_page(&db, &format!("Session ferns {route}")).await;
        // A sessionless row of the same page that a time window starting at
        // the archive would also cover. It must stay sessionless.
        let neighbour = 900;
        add_sessionless_history_row(&tmp, &id, neighbour, chrono::Utc::now().timestamp() + 60)
            .await;
        let request = match route {
            "delete" => Request::builder()
                .method(Method::DELETE)
                .uri(format!("/api/pages/{id}")),
            _ => Request::builder()
                .method(Method::POST)
                .uri(format!("/api/pages/{id}/archive")),
        };
        let response = app
            .clone()
            .oneshot(
                request
                    .header("x-agent-name", "claude-code")
                    .header(wenlan_types::SESSION_HEADER, session)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let removed: PageRemovalResponse = serde_json::from_slice(&bytes).unwrap();
        let version = removed.version.expect("this call archived the page");

        let expected = wenlan_core::db::sessions::session_row_id(
            &wenlan_types::SessionRef::parse(session).unwrap(),
        );
        assert_eq!(
            history_session(&tmp, &id, version).await.as_deref(),
            Some(expected.as_str()),
            "{route}: the archive's history row names the session"
        );
        assert_eq!(
            history_session(&tmp, &id, neighbour).await,
            None,
            "{route}: only the archived version is stamped"
        );
    }
}

/// Every event a subscriber saw while `run` ran, one formatted line each.
#[derive(Clone, Default)]
struct CapturedLog(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl CapturedLog {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

#[tokio::test]
async fn a_permanent_memory_delete_and_its_refusal_are_logged_with_who_asked() {
    let _lock = data_dir_lock().lock().await;
    let _data = ScratchDataDir::new();
    let (app, _tmp, db) = common::test_app().await;
    common::insert_memory(
        &db,
        "mem-logged",
        "Ferns unfurl from fiddleheads.",
        "memory",
        Some("claude-code"),
        None,
        false,
        1,
    )
    .await;

    let log = CapturedLog::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .finish();
    let _default = tracing::subscriber::set_default(subscriber);

    let (status, _) = send(
        &app,
        Method::DELETE,
        "/api/memory/delete/mem-logged?permanent=true",
        Caller::Agent("claude-code"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, value) = send(
        &app,
        Method::DELETE,
        "/api/memory/delete/mem-logged?permanent=true",
        Caller::Person,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");

    let text = log.text();
    let refused = text
        .lines()
        .find(|line| line.contains("refused permanent delete of a memory"))
        .unwrap_or_else(|| panic!("no refusal line in:\n{text}"));
    assert!(refused.contains("WARN"), "{refused}");
    assert!(refused.contains("source_id=mem-logged"), "{refused}");
    assert!(refused.contains("agent=claude-code"), "{refused}");
    let deleted = text
        .lines()
        .find(|line| line.contains("deleted a memory permanently"))
        .unwrap_or_else(|| panic!("no delete line in:\n{text}"));
    assert!(deleted.contains("INFO"), "{deleted}");
    assert!(deleted.contains("source_id=mem-logged"), "{deleted}");
    assert!(deleted.contains("agent=person"), "{deleted}");
}
