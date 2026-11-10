// SPDX-License-Identifier: Apache-2.0
//! UX-WHO-2 over HTTP: an AI tool's refresh the person explicitly asked for
//! (`person_asked`) lands on the person's sentences, and
//! `POST /api/pages/{id}/restore-version` puts an earlier version back as a
//! new version. Every DB, vault and config path is scratch.

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;
use wenlan_core::db::MemoryDB;
use wenlan_types::responses::{PageChangelogEntry, RestorePageVersionResponse};

/// Every test points `WENLAN_DATA_DIR` (and so the knowledge path and
/// `config.json`) at its own scratch vault and holds this lock for its whole
/// body; no test may touch the developer's vault or config.
fn data_dir_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

struct ScratchVault {
    previous: Option<std::ffi::OsString>,
    _tmp: tempfile::TempDir,
}

impl ScratchVault {
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

impl Drop for ScratchVault {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("WENLAN_DATA_DIR", value),
            None => std::env::remove_var("WENLAN_DATA_DIR"),
        }
    }
}

#[derive(Clone, Copy)]
enum Caller {
    /// The app, with its person marker.
    Person,
    /// An AI tool, named by `x-agent-name`.
    Agent,
    /// An AI tool limited to one Space.
    AgentInSpace(&'static str),
}

async fn call(
    router: &common::AppRouter,
    method: Method,
    uri: &str,
    body: Option<Value>,
    caller: Caller,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    request = match caller {
        Caller::Person => request.header(
            wenlan_types::requests::CALLER_HEADER,
            wenlan_types::requests::PERSON_CALLER_VALUE,
        ),
        Caller::Agent => request.header("x-agent-name", "claude-code"),
        Caller::AgentInSpace(space) => request
            .header("x-agent-name", "claude-code")
            .header("x-wenlan-space", space),
    };
    let response = router
        .clone()
        .oneshot(
            request
                .body(
                    body.map(|b| Body::from(serde_json::to_vec(&b).unwrap()))
                        .unwrap_or_else(Body::empty),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

const SOURCES: [&str; 2] = ["mem-restore-a", "mem-restore-b"];
const AI_BODY: &str =
    "Tokio runs async tasks on a runtime.\n\nThe runtime uses a work-stealing scheduler.";
const MY_LINE: &str = "I prefer the current-thread runtime.";
const QUOTE: &str = "請把我那句改成 multi-thread runtime，我改主意了";

/// An AI-written page (version 1) that the person then adds one line to over
/// the app's save route (version 2).
async fn page_with_my_line(router: &common::AppRouter, db: &Arc<MemoryDB>) -> String {
    for id in SOURCES {
        common::insert_memory(
            db,
            id,
            "Tokio runs async tasks on a runtime with a work-stealing scheduler.",
            "memory",
            Some("test"),
            None,
            false,
            chrono::Utc::now().timestamp(),
        )
        .await;
    }
    let id = common::create_page_fixture(db, "Tokio Runtime", AI_BODY, None, &SOURCES, "distilled")
        .await;
    let (status, body) = call(
        router,
        Method::POST,
        &format!("/api/memory/{id}/update-page"),
        Some(json!({ "content": format!("{AI_BODY}\n\n{MY_LINE}") })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    id
}

async fn last_entry(db: &MemoryDB, id: &str) -> PageChangelogEntry {
    let entries: Vec<PageChangelogEntry> =
        serde_json::from_str(&db.get_page_changelog(id).await.unwrap()).unwrap();
    entries.into_iter().last().expect("a changelog entry")
}

async fn version_content(db: &MemoryDB, id: &str, version: i64) -> String {
    db.list_page_history(id, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.version == version)
        .unwrap_or_else(|| panic!("no version {version}"))
        .content
}

async fn restore(
    router: &common::AppRouter,
    id: &str,
    body: Value,
    caller: Caller,
) -> (StatusCode, Value) {
    call(
        router,
        Method::POST,
        &format!("/api/pages/{id}/restore-version"),
        Some(body),
        caller,
    )
    .await
}

fn code(body: &Value) -> &str {
    body["code"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn ux_who_2_a_person_asked_refresh_lands_and_restoring_the_earlier_version_undoes_it() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let id = page_with_my_line(&router, &db).await;
    let mine = db.get_page(&id).await.unwrap().unwrap();
    let asked = format!("{AI_BODY}\n\nI prefer the multi-thread runtime now.");

    // The person asked for it in the conversation: the change lands on their
    // sentence, and history keeps their words.
    let (status, body) = call(
        &router,
        Method::PUT,
        &format!("/api/pages/{id}"),
        Some(json!({
            "content": asked,
            "source_memory_ids": SOURCES,
            "expected_version": mine.version,
            "person_asked": QUOTE,
        })),
        Caller::Agent,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let landed = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(landed.content, asked);
    let entry = last_entry(&db, &id).await;
    assert_eq!(entry.edited_by, "person_asked");
    assert_eq!(entry.person_asked.as_deref(), Some(QUOTE));

    // person_asked without the version read is refused, nothing written.
    let (status, body) = call(
        &router,
        Method::PUT,
        &format!("/api/pages/{id}"),
        Some(json!({
            "content": AI_BODY,
            "source_memory_ids": SOURCES,
            "person_asked": QUOTE,
        })),
        Caller::Agent,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(db.get_page(&id).await.unwrap().unwrap().content, asked);

    // Putting the person's own line back changes their sentence again, so the
    // restore needs their words too.
    let (status, body) = restore(
        &router,
        &id,
        json!({"version": mine.version, "expected_version": landed.version}),
        Caller::Agent,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(code(&body), "restore_needs_person_asked", "{body}");
    assert_eq!(db.get_page(&id).await.unwrap().unwrap().content, asked);

    let undo = "改回去，我還是比較喜歡 current-thread";
    let (status, body) = restore(
        &router,
        &id,
        json!({
            "version": mine.version,
            "expected_version": landed.version,
            "person_asked": undo,
        }),
        Caller::Agent,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let restored: RestorePageVersionResponse = serde_json::from_value(body).unwrap();
    assert_eq!(restored.restored_from, mine.version);
    let after = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(after.content, mine.content, "version 2 is back");
    assert!(after.version > landed.version, "as a new version");
    let entry = last_entry(&db, &id).await;
    assert_eq!(entry.restored_from, Some(mine.version));
    assert_eq!(entry.person_asked.as_deref(), Some(undo));
}

#[tokio::test]
async fn restore_version_puts_back_an_ai_only_change_and_refuses_what_it_should() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let id = page_with_my_line(&router, &db).await;
    let mine = db.get_page(&id).await.unwrap().unwrap();

    // An AI-only change lands (the person's line is untouched).
    let ai_change = format!(
        "Tokio runs async tasks on a runtime.\n\n\
         The runtime uses a work-stealing scheduler across worker threads.\n\n{MY_LINE}"
    );
    let (status, body) = call(
        &router,
        Method::PUT,
        &format!("/api/pages/{id}"),
        Some(json!({
            "content": ai_change,
            "source_memory_ids": SOURCES,
            "expected_version": mine.version,
        })),
        Caller::Agent,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let changed = db.get_page(&id).await.unwrap().unwrap();
    assert!(changed.content.contains("across worker threads"));

    // Refusals change nothing.
    for (request, want_status, want_code) in [
        (
            json!({"version": mine.version, "expected_version": changed.version - 1}),
            StatusCode::CONFLICT,
            "restore_version_conflict",
        ),
        (
            json!({"version": 99, "expected_version": changed.version}),
            StatusCode::NOT_FOUND,
            "restore_version_unknown",
        ),
        (
            json!({"version": changed.version, "expected_version": changed.version}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "restore_version_current",
        ),
        (
            json!({"version": mine.version, "expected_version": changed.version, "person_asked": "  "}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "person_asked_invalid",
        ),
    ] {
        let (status, body) = restore(&router, &id, request.clone(), Caller::Agent).await;
        assert_eq!(status, want_status, "{request}: {body}");
        assert_eq!(code(&body), want_code, "{request}: {body}");
        assert_eq!(body["current_version"], json!(changed.version), "{body}");
        let now = db.get_page(&id).await.unwrap().unwrap();
        assert_eq!(
            (now.version, now.content.as_str()),
            (changed.version, changed.content.as_str())
        );
    }

    // Putting version 2 back touches only the AI's sentence: no words needed.
    let (status, body) = restore(
        &router,
        &id,
        json!({"version": mine.version, "expected_version": changed.version}),
        Caller::Agent,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let restored: RestorePageVersionResponse = serde_json::from_value(body).unwrap();
    assert_eq!(restored.restored_from, mine.version);
    let after = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(after.content, version_content(&db, &id, mine.version).await);
    assert_eq!(after.version, changed.version + 1);
    let mut sources = after.source_memory_ids.clone();
    sources.sort();
    assert_eq!(sources, SOURCES.map(String::from).to_vec());
    let entry = last_entry(&db, &id).await;
    assert_eq!(entry.edited_by, "agent_refresh");
    assert_eq!(entry.restored_from, Some(mine.version));
    assert_eq!(entry.person_asked, None);

    // An archived page is refused.
    db.archive_page(&id).await.unwrap();
    let archived = db.get_page(&id).await.unwrap().unwrap();
    let (status, body) = restore(
        &router,
        &id,
        json!({"version": mine.version, "expected_version": archived.version}),
        Caller::Agent,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(code(&body), "restore_page_archived", "{body}");
}

#[tokio::test]
async fn restore_version_of_a_page_outside_the_callers_space_reads_as_missing() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    db.create_space("home", None, false).await.unwrap();
    db.create_space("work", None, false).await.unwrap();
    let work = common::create_page_fixture(
        &db,
        "Work Note",
        "A work fact.",
        Some("work"),
        &[],
        "authored",
    )
    .await;
    let page = db.get_page(&work).await.unwrap().unwrap();
    let request = json!({"version": 1, "expected_version": page.version});
    let (outside, outside_body) = restore(
        &router,
        &work,
        request.clone(),
        Caller::AgentInSpace("home"),
    )
    .await;
    let (missing, missing_body) = restore(
        &router,
        "page_does_not_exist",
        request,
        Caller::AgentInSpace("home"),
    )
    .await;
    assert_eq!(outside, StatusCode::NOT_FOUND, "{outside_body}");
    assert_eq!((outside, outside_body), (missing, missing_body));
    assert_eq!(
        db.get_page(&work).await.unwrap().unwrap().version,
        page.version
    );
}

/// UX-WHO-2 over HTTP: a one-line change the person asked for lands even
/// though the page also cites a source its Space can no longer read, because
/// the sentence citing it is left as it was. Changing that sentence is still
/// refused, and so is the same write without the person's words.
#[tokio::test]
async fn a_person_asked_change_lands_when_an_untouched_sentence_cites_an_unreadable_source() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    db.create_space("work", None, false).await.unwrap();
    db.create_space("home", None, false).await.unwrap();
    let facts = [
        (
            "mem-cite-a",
            "Tokio runs async tasks on a work-stealing runtime scheduler.",
        ),
        (
            "mem-cite-b",
            "Interview notes say people forget the tags they invent.",
        ),
    ];
    let put = |id: &'static str, text: &'static str, space: &'static str| {
        let db = db.clone();
        async move {
            db.upsert_documents(vec![wenlan_core::sources::RawDocument {
                source: "memory".into(),
                source_id: id.into(),
                title: format!("title-{id}"),
                content: text.into(),
                source_agent: Some("test".into()),
                space: Some(space.into()),
                last_modified: chrono::Utc::now().timestamp(),
                ..Default::default()
            }])
            .await
            .unwrap();
        }
    };
    for (id, text) in facts {
        put(id, text, "work").await;
    }
    let sources: Vec<&str> = facts.iter().map(|(id, _)| *id).collect();
    let draft = format!("{}\n\n{}", facts[0].1, facts[1].1);
    let id = common::create_page_fixture(&db, "Notes", &draft, Some("work"), &sources, "distilled")
        .await;
    let cited = "Status: draft.\n\n\
        Tokio runs async tasks on a work-stealing runtime scheduler [1].\n\n\
        Interview notes say people forget the tags they invent [2].";
    let page = db.get_page(&id).await.unwrap().unwrap();
    let (status, body) = call(
        &router,
        Method::PUT,
        &format!("/api/pages/{id}"),
        Some(json!({
            "content": cited,
            "source_memory_ids": sources,
            "expected_version": page.version,
        })),
        Caller::Agent,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let before = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(before.citations.len(), 2, "{:?}", before.citations);

    // The second source moves to another Space: the page's Space cannot read it.
    put(facts[1].0, facts[1].1, "home").await;
    let moved = db.get_page(&id).await.unwrap().unwrap();

    let asked = cited.replace("Status: draft.", "Status: replaced by the new page.");
    let uri = format!("/api/pages/{id}");
    let write = |content: String, person_asked: Option<&'static str>| {
        let mut request = json!({
            "content": content,
            "source_memory_ids": sources,
            "expected_version": before.version,
        });
        if let Some(quote) = person_asked {
            request["person_asked"] = json!(quote);
        }
        call(&router, Method::PUT, &uri, Some(request), Caller::Agent)
    };
    let (status, body) = write(asked.clone(), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.to_string().contains("source_read_refused"), "{body}");

    let reworded = asked.replace("forget the tags", "never use the tags");
    let (status, body) = write(reworded, Some(QUOTE)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.to_string().contains("positions=2 "), "{body}");
    assert_eq!(
        db.get_page(&id).await.unwrap().unwrap().version,
        before.version
    );

    let (status, body) = write(asked.clone(), Some(QUOTE)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let landed = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(landed.content, asked);
    assert_eq!(landed.source_memory_ids.len(), 2);
    assert_eq!(landed.citations.len(), 2);
    assert_eq!(landed.citations[1].locator, before.citations[1].locator);
    assert_eq!(landed.citations[1].status, moved.citations[1].status);
    assert_eq!(
        landed.citations[1].cited_digest,
        before.citations[1].cited_digest
    );
    assert_eq!(last_entry(&db, &id).await.edited_by, "person_asked");
}
