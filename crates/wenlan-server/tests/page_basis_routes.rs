// SPDX-License-Identifier: Apache-2.0
//! HTTP contract for a note that relies on another note (its basis).
//!
//! Numbers in the test names refer to the acceptance list in the B1 plan
//! (`2026-10-17-page-cites-page-and-lock-toggle.plan.md`).

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;
use wenlan_core::db::MemoryDB;
use wenlan_core::truth_contract::{CONTRACT_HEADER, INTENT_HEADER};
use wenlan_types::responses::{CreatePageResponse, ManualPageWriteResponse, PageWriteResponse};
use wenlan_types::{BasisConfirmOutcome, BasisRef};

/// Every test here points `WENLAN_DATA_DIR` (and so the knowledge/page path) at
/// its own scratch vault. The variable is process-wide, so each test holds this
/// lock for its whole body; no test may write to the developer's vault.
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
    /// An agent reading with the explicit-intent marker the read routes want.
    Reader,
    /// An agent writing, named by `x-agent-name`.
    Agent(&'static str),
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
        Caller::Reader => {
            request = request
                .header(INTENT_HEADER, "explicit")
                .header(CONTRACT_HEADER, "1")
                .header("x-agent-name", "basis-route-contract");
        }
        Caller::Agent(name) => request = request.header("x-agent-name", name),
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

async fn note(db: &Arc<MemoryDB>, title: &str, content: &str, space: Option<&str>) -> String {
    common::create_page_fixture(db, title, content, space, &[], "authored").await
}

/// A person saves new text over a note (the human save route).
async fn person_edits(router: &common::AppRouter, id: &str, content: &str) {
    let (status, body) = call(
        router,
        Method::POST,
        &format!("/api/memory/{id}/update-page"),
        Some(json!({ "content": content })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn read_note(router: &common::AppRouter, id: &str) -> Value {
    let (status, body) = call(
        router,
        Method::GET,
        &format!("/api/pages/{id}"),
        None,
        Caller::Reader,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn version_of(read: &Value) -> i64 {
    read["page"]["version"].as_i64().expect("page.version")
}

fn basis_of(read: &Value) -> Vec<BasisRef> {
    read.get("basis")
        .map(|b| typed(b.clone()))
        .unwrap_or_default()
}

fn basis_uri(page: &str, basis: &str) -> String {
    format!("/api/pages/{page}/basis/{basis}")
}

async fn mark(router: &common::AppRouter, page: &str, basis: &str, version: i64) -> BasisRef {
    let (status, body) = call(
        router,
        Method::PUT,
        &basis_uri(page, basis),
        Some(json!({ "version": version })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    typed(body)
}

// ---------------------------------------------------------------------------
// Acceptance 1, 3, 4, 5: mark, read, diff, confirm, group, remove
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptance_1_3_4_5_mark_change_diff_confirm_and_remove_over_http() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let basis = note(&db, "Launch Plan", "The launch is on 2026-03-01.", None).await;
    let citer = note(&db, "Roadmap", "Built on the launch plan.", None).await;
    let second = note(&db, "Budget", "Also built on the launch plan.", None).await;

    // 1: marking lists the basis as ok, with its title and versions.
    let marked = mark(&router, &citer, &basis, 1).await;
    assert_eq!(marked.page_id, basis);
    assert_eq!(marked.title, "Launch Plan");
    assert_eq!(marked.state, "ok");
    assert_eq!((marked.from_version, marked.to_version), (1, Some(1)));
    mark(&router, &second, &basis, 1).await;
    let read = read_note(&router, &citer).await;
    assert_eq!(basis_of(&read).len(), 1);
    assert_eq!(basis_of(&read)[0].state, "ok");

    // 3: the basis changes; the citer reads pending and the diff has both texts.
    person_edits(&router, &basis, "The launch is on 2026-04-01.").await;
    let read = read_note(&router, &citer).await;
    let rows = basis_of(&read);
    assert_eq!(rows[0].state, "pending");
    assert_eq!((rows[0].from_version, rows[0].to_version), (1, Some(2)));
    assert!(
        read["page"].get("basis").is_none(),
        "the note's own entry carries no second copy of its bases"
    );

    let (status, diff) = call(
        &router,
        Method::GET,
        &format!("{}/diff", basis_uri(&citer, &basis)),
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{diff}");
    assert_eq!(diff["from_version"], 1);
    assert_eq!(diff["to_version"], 2);
    assert_eq!(diff["from_content"], "The launch is on 2026-03-01.");
    assert_eq!(diff["to_content"], "The launch is on 2026-04-01.");
    assert_eq!(diff["edits"].as_array().unwrap().len(), 1);
    assert_eq!(diff["edits"][0]["version"], 2);

    // 5: two citers of one basis are one group.
    let (status, pending) = call(
        &router,
        Method::GET,
        "/api/pages/basis-pending",
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pending}");
    let groups = pending.as_array().unwrap();
    assert_eq!(groups.len(), 1, "{pending}");
    assert_eq!(groups[0]["basis_page_id"], basis.as_str());
    assert_eq!(groups[0]["title"], "Launch Plan");
    assert_eq!(groups[0]["state"], "pending");
    assert_eq!(groups[0]["changed_version"], 2);
    assert_eq!(groups[0]["citers"].as_array().unwrap().len(), 2);

    // 4: confirming a version the basis has moved past is a 409 and changes nothing.
    let confirm_uri = format!("{}/confirm", basis_uri(&citer, &basis));
    let (status, refused) = call(
        &router,
        Method::POST,
        &confirm_uri,
        Some(json!({ "version": 1 })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("version 2"),
        "the refusal names the version the basis is at: {refused}"
    );
    assert_eq!(
        basis_of(&read_note(&router, &citer).await)[0].state,
        "pending"
    );

    let (status, confirmed) = call(
        &router,
        Method::POST,
        &confirm_uri,
        Some(json!({ "version": 2 })),
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    let confirmed: BasisRef = typed(confirmed);
    assert_eq!(confirmed.state, "ok");
    assert_eq!((confirmed.from_version, confirmed.to_version), (2, Some(2)));

    // The other citer is still waiting; one group, one citer.
    let (_, pending) = call(
        &router,
        Method::GET,
        "/api/pages/basis-pending",
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(pending[0]["citers"].as_array().unwrap().len(), 1);
    assert_eq!(pending[0]["citers"][0]["page_id"], second.as_str());

    // Remove: once true, then false; the note no longer lists it.
    for expected in [true, false] {
        let (status, removed) = call(
            &router,
            Method::DELETE,
            &basis_uri(&citer, &basis),
            None,
            Caller::Person,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{removed}");
        assert_eq!(removed, json!({ "removed": expected }));
    }
    assert!(basis_of(&read_note(&router, &citer).await).is_empty());
    assert!(read_note(&router, &citer).await.get("basis").is_none());
}

#[tokio::test]
async fn a_comparison_that_cannot_be_made_is_409_with_a_reason_and_bad_pairs_are_refused() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let basis = note(&db, "Launch Plan", "The launch is on 2026-03-01.", None).await;
    let citer = note(&db, "Roadmap", "Built on the launch plan.", None).await;
    mark(&router, &citer, &basis, 1).await;

    // Not 404, not 422: a note that is not a basis of this one.
    let (status, _) = call(
        &router,
        Method::GET,
        &format!("{}/diff", basis_uri(&basis, &citer)),
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &router,
        Method::PUT,
        &basis_uri(&citer, "no-such-note"),
        Some(json!({ "version": 1 })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The rules the daemon enforces: not itself, a version the note has had.
    for (target, version) in [(citer.as_str(), 1), (basis.as_str(), 9)] {
        let (status, body) = call(
            &router,
            Method::PUT,
            &basis_uri(&citer, target),
            Some(json!({ "version": version })),
            Caller::Person,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{target}: {body}");
    }
    // A body that names anything but a version is refused.
    let (status, _) = call(
        &router,
        Method::PUT,
        &basis_uri(&citer, &basis),
        Some(json!({ "version": 1, "force": true })),
        Caller::Person,
    )
    .await;
    assert!(status.is_client_error(), "{status}");

    // The basis is deleted: the comparison is gone, with the reason, and the
    // citer is told which note it relied on.
    let (status, _) = call(
        &router,
        Method::DELETE,
        &format!("/api/pages/{basis}"),
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, gone) = call(
        &router,
        Method::GET,
        &format!("{}/diff", basis_uri(&citer, &basis)),
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{gone}");
    assert_eq!(gone["reason"], "deleted");
    assert!(gone["error"].as_str().unwrap().contains("not available"));

    let read = read_note(&router, &citer).await;
    let rows = basis_of(&read);
    assert_eq!(rows[0].state, "deleted");
    assert_eq!(rows[0].title, "Launch Plan");
    assert_eq!(rows[0].to_version, None);

    let (status, _) = call(
        &router,
        Method::POST,
        &format!("{}/confirm", basis_uri(&citer, &basis)),
        Some(json!({ "version": 1 })),
        Caller::Person,
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "a deleted basis cannot be confirmed"
    );
    // Removing it still works.
    let (status, removed) = call(
        &router,
        Method::DELETE,
        &basis_uri(&citer, &basis),
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(removed, json!({ "removed": true }));
}

// ---------------------------------------------------------------------------
// R7 and acceptance 9: an agent's write carries what it relied on
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_note_relies_on_a_note_in_another_space_unless_the_caller_is_held_to_one() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    db.create_space("home", None, false).await.unwrap();
    db.create_space("work", None, false).await.unwrap();
    let citer = note(&db, "Roadmap", "Built on the work plan.", Some("home")).await;
    let basis = note(&db, "Work Plan", "Ship in March.", Some("work")).await;

    // An agent held to Home cannot see the Work note: absent, as on a read.
    let scoped = |method: Method, uri: String| {
        let router = router.clone();
        async move {
            let response = router
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header("content-type", "application/json")
                        .header("x-agent-name", "claude-code")
                        .header("x-wenlan-space", "home")
                        .body(Body::from(json!({ "version": 1 }).to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            response.status()
        }
    };
    assert_eq!(
        scoped(Method::PUT, basis_uri(&citer, &basis)).await,
        StatusCode::NOT_FOUND
    );
    assert!(basis_of(&read_note(&router, &citer).await).is_empty());

    // The app can, and the note it relies on is named, compared and confirmed
    // like one in the same Space.
    let marked = mark(&router, &citer, &basis, 1).await;
    assert_eq!(marked.title, "Work Plan");
    assert_eq!(marked.state, "ok");
    person_edits(&router, &basis, "Ship in April.").await;
    // The app filtering Home still names the Work note its note waits on.
    let (status, pending) = call(
        &router,
        Method::GET,
        "/api/pages/basis-pending?space=home",
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pending}");
    assert_eq!(pending[0]["basis_page_id"], basis.as_str(), "{pending}");
    assert_eq!(pending[0]["title"], "Work Plan", "{pending}");
    let (status, diff) = call(
        &router,
        Method::GET,
        &format!("{}/diff", basis_uri(&citer, &basis)),
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{diff}");
    let (status, confirmed) = call(
        &router,
        Method::POST,
        &format!("{}/confirm", basis_uri(&citer, &basis)),
        Some(json!({ "version": 2 })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["state"], "ok");
    // Still out of reach for the agent held to Home.
    assert_eq!(
        scoped(
            Method::POST,
            format!("{}/confirm", basis_uri(&citer, &basis))
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn acceptance_9_an_agent_write_marks_a_moved_basis_pending_and_a_current_one_ok() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let moved = note(&db, "Launch Plan", "The launch is on 2026-03-01.", None).await;
    let steady = note(&db, "Glossary", "Terms that do not change.", None).await;
    let citer = note(&db, "Roadmap", "Original plans.", None).await;
    person_edits(&router, &moved, "The launch is on 2026-04-01.").await;
    let version = version_of(&read_note(&router, &citer).await);

    let (status, wrote) = call(
        &router,
        Method::PUT,
        &format!("/api/pages/{citer}"),
        Some(json!({
            "content": "Revised plans, built on two notes.",
            "source_memory_ids": [],
            "expected_version": version,
            // The agent read "moved" at version 1; it is at 2 by now.
            "basis": [
                { "page_id": moved, "version": 1 },
                { "page_id": steady, "version": 1 }
            ],
        })),
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{wrote}");
    let wrote: PageWriteResponse = typed(wrote);
    assert!(wrote.ok && !wrote.gated);
    let by_id = |id: &str| {
        wrote
            .basis
            .iter()
            .find(|b| b.page_id == id)
            .unwrap()
            .clone()
    };
    assert_eq!(by_id(&moved).state, "pending");
    assert_eq!(by_id(&moved).to_version, Some(2));
    assert_eq!(by_id(&steady).state, "ok");

    // The write response and read_page agree.
    let read = read_note(&router, &citer).await;
    assert_eq!(basis_of(&read), wrote.basis);
    assert_eq!(
        read["page"]["content"],
        "Revised plans, built on two notes."
    );
}

#[tokio::test]
async fn a_refused_basis_or_a_note_id_in_the_source_list_writes_nothing() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let other = note(&db, "Launch Plan", "The launch is on 2026-03-01.", None).await;
    let citer = note(&db, "Roadmap", "Original plans.", None).await;
    let version = version_of(&read_note(&router, &citer).await);
    let write = |extra: Value| {
        let router = router.clone();
        let citer = citer.clone();
        async move {
            let mut body = json!({
                "content": "Changed plans.",
                "source_memory_ids": [],
                "expected_version": version,
            });
            for (key, value) in extra.as_object().unwrap() {
                body[key] = value.clone();
            }
            call(
                &router,
                Method::PUT,
                &format!("/api/pages/{citer}"),
                Some(body),
                Caller::Agent("claude-code"),
            )
            .await
        }
    };

    // A note id in `source_memory_ids` is refused and says where notes go.
    let (status, body) = write(json!({ "source_memory_ids": [other] })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("use `basis` for notes"),
        "{body}"
    );

    // A basis that does not exist refuses the whole write.
    let (status, body) =
        write(json!({ "basis": [{ "page_id": other, "version": 1 }, { "page_id": "ghost", "version": 1 }] }))
            .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    // So does a version the basis never had.
    let (status, body) = write(json!({ "basis": [{ "page_id": other, "version": 7 }] })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    let read = read_note(&router, &citer).await;
    assert_eq!(read["page"]["content"], "Original plans.");
    assert_eq!(version_of(&read), version);
    assert!(basis_of(&read).is_empty(), "the first mark rolled back too");
}

#[tokio::test]
async fn creating_a_note_with_basis_marks_it_and_refuses_a_bad_basis_before_creating() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let basis = note(&db, "Launch Plan", "The launch is on 2026-03-01.", None).await;
    person_edits(&router, &basis, "The launch is on 2026-04-01.").await;
    let create = |body: Value| {
        let router = router.clone();
        async move {
            call(
                &router,
                Method::POST,
                "/api/pages",
                Some(body),
                Caller::Agent("claude-code"),
            )
            .await
        }
    };

    // `space: null` is Uncategorized, which is where the basis lives.
    let (status, created) = create(json!({
        "title": "Derived Plan",
        "content": "Derived from the launch plan.",
        "space": null,
        "creation_kind": "authored",
        "basis": [{ "page_id": basis, "version": 1 }],
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let created: CreatePageResponse = typed(created);
    assert_eq!(created.basis.len(), 1);
    assert_eq!(created.basis[0].page_id, basis);
    assert_eq!(
        created.basis[0].state, "pending",
        "the basis moved on after the version read"
    );
    assert_eq!(
        basis_of(&read_note(&router, &created.id).await),
        created.basis,
        "read_page shows what the create said"
    );

    let (listed_before, _) = (
        call(
            &router,
            Method::GET,
            "/api/pages?limit=50",
            None,
            Caller::Reader,
        )
        .await
        .1["pages"]
            .as_array()
            .unwrap()
            .len(),
        (),
    );
    // The `domain` alias still names a Space through the flattened body: the
    // basis is in another Space, so the create is refused.
    let (status, body) = create(json!({
        "title": "Elsewhere Plan",
        "content": "Wants a basis from another Space.",
        "domain": "work",
        "creation_kind": "authored",
        "basis": [{ "page_id": basis, "version": 2 }],
    }))
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    for bad in [
        json!({ "page_id": "ghost", "version": 1 }),
        json!({ "page_id": basis, "version": 9 }),
    ] {
        let (status, body) = create(json!({
            "title": "Refused Plan",
            "content": "Never created.",
            "creation_kind": "authored",
            "basis": [bad],
        }))
        .await;
        assert!(status.is_client_error(), "{status}: {body}");
    }
    let (status, body) = create(json!({
        "title": "Memory Plan",
        "content": "Names a note as a source.",
        "creation_kind": "authored",
        "source_memory_ids": [basis],
    }))
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("use `basis` for notes"));

    let (_, listed) = call(
        &router,
        Method::GET,
        "/api/pages?limit=50",
        None,
        Caller::Reader,
    )
    .await;
    assert_eq!(
        listed["pages"].as_array().unwrap().len(),
        listed_before,
        "no refused create left a note behind"
    );
}

/// Make `page_id` hidden from an automatic reader, the way a failed support
/// judgement does after the truth cutover.
async fn hide_from_automatic_readers(tmp: &tempfile::TempDir, db: &Arc<MemoryDB>, page_id: &str) {
    let page = db.get_page(page_id).await.unwrap().unwrap();
    let fixture = libsql::Builder::new_local(tmp.path().join("origin_memory.db"))
        .build()
        .await
        .unwrap();
    let conn = fixture.connect().unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO page_truth_state
            (page_id,page_version,support_status,human_reviewed,updated_at,evaluated_at)
         VALUES (?1,?2,'provisional',0,1,1)",
        libsql::params![page_id, page.version],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO claim_derivation_markers
            (page_id,page_version,page_version_digest,extractor_version,inventory_count,created_at)
         VALUES (?1,?2,?3,?4,1,0)",
        libsql::params![
            page_id,
            page.version,
            wenlan_core::provenance::revision_content_digest(&page.content),
            wenlan_core::db::EXTRACTOR_VERSION
        ],
    )
    .await
    .unwrap();
    db.set_app_metadata("claim_promoter_enforcement", "1")
        .await
        .unwrap();
    db.set_truth_cutover_generation(1).await.unwrap();
    let seen = db
        .page_visibility(
            &wenlan_core::truth_contract::TruthGrant::Automatic,
            std::slice::from_ref(&page_id.to_string()),
        )
        .await
        .unwrap();
    assert_eq!(
        seen.get(page_id),
        Some(&wenlan_core::truth_contract::Visibility::Hidden),
        "the fixture must be genuinely hidden"
    );
}

#[tokio::test]
async fn the_comparison_with_an_archived_basis_is_409_archived() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let basis = note(&db, "Launch Plan", "The launch is on 2026-03-01.", None).await;
    let citer = note(&db, "Roadmap", "Built on the launch plan.", None).await;
    mark(&router, &citer, &basis, 1).await;
    person_edits(&router, &basis, "The launch is on 2026-04-01.").await;
    db.archive_page(&basis).await.unwrap();

    let (status, body) = call(
        &router,
        Method::GET,
        &format!("{}/diff", basis_uri(&citer, &basis)),
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["reason"], "archived");
}

#[tokio::test]
async fn a_basis_the_caller_cannot_see_reads_as_absent_on_create_and_on_agent_update() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, tmp, db) = common::test_app_no_gate().await;
    let hidden = note(&db, "Secret Plan", "Not for automatic readers.", None).await;
    let citer = note(&db, "Roadmap", "Original plans.", None).await;
    db.create_space("other", None, false).await.unwrap();
    let version = version_of(&read_note(&router, &citer).await);

    // A Space-scoped create cannot see a note outside that Space: absent (404),
    // not the 422 the same-Space rule would give for a note that exists.
    let (status, body) = {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/pages")
                    .header("content-type", "application/json")
                    .header("x-agent-name", "claude-code")
                    .header("x-wenlan-space", "other")
                    .body(Body::from(
                        json!({
                            "title": "Derived Plan",
                            "content": "Derived from a note in no Space.",
                            "creation_kind": "authored",
                            "basis": [{ "page_id": hidden, "version": 1 }],
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice::<Value>(&bytes).unwrap())
    };
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], format!("page {hidden} not found"));

    hide_from_automatic_readers(&tmp, &db, &hidden).await;

    // A hidden basis reads as absent whatever version is claimed, so a probe
    // cannot learn whether the note exists or what version it is at.
    for claimed in [1, 9] {
        let (status, body) = call(
            &router,
            Method::POST,
            "/api/pages",
            Some(json!({
                "title": "Derived Plan",
                "content": "Derived from a hidden note.",
                "creation_kind": "authored",
                "basis": [{ "page_id": hidden, "version": claimed }],
            })),
            Caller::Agent("claude-code"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "create, v{claimed}: {body}");
        assert_eq!(body["error"], format!("page {hidden} not found"));

        let (status, body) = call(
            &router,
            Method::PUT,
            &format!("/api/pages/{citer}"),
            Some(json!({
                "content": "Changed plans.",
                "source_memory_ids": [],
                "expected_version": version,
                "basis": [{ "page_id": hidden, "version": claimed }],
            })),
            Caller::Agent("claude-code"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "update, v{claimed}: {body}");
        assert_eq!(body["error"], format!("page {hidden} not found"));
    }

    // Nothing was written by either refusal.
    let read = read_note(&router, &citer).await;
    assert_eq!(read["page"]["content"], "Original plans.");
    assert_eq!(version_of(&read), version);
    let (_, listed) = call(
        &router,
        Method::GET,
        "/api/pages?limit=50",
        None,
        Caller::Reader,
    )
    .await;
    assert!(
        listed["pages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["title"] != "Derived Plan"),
        "a refused create left a note behind: {listed}"
    );
}

// ---------------------------------------------------------------------------
// R11: a person's save confirms in its own transaction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn r11_the_save_confirms_a_basis_only_while_it_is_still_at_the_version_looked_at() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let basis = note(&db, "Launch Plan", "The launch is on 2026-03-01.", None).await;
    let citer = note(&db, "Roadmap", "Built on the launch plan.", None).await;
    mark(&router, &citer, &basis, 1).await;
    person_edits(&router, &basis, "The launch is on 2026-04-01.").await;

    let save = |content: &'static str, confirm: Option<i64>, minor: Option<bool>| {
        let router = router.clone();
        let citer = citer.clone();
        let basis = basis.clone();
        async move {
            let mut body = json!({ "content": content });
            if let Some(version) = confirm {
                body["confirm_basis"] = json!({ "basis_page_id": basis, "version": version });
            }
            if let Some(minor) = minor {
                body["minor"] = json!(minor);
            }
            let (status, response) = call(
                &router,
                Method::POST,
                &format!("/api/memory/{citer}/update-page"),
                Some(body),
                Caller::Person,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{response}");
            response
        }
    };

    // No confirm_basis: the response says nothing about one, and nothing is confirmed.
    let plain = save("Built on the launch plan, tidied.", None, None).await;
    assert!(plain.get("basis_confirm").is_none(), "{plain}");
    assert_eq!(
        basis_of(&read_note(&router, &citer).await)[0].state,
        "pending"
    );

    // The person looked at version 2 and saved: confirmed in the save.
    let saved: ManualPageWriteResponse =
        typed(save("Built on the launch plan, rechecked.", Some(2), None).await);
    assert_eq!(saved.basis_confirm, Some(BasisConfirmOutcome::Confirmed));
    assert_eq!(basis_of(&read_note(&router, &citer).await)[0].state, "ok");

    // The basis moves on while the person is still looking at version 2.
    person_edits(&router, &basis, "The launch is on 2026-05-01.").await;
    let saved: ManualPageWriteResponse =
        typed(save("Built on the launch plan, rechecked again.", Some(2), None).await);
    assert!(saved.ok, "the save still lands");
    assert_eq!(saved.basis_confirm, Some(BasisConfirmOutcome::MovedOn));
    let read = read_note(&router, &citer).await;
    assert_eq!(
        read["page"]["content"],
        "Built on the launch plan, rechecked again."
    );
    assert_eq!(basis_of(&read)[0].state, "pending");

    // A save that changes nothing has nothing to confirm with.
    let unchanged: ManualPageWriteResponse =
        typed(save("Built on the launch plan, rechecked again.", Some(3), None).await);
    assert_eq!(unchanged.basis_confirm, Some(BasisConfirmOutcome::NotSaved));
    assert_eq!(
        basis_of(&read_note(&router, &citer).await)[0].state,
        "pending"
    );

    // A minor edit of the basis shows as minor in the comparison.
    let (status, _) = call(
        &router,
        Method::POST,
        &format!("/api/memory/{basis}/update-page"),
        Some(json!({ "content": "The launch is on 2026-05-01 .", "minor": true })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, diff) = call(
        &router,
        Method::GET,
        &format!("{}/diff", basis_uri(&citer, &basis)),
        None,
        Caller::Person,
    )
    .await;
    let edits = diff["edits"].as_array().unwrap();
    assert_eq!(edits.last().unwrap()["minor"], true, "{diff}");
    assert_eq!(edits.first().unwrap()["minor"], false, "{diff}");
}

// ---------------------------------------------------------------------------
// Lists and recall carry only what needs a look
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_and_recall_carry_only_the_bases_that_are_not_ok() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    // Notes reach a recall hit only for a caller with review or full trust.
    db.register_agent("basis-recall").await.unwrap();
    db.update_agent("basis-recall", None, None, None, Some("review"), None)
        .await
        .unwrap();
    let basis = note(&db, "Launch Plan", "The launch is on 2026-03-01.", None).await;
    let waiting = note(
        &db,
        "Waiting Roadmap",
        "Roadmap built on the launch plan.",
        None,
    )
    .await;
    let quiet = note(
        &db,
        "Quiet Budget",
        "Budget built on the launch plan.",
        None,
    )
    .await;
    mark(&router, &waiting, &basis, 1).await;
    mark(&router, &quiet, &basis, 1).await;
    person_edits(&router, &basis, "The launch is on 2026-04-01.").await;
    // Settle one citer; the other keeps waiting.
    let (status, _) = call(
        &router,
        Method::POST,
        &format!("{}/confirm", basis_uri(&quiet, &basis)),
        Some(json!({ "version": 2 })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, listed) = call(
        &router,
        Method::GET,
        "/api/pages?limit=50",
        None,
        Caller::Reader,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let row = |id: &str| {
        listed["pages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing from {listed}"))
            .clone()
    };
    let waiting_basis: Vec<BasisRef> = typed(row(&waiting)["basis"].clone());
    assert_eq!(waiting_basis.len(), 1);
    assert_eq!(waiting_basis[0].state, "pending");
    assert_eq!(waiting_basis[0].page_id, basis);
    assert!(
        row(&quiet).get("basis").is_none(),
        "an ok basis is not listed"
    );
    assert!(row(&basis).get("basis").is_none());

    // Recall: a hit on the waiting note says so; a hit on the quiet one does not.
    let (status, recalled) = call(
        &router,
        Method::POST,
        "/api/memory/search",
        Some(json!({ "query": "Roadmap built on the launch plan", "limit": 10 })),
        Caller::Agent("basis-recall"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{recalled}");
    let hits = recalled["supplemental_pages"]
        .as_array()
        .unwrap_or_else(|| panic!("the note channel returned nothing: {recalled}"));
    let waiting_hit = hits
        .iter()
        .find(|h| h["source_id"] == waiting.as_str())
        .unwrap_or_else(|| panic!("the waiting note is not in the hits: {recalled}"));
    let carried: Vec<BasisRef> = typed(waiting_hit["basis"].clone());
    assert_eq!(carried[0].state, "pending");
    for hit in hits.iter().filter(|h| h["source_id"] != waiting.as_str()) {
        assert!(
            hit.get("basis").is_none(),
            "only the waiting note carries a basis: {hit}"
        );
    }
}

// ---------------------------------------------------------------------------
// Arc 2 review: a Space-scoped agent write, and a locked note's basis marks
// ---------------------------------------------------------------------------

/// An agent's `PUT /api/pages/{id}`, optionally scoped by `x-wenlan-space`.
async fn agent_put(
    router: &common::AppRouter,
    id: &str,
    body: Value,
    space: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(Method::PUT)
        .uri(format!("/api/pages/{id}"))
        .header("content-type", "application/json")
        .header("x-agent-name", "claude-code");
    if let Some(space) = space {
        request = request.header("x-wenlan-space", space);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn pending_cards(db: &Arc<MemoryDB>) -> Vec<String> {
    db.list_pending_revisions(50)
        .await
        .unwrap()
        .into_iter()
        .map(|card| card.revision_source_id)
        .collect()
}

async fn accept(router: &common::AppRouter, card_id: &str) -> (StatusCode, Value) {
    call(
        router,
        Method::POST,
        &format!("/api/memory/revision/{card_id}/accept"),
        None,
        Caller::Person,
    )
    .await
}

#[tokio::test]
async fn an_agent_write_scoped_to_one_space_answers_a_note_in_another_exactly_like_a_missing_one() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    db.create_space("alpha", None, false).await.unwrap();
    db.create_space("beta", None, false).await.unwrap();
    let basis = note(
        &db,
        "Launch Plan",
        "The launch is on 2026-03-01.",
        Some("beta"),
    )
    .await;
    let open = note(&db, "Roadmap", "Original plans.", Some("beta")).await;
    let locked = note(&db, "Budget", "Original budget.", Some("beta")).await;
    assert!(db.set_page_review_required(&locked, true).await.unwrap());
    // The scoped requests carry no basis, so the refusal can only come from
    // the note itself being out of scope (a basis in another Space is refused
    // earlier, as absent, by its own check).
    let body = |version: i64, with_basis: bool| {
        let mut body = json!({
            "content": "Changed by an agent scoped elsewhere.",
            "source_memory_ids": [],
            "expected_version": version,
        });
        if with_basis {
            body["basis"] = json!([{ "page_id": basis, "version": 1 }]);
        }
        body
    };

    let (missing_status, missing) =
        agent_put(&router, "ghost", body(1, false), Some("alpha")).await;
    assert_eq!(
        missing_status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{missing}"
    );
    assert_eq!(missing, json!({ "error": "page ghost not found" }));

    for id in [&open, &locked] {
        let version = db.get_page(id).await.unwrap().unwrap().version;
        let before = db.get_page(id).await.unwrap().unwrap();
        let (status, refused) = agent_put(&router, id, body(version, false), Some("alpha")).await;
        assert_eq!(status, missing_status, "{id}: {refused}");
        // The same body as a missing note, id aside: no basis, no card id,
        // nothing that says the note exists.
        assert_eq!(refused, json!({ "error": format!("page {id} not found") }));
        let after = db.get_page(id).await.unwrap().unwrap();
        assert_eq!(after.content, before.content, "{id}: nothing written");
        assert_eq!(after.version, before.version, "{id}: nothing written");
        assert!(
            db.list_page_basis(id).await.unwrap().is_empty(),
            "{id}: no mark"
        );
        assert!(pending_cards(&db).await.is_empty(), "{id}: no card staged");
    }

    // The same requests without the header (the app's global scope) work: the
    // open note takes the write, the locked one stages a card.
    let version = db.get_page(&open).await.unwrap().unwrap().version;
    let (status, wrote) = agent_put(&router, &open, body(version, true), None).await;
    assert_eq!(status, StatusCode::OK, "{wrote}");
    let wrote: PageWriteResponse = typed(wrote);
    assert!(wrote.ok && !wrote.gated);
    assert_eq!(wrote.basis.len(), 1);
    let version = db.get_page(&locked).await.unwrap().unwrap().version;
    let (status, gated) = agent_put(&router, &locked, body(version, true), None).await;
    assert_eq!(status, StatusCode::OK, "{gated}");
    let gated: PageWriteResponse = typed(gated);
    assert!(gated.gated && gated.revision_card_id.is_some());
    // And the right Space header works too.
    let version = db.get_page(&open).await.unwrap().unwrap().version;
    let (status, wrote) = agent_put(
        &router,
        &open,
        json!({
            "content": "Changed again, in its own Space.",
            "source_memory_ids": [],
            "expected_version": version,
        }),
        Some("beta"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{wrote}");
}

/// A Space-scoped agent that puts a note id from another Space in its source
/// list hears exactly what it hears for an id that names nothing, so the
/// answer cannot confirm the note exists. A note in its own Space is named.
#[tokio::test]
async fn a_scoped_agent_cannot_learn_from_the_source_list_that_a_note_in_another_space_exists() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    db.create_space("alpha", None, false).await.unwrap();
    db.create_space("beta", None, false).await.unwrap();
    let elsewhere = note(
        &db,
        "Launch Plan",
        "The launch is on 2026-03-01.",
        Some("beta"),
    )
    .await;
    let sibling = note(&db, "Glossary", "Terms.", Some("alpha")).await;
    let target = note(&db, "Roadmap", "Original plans.", Some("alpha")).await;
    let put = |source: String| {
        let router = router.clone();
        let target = target.clone();
        let db = db.clone();
        async move {
            let version = db.get_page(&target).await.unwrap().unwrap().version;
            agent_put(
                &router,
                &target,
                json!({
                    "content": "Changed by an agent.",
                    "source_memory_ids": [source],
                    "expected_version": version,
                }),
                Some("alpha"),
            )
            .await
        }
    };

    let (missing_status, missing) = put("ghost".into()).await;
    let (status, refused) = put(elsewhere.clone()).await;
    assert_eq!(status, missing_status, "{refused}");
    assert_eq!(
        refused.to_string().replace(&elsewhere, "ghost"),
        missing.to_string(),
        "the same answer as a missing id"
    );
    assert!(!refused.to_string().contains("is a note"), "{refused}");

    let (status, named) = put(sibling.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{named}");
    assert!(
        named.to_string().contains(&format!("{sibling} is a note")),
        "{named}"
    );
}

/// The bases a write answers with are shown within the writer's Space: a
/// basis that moved to another Space keeps its state, but not its title.
#[tokio::test]
async fn a_scoped_write_withholds_the_title_of_a_basis_that_moved_to_another_space() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    db.create_space("alpha", None, false).await.unwrap();
    db.create_space("beta", None, false).await.unwrap();
    let basis = note(
        &db,
        "Launch Plan",
        "The launch is on 2026-03-01.",
        Some("alpha"),
    )
    .await;
    let citer = note(&db, "Roadmap", "Original plans.", Some("alpha")).await;
    mark(&router, &citer, &basis, 1).await;
    db.set_page_workspace(&basis, Some("beta")).await.unwrap();
    let write = |content: &'static str, space: Option<&'static str>| {
        let router = router.clone();
        let citer = citer.clone();
        let db = db.clone();
        async move {
            let version = db.get_page(&citer).await.unwrap().unwrap().version;
            let (status, body) = agent_put(
                &router,
                &citer,
                json!({
                    "content": content,
                    "source_memory_ids": [],
                    "expected_version": version,
                }),
                space,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            typed::<PageWriteResponse>(body)
        }
    };

    let scoped = write("Changed in its own Space.", Some("alpha")).await;
    assert_eq!(scoped.basis.len(), 1);
    assert_eq!(scoped.basis[0].page_id, basis);
    assert_eq!(scoped.basis[0].title, "", "title withheld");

    let global = write("Changed by the app.", None).await;
    assert_eq!(global.basis[0].title, "Launch Plan");
}

#[tokio::test]
async fn a_locked_notes_agent_write_keeps_its_marks_on_the_card_and_accepting_records_them() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let fresh = note(&db, "Launch Plan", "The launch is on 2026-03-01.", None).await;
    let moved = note(&db, "Glossary", "Terms that rarely change.", None).await;
    let citer = note(&db, "Roadmap", "Original plans.", None).await;
    // The citer already relies on `moved` at version 1; `moved` changes, so
    // the agent's write reconfirms it at version 2.
    mark(&router, &citer, &moved, 1).await;
    person_edits(&router, &moved, "Terms that changed once.").await;
    assert_eq!(
        basis_of(&read_note(&router, &citer).await)[0].state,
        "pending"
    );
    assert!(db.set_page_review_required(&citer, true).await.unwrap());
    let version = version_of(&read_note(&router, &citer).await);

    let (status, gated) = agent_put(
        &router,
        &citer,
        json!({
            "content": "Plans built on the launch plan and the glossary.",
            "source_memory_ids": [],
            "expected_version": version,
            "basis": [
                { "page_id": fresh, "version": 1 },
                { "page_id": moved, "version": 2 }
            ],
        }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{gated}");
    let gated: PageWriteResponse = typed(gated);
    assert!(gated.gated);
    let card_id = gated.revision_card_id.unwrap();
    // Waiting on review: nothing has landed yet.
    let read = read_note(&router, &citer).await;
    assert_eq!(read["page"]["content"], "Original plans.");
    assert_eq!(basis_of(&read).len(), 1);
    assert_eq!(basis_of(&read)[0].state, "pending");

    let (status, accepted) = accept(&router, &card_id).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    let read = read_note(&router, &citer).await;
    assert_eq!(
        read["page"]["content"],
        "Plans built on the launch plan and the glossary."
    );
    let basis = basis_of(&read);
    let by_id = |id: &str| basis.iter().find(|b| b.page_id == id).unwrap().clone();
    assert_eq!(by_id(&fresh).state, "ok");
    assert_eq!(by_id(&moved).state, "ok", "the reconfirm landed on accept");
    assert_eq!(by_id(&moved).from_version, 2);
}

#[tokio::test]
async fn a_locked_notes_agent_write_with_a_refused_mark_is_refused_before_a_card_is_staged() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let archived = note(&db, "Old Plan", "Superseded plans.", None).await;
    let citer = note(&db, "Roadmap", "Original plans.", None).await;
    db.archive_page(&archived).await.unwrap();
    assert!(db.set_page_review_required(&citer, true).await.unwrap());
    let version = version_of(&read_note(&router, &citer).await);

    let (status, body) = agent_put(
        &router,
        &citer,
        json!({
            "content": "Plans built on the old plan.",
            "source_memory_ids": [],
            "expected_version": version,
            "basis": [{ "page_id": archived, "version": 1 }],
        }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(pending_cards(&db).await.is_empty(), "no card was staged");
}

#[tokio::test]
async fn accepting_a_locked_notes_card_whose_basis_was_archived_since_is_409_and_discards_it() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let basis = note(&db, "Launch Plan", "The launch is on 2026-03-01.", None).await;
    let citer = note(&db, "Roadmap", "Original plans.", None).await;
    assert!(db.set_page_review_required(&citer, true).await.unwrap());
    let version = version_of(&read_note(&router, &citer).await);
    let (status, gated) = agent_put(
        &router,
        &citer,
        json!({
            "content": "Plans built on the launch plan.",
            "source_memory_ids": [],
            "expected_version": version,
            "basis": [{ "page_id": basis, "version": 1 }],
        }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{gated}");
    let card_id = typed::<PageWriteResponse>(gated).revision_card_id.unwrap();

    db.archive_page(&basis).await.unwrap();
    let (status, refused) = accept(&router, &card_id).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let message = refused["error"].as_str().unwrap();
    assert!(
        message.starts_with(&format!("{}:", wenlan_types::PAGE_REVISION_BASIS_REFUSED)),
        "{message}"
    );
    assert!(message.contains(&basis), "names the note: {message}");
    let read = read_note(&router, &citer).await;
    assert_eq!(read["page"]["content"], "Original plans.");
    assert_eq!(version_of(&read), version);
    assert!(basis_of(&read).is_empty());
    assert!(
        pending_cards(&db).await.is_empty(),
        "the card is not stranded"
    );
    let (status, _) = accept(&router, &card_id).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a second accept finds nothing"
    );
}

#[tokio::test]
async fn an_agent_write_that_changes_nothing_still_records_its_basis_marks() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let basis = note(&db, "Launch Plan", "The launch is on 2026-03-01.", None).await;
    let citer = note(&db, "Roadmap", "Original plans.", None).await;
    let version = version_of(&read_note(&router, &citer).await);
    let unchanged = |marks: Value| {
        json!({
            "content": "Original plans.",
            "source_memory_ids": [],
            "expected_version": version,
            "basis": marks,
        })
    };

    // A refused mark still refuses the request, and records nothing.
    let (status, body) = agent_put(
        &router,
        &citer,
        unchanged(json!([{ "page_id": basis, "version": 1 }, { "page_id": basis, "version": 9 }])),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(basis_of(&read_note(&router, &citer).await).is_empty());

    let (status, wrote) = agent_put(
        &router,
        &citer,
        unchanged(json!([{ "page_id": basis, "version": 1 }])),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{wrote}");
    let wrote: PageWriteResponse = typed(wrote);
    assert_eq!(wrote.basis.len(), 1, "{wrote:?}");
    assert_eq!(wrote.basis[0].state, "ok");
    let read = read_note(&router, &citer).await;
    assert_eq!(version_of(&read), version, "the body was not rewritten");
    assert_eq!(basis_of(&read), wrote.basis);
}
