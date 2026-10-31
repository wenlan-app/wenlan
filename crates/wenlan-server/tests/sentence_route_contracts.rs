// SPDX-License-Identifier: Apache-2.0
//! HTTP contract for who wrote each sentence of a page (UX-WHO-1..5):
//! the sentences view, sentence proposals the person accepts or rejects one
//! by one, the person's 「不對」, rechecking one sentence, and the page lock
//! as the only per-page AI-change setting (no "edit directly" mode).

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;
use wenlan_core::db::MemoryDB;
use wenlan_types::authorship::{
    AiEditMode, PageSentencesResponse, SentenceAcceptResponse, SentenceAuthor,
    SentenceFlagResponse, SentenceProposalResponse, SentenceProposalStatus,
    SentenceRecheckResponse,
};

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
    /// The app: no headers.
    Person,
    /// An automatic agent read: these routes serve automatic callers only.
    Reader,
    /// An agent writing, named by `x-agent-name`.
    Agent(&'static str),
    /// The app, scoped to one Space.
    InSpace(&'static str),
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
    match caller {
        Caller::Person => {}
        Caller::Reader => request = request.header("x-agent-name", "sentence-route-contract"),
        Caller::Agent(name) => request = request.header("x-agent-name", name),
        Caller::InSpace(space) => request = request.header("x-wenlan-space", space),
    }
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

fn typed<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value.clone())
        .unwrap_or_else(|error| panic!("not the expected shape ({error}): {value}"))
}

const SOURCES: [&str; 2] = ["mem-sentence-a", "mem-sentence-b"];
const AI_BODY: &str =
    "Tokio runs async tasks on a runtime.\n\nThe runtime uses a work-stealing scheduler.";
const MY_LINE: &str = "I prefer the current-thread runtime.";

/// An AI-written page that the person then adds one line to over the human
/// save route, so it is human-owned with one sentence of the person's.
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

async fn sentences(router: &common::AppRouter, id: &str) -> PageSentencesResponse {
    let (status, body) = call(
        router,
        Method::GET,
        &format!("/api/pages/{id}/sentences"),
        None,
        Caller::Reader,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    typed(body)
}

fn hash_of(view: &PageSentencesResponse, text: &str) -> String {
    view.sentences
        .iter()
        .find(|s| s.text.trim() == text)
        .unwrap_or_else(|| panic!("no sentence {text:?} in {view:?}"))
        .hash
        .clone()
}

#[tokio::test]
async fn the_sentences_view_tells_the_persons_sentences_from_the_ais() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let id = page_with_my_line(&router, &db).await;

    let view = sentences(&router, &id).await;
    assert_eq!(view.page_id, id);
    assert_eq!(view.ai_edits, AiEditMode::Propose);
    let mine = view
        .sentences
        .iter()
        .find(|s| s.text.trim() == MY_LINE)
        .unwrap();
    assert_eq!(mine.author, SentenceAuthor::Person);
    assert!(mine.protected);
    let ai = view
        .sentences
        .iter()
        .find(|s| s.text.contains("work-stealing"))
        .unwrap();
    assert_eq!(ai.author, SentenceAuthor::Ai);
    assert!(!ai.protected);

    // A missing page, and a page outside the caller's Space, read as absent.
    let (status, _) = call(
        &router,
        Method::GET,
        "/api/pages/page_does_not_exist/sentences",
        None,
        Caller::Reader,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Both Spaces exist; the caller reads from `home`.
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
    let (status, _) = call(
        &router,
        Method::GET,
        &format!("/api/pages/{work}/sentences"),
        None,
        Caller::InSpace("home"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a page in another Space reads as absent"
    );
}

#[tokio::test]
async fn an_agent_refresh_lands_on_the_ais_sentences_and_proposes_for_the_persons() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let id = page_with_my_line(&router, &db).await;
    let version = db.get_page(&id).await.unwrap().unwrap().version;

    let rewrite = "Tokio runs async tasks on a runtime.\n\n\
                   The runtime uses a work-stealing scheduler across worker threads.\n\n\
                   I prefer the multi-thread runtime.";
    let (status, body) = call(
        &router,
        Method::PUT,
        &format!("/api/pages/{id}"),
        Some(json!({
            "content": rewrite,
            "source_memory_ids": SOURCES,
            "expected_version": version,
        })),
        Caller::Agent("refresh-agent"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let landed = db.get_page(&id).await.unwrap().unwrap().content;
    assert!(landed.contains("across worker threads"), "{landed}");
    assert!(
        landed.contains(MY_LINE),
        "the person's line is kept: {landed}"
    );

    let view = sentences(&router, &id).await;
    assert_eq!(view.proposals.len(), 1, "{view:?}");
    let staged = &view.proposals[0];
    assert_eq!(staged.before.trim(), MY_LINE);
    assert_eq!(staged.after.trim(), "I prefer the multi-thread runtime.");
    assert_eq!(staged.status, SentenceProposalStatus::Pending);

    // Only the person rejects it; an agent is refused.
    let reject = format!("/api/pages/{id}/sentences/proposals/{}/reject", staged.id);
    let (status, _) = call(&router, Method::POST, &reject, None, Caller::Agent("x")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = call(&router, Method::POST, &reject, None, Caller::Person).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rejected: SentenceProposalResponse = typed(body);
    assert_eq!(rejected.proposal.status, SentenceProposalStatus::Rejected);
    assert!(sentences(&router, &id).await.proposals.is_empty());
    assert!(db
        .get_page(&id)
        .await
        .unwrap()
        .unwrap()
        .content
        .contains(MY_LINE));
}

#[tokio::test]
async fn proposals_are_made_by_the_ai_and_accepted_by_the_person() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let id = page_with_my_line(&router, &db).await;
    let view = sentences(&router, &id).await;
    let mine = hash_of(&view, MY_LINE);

    let (status, body) = call(
        &router,
        Method::POST,
        &format!("/api/pages/{id}/sentences/propose"),
        Some(json!({
            "sentence_hash": mine,
            "text": "I prefer the current-thread runtime for tests.",
            "why": "the tests pin it",
            "expected_version": view.version,
        })),
        Caller::Agent("proposer"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let proposed: SentenceProposalResponse = typed(body);
    assert_eq!(proposed.proposal.status, SentenceProposalStatus::Pending);
    assert_eq!(proposed.proposal.actor.agent.as_deref(), Some("proposer"));
    assert!(db
        .get_page(&id)
        .await
        .unwrap()
        .unwrap()
        .content
        .contains(MY_LINE));

    // A stale expected_version conflicts; unknown fields are refused.
    let (status, _) = call(
        &router,
        Method::POST,
        &format!("/api/pages/{id}/sentences/propose"),
        Some(json!({ "sentence_hash": mine, "text": "x.", "expected_version": 1 })),
        Caller::Agent("proposer"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(
        &router,
        Method::POST,
        &format!("/api/pages/{id}/sentences/propose"),
        Some(json!({ "sentence_hash": mine, "text": "x.", "extra": 1 })),
        Caller::Agent("proposer"),
    )
    .await;
    assert!(status.is_client_error(), "{status}");

    let accept = format!(
        "/api/pages/{id}/sentences/proposals/{}/accept",
        proposed.proposal.id
    );
    let (status, _) = call(
        &router,
        Method::POST,
        &accept,
        None,
        Caller::Agent("proposer"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = call(
        &router,
        Method::POST,
        &accept,
        Some(json!({ "why": "yes" })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let accepted: SentenceAcceptResponse = typed(body);
    assert_eq!(accepted.proposal.status, SentenceProposalStatus::Accepted);
    assert!(accepted.version > view.version);
    let after = sentences(&router, &id).await;
    let taken = after
        .sentences
        .iter()
        .find(|s| s.text.contains("for tests"))
        .unwrap();
    assert_eq!(taken.author, SentenceAuthor::Person);
    assert!(taken.accepted);
    // Accepting twice is refused.
    let (status, _) = call(&router, Method::POST, &accept, None, Caller::Person).await;
    assert!(status.is_client_error(), "{status}");
}

#[tokio::test]
async fn the_persons_flag_and_a_one_sentence_recheck() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let id = page_with_my_line(&router, &db).await;
    let view = sentences(&router, &id).await;
    let ai = view
        .sentences
        .iter()
        .find(|s| s.text.contains("work-stealing"))
        .unwrap()
        .hash
        .clone();

    let flag = format!("/api/pages/{id}/sentences/flag");
    let (status, _) = call(
        &router,
        Method::POST,
        &flag,
        Some(json!({ "sentence_hash": ai })),
        Caller::Agent("x"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = call(
        &router,
        Method::POST,
        &flag,
        Some(json!({ "sentence_hash": ai, "why": "wrong scheduler" })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let flagged: SentenceFlagResponse = typed(body);
    assert_eq!(flagged.sentence_hash, ai);
    let after = sentences(&router, &id).await;
    assert!(
        after
            .sentences
            .iter()
            .find(|s| s.hash == ai)
            .unwrap()
            .flagged
    );

    let recheck = format!("/api/pages/{id}/sentences/recheck");
    let (status, body) = call(
        &router,
        Method::POST,
        &recheck,
        Some(json!({ "sentence_hash": ai })),
        Caller::Reader,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rechecked: SentenceRecheckResponse = typed(body);
    assert_eq!(rechecked.sentence.hash, ai);
    // Recheck changes no text.
    assert_eq!(rechecked.version, after.version);
    let (status, _) = call(
        &router,
        Method::POST,
        &recheck,
        Some(json!({ "sentence_hash": "0000000000000000" })),
        Caller::Reader,
    )
    .await;
    assert!(status.is_client_error(), "{status}");
}

#[tokio::test]
async fn the_page_lock_is_the_only_ai_change_setting() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let id =
        common::create_page_fixture(&db, "Plain", "One sentence.", None, &[], "authored").await;
    let uri = format!("/api/pages/{id}/review-required");

    let (status, body) = call(
        &router,
        Method::PUT,
        &uri,
        Some(json!({ "required": true })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "page_id": id, "review_required": true, "ai_edits": "locked" })
    );
    let (_, body) = call(
        &router,
        Method::PUT,
        &uri,
        Some(json!({ "required": false })),
        Caller::Person,
    )
    .await;
    assert_eq!(body["ai_edits"], json!("propose"), "{body}");

    // There is no "AI may edit directly" mode, per page or globally.
    let (status, _) = call(
        &router,
        Method::PUT,
        &uri,
        Some(json!({ "required": false, "ai_direct_edit": true })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = call(&router, Method::GET, "/api/config", None, Caller::Person).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("ai_direct_edit").is_none(), "{body}");
}
