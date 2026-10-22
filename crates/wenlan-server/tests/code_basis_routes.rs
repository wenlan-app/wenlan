// SPDX-License-Identifier: Apache-2.0
//! HTTP contract for a note that relies on lines of code (its code basis).
//!
//! Numbers in the test names refer to the acceptance list of the code-basis
//! spec (`2026-10-20-code-basis-spec.md`, 验收).

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;
use wenlan_core::db::MemoryDB;
use wenlan_core::truth_contract::{CONTRACT_HEADER, INTENT_HEADER};
use wenlan_types::responses::{CreatePageResponse, PageWriteResponse};
use wenlan_types::{BriefReadResponse, CodeBasisRecheckEntry, CodeBasisRef};

const SPACE: &str = "work";

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
                .header("x-agent-name", "code-basis-route-contract");
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

/// `n` numbered lines: `line 1`, `line 2`, ...
fn numbered(n: u32) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn new(path: &str, text: &str) -> Self {
        let repo = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        repo.write(path, text);
        repo
    }
    fn root(&self) -> String {
        self.dir.path().to_str().unwrap().to_string()
    }
    fn write(&self, path: &str, text: &str) {
        let full = self.dir.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    fn span(&self, path: &str, start: u32, end: u32) -> Value {
        json!({
            "repo_root": self.root(),
            "path": path,
            "line_start": start,
            "line_end": end,
        })
    }
}

async fn new_note(
    router: &common::AppRouter,
    title: &str,
    code_basis: Value,
) -> (StatusCode, Value) {
    call(
        router,
        Method::POST,
        "/api/pages",
        Some(json!({
            "title": title,
            "content": format!("{title}: what the code does."),
            "space": SPACE,
            "creation_kind": "authored",
            "code_basis": code_basis,
        })),
        Caller::Agent("claude-code"),
    )
    .await
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

fn code_basis_of(read: &Value) -> Vec<CodeBasisRef> {
    read.get("code_basis")
        .map(|rows| typed(rows.clone()))
        .unwrap_or_default()
}

// Recheck and brief serve automatic callers only: the truth guard refuses the
// explicit reader marker there (403 in production), so these helpers call them
// the way the MCP server does, as a named agent without the marker.
async fn recheck(router: &common::AppRouter) -> Vec<CodeBasisRecheckEntry> {
    let (status, body) = call(
        router,
        Method::POST,
        &format!("/api/pages/code-basis/recheck?space={SPACE}"),
        None,
        Caller::Agent("code-basis-route-contract"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    typed(body)
}

async fn brief_pending(router: &common::AppRouter) -> Option<u32> {
    let (status, body) = call(
        router,
        Method::POST,
        "/api/brief",
        Some(json!({ "space": SPACE })),
        Caller::Agent("code-basis-route-contract"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let response: BriefReadResponse = typed(body);
    response.code_basis_pending
}

async fn make_brief(db: &Arc<MemoryDB>) {
    db.apply_brief_update(
        &serde_json::from_value(json!({
            "space": SPACE,
            "caller_id": "code-basis-route-test",
            "operation_id": "create-work",
            "mutations": [{"kind": "add", "text": "ship it", "state": "active"}],
        }))
        .unwrap(),
    )
    .await
    .unwrap();
}

// ---------------------------------------------------------------------------
// Acceptance 1-5 and 7 over HTTP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptance_1_to_5_write_move_change_confirm_and_lose_the_file_over_http() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    make_brief(&db).await;
    let repo = Repo::new("src/lib.rs", &numbered(40));

    // 1: the write records the lines; the response and read_page say `ok`.
    let (status, created) =
        new_note(&router, "Library", json!([repo.span("src/lib.rs", 10, 20)])).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let created: CreatePageResponse = typed(created);
    assert_eq!(created.code_basis.len(), 1);
    assert_eq!(created.code_basis[0].state, "ok");
    assert_eq!(
        (
            created.code_basis[0].line_start,
            created.code_basis[0].line_end
        ),
        (10, 20)
    );
    let id = created.id;
    assert_eq!(
        code_basis_of(&read_note(&router, &id).await),
        created.code_basis
    );
    assert_eq!(brief_pending(&router).await, Some(0));

    // 2: a comment above moves the span; nobody is told, the agent can see it.
    repo.write("src/lib.rs", &format!("// new comment\n{}", numbered(40)));
    assert!(recheck(&router).await.is_empty());
    assert_eq!(brief_pending(&router).await, Some(0));
    let rows = code_basis_of(&read_note(&router, &id).await);
    assert_eq!(rows[0].state, "moved");
    assert_eq!((rows[0].line_start, rows[0].line_end), (11, 21));
    let (_, listed) = call(
        &router,
        Method::GET,
        "/api/pages?limit=50",
        None,
        Caller::Reader,
    )
    .await;
    let listed_rows: Vec<CodeBasisRef> = typed(listed["pages"][0]["code_basis"].clone());
    assert_eq!(listed_rows[0].state, "moved", "list carries non-ok rows");

    // 3: an edit inside the lines makes the note pending, and the brief counts it.
    repo.write(
        "src/lib.rs",
        &format!(
            "// new comment\n{}",
            numbered(40).replace("line 15\n", "line fifteen, rewritten\n")
        ),
    );
    // The list rechecks the cited files itself: read before any Brief or
    // recheck call, it already shows the note as pending.
    let (_, listed) = call(
        &router,
        Method::GET,
        &format!("/api/pages?limit=50&space={SPACE}"),
        None,
        Caller::Reader,
    )
    .await;
    let listed_rows: Vec<CodeBasisRef> = typed(listed["pages"][0]["code_basis"].clone());
    assert_eq!(listed_rows[0].state, "pending", "list rechecks first");
    assert_eq!(brief_pending(&router).await, Some(1));
    let report = recheck(&router).await;
    assert_eq!(report.len(), 1);
    assert_eq!(report[0].page_id, id);
    assert_eq!(report[0].title, "Library");
    assert_eq!(report[0].pending[0].state, "pending");
    assert!(report[0].pending[0].changed_at.is_some());
    let listed_rows = code_basis_of(&json!({
        "code_basis": call(&router, Method::GET, "/api/pages?limit=50", None, Caller::Reader)
            .await
            .1["pages"][0]["code_basis"]
    }));
    assert_eq!(listed_rows[0].state, "pending");

    // 4: confirm re-anchors on what the file says now.
    let locator = json!({
        "repo_root": repo.root(), "path": "src/lib.rs", "line_start": 11, "line_end": 21
    });
    let (status, confirmed) = call(
        &router,
        Method::POST,
        &format!("/api/pages/{id}/code-basis/confirm"),
        Some(locator.clone()),
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    let confirmed: CodeBasisRef = typed(confirmed);
    assert_eq!(confirmed.state, "ok");
    assert_eq!((confirmed.line_start, confirmed.line_end), (11, 21));
    assert_eq!(brief_pending(&router).await, Some(0));
    assert!(recheck(&router).await.is_empty());

    // An unknown row is 404.
    let (status, missing) = call(
        &router,
        Method::POST,
        &format!("/api/pages/{id}/code-basis/confirm"),
        Some(
            json!({"repo_root": repo.root(), "path": "src/lib.rs", "line_start": 1, "line_end": 2}),
        ),
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");

    // 5: the file goes away; the row is unavailable and the brief counts it.
    std::fs::remove_file(repo.dir.path().join("src/lib.rs")).unwrap();
    assert_eq!(brief_pending(&router).await, Some(1));
    let report = recheck(&router).await;
    assert_eq!(report[0].pending[0].state, "unavailable");
    assert_eq!(report[0].pending[0].reason.as_deref(), Some("file_missing"));
    // Confirming against a file that cannot be read is a 409 and changes nothing.
    let (status, refused) = call(
        &router,
        Method::POST,
        &format!("/api/pages/{id}/code-basis/confirm"),
        Some(locator.clone()),
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(
        code_basis_of(&read_note(&router, &id).await)[0].state,
        "unavailable"
    );

    // Remove: once true, then false; the note no longer lists it.
    for expected in [true, false] {
        let (status, removed) = call(
            &router,
            Method::DELETE,
            &format!("/api/pages/{id}/code-basis"),
            Some(locator.clone()),
            Caller::Agent("claude-code"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{removed}");
        assert_eq!(removed, json!({ "removed": expected }));
    }
    let read = read_note(&router, &id).await;
    assert!(read.get("code_basis").is_none(), "{read}");
}

#[tokio::test]
async fn the_cited_route_serves_the_blob_id_the_row_was_written_or_confirmed_against() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    // Registers the Space; the create route refuses an unknown one (422).
    make_brief(&db).await;
    let original = numbered(40);
    let repo = Repo::new("src/lib.rs", &original);
    let (status, created) =
        new_note(&router, "Library", json!([repo.span("src/lib.rs", 10, 20)])).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = typed::<CreatePageResponse>(created).id;
    let locator = json!({
        "repo_root": repo.root(), "path": "src/lib.rs", "line_start": 10, "line_end": 20
    });
    let uri = format!("/api/pages/{id}/code-basis/cited");

    let (status, cited) = call(
        &router,
        Method::POST,
        &uri,
        Some(locator.clone()),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cited}");
    assert_eq!(
        cited["cited_blob"],
        json!(wenlan_core::code_basis::git_blob_id(original.as_bytes()))
    );
    assert_eq!(cited["state"], "ok");
    assert!(cited["confirmed_at"].as_i64().unwrap() > 0);

    // The blob id never rides on the note's own read.
    let read = read_note(&router, &id).await;
    assert!(!read.to_string().contains("cited_blob"), "{read}");

    // An edit makes the row pending but it keeps pointing at the old file;
    // a confirm moves it to the file as it reads now.
    let edited = original.replace("line 15\n", "line fifteen, rewritten\n");
    repo.write("src/lib.rs", &edited);
    recheck(&router).await;
    let (_, pending) = call(
        &router,
        Method::POST,
        &uri,
        Some(locator.clone()),
        Caller::Person,
    )
    .await;
    assert_eq!(pending["state"], "pending");
    assert_eq!(pending["cited_blob"], cited["cited_blob"]);
    let (status, _) = call(
        &router,
        Method::POST,
        &format!("/api/pages/{id}/code-basis/confirm"),
        Some(locator.clone()),
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, after) = call(&router, Method::POST, &uri, Some(locator), Caller::Person).await;
    assert_eq!(
        after["cited_blob"],
        json!(wenlan_core::code_basis::git_blob_id(edited.as_bytes()))
    );

    // An unknown row or note is 404.
    let unknown = json!({
        "repo_root": repo.root(), "path": "src/lib.rs", "line_start": 1, "line_end": 2
    });
    let (status, _) = call(
        &router,
        Method::POST,
        &uri,
        Some(unknown.clone()),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &router,
        Method::POST,
        "/api/pages/nope/code-basis/cited",
        Some(unknown),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The sentence and verdict the everyday model leaves reach the note's own read
/// while the row is pending, the Brief says every pending note was judged, and
/// a confirm takes both away again.
#[tokio::test]
async fn the_models_explanation_reaches_the_note_read_and_a_confirm_clears_it() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    make_brief(&db).await;
    let original = numbered(40);
    let repo = Repo::new("src/lib.rs", &original);
    let (status, created) =
        new_note(&router, "Library", json!([repo.span("src/lib.rs", 10, 20)])).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = typed::<CreatePageResponse>(created).id;

    // Quiet while the lines are unchanged.
    let read = read_note(&router, &id).await;
    assert!(!read.to_string().contains("explanation"), "{read}");
    assert!(!read.to_string().contains("ai_verdict"), "{read}");

    repo.write(
        "src/lib.rs",
        &original.replace("line 15\n", "line fifteen, rewritten\n"),
    );
    recheck(&router).await;
    let rows = db.list_code_basis_recheck_candidates(10).await.unwrap();
    assert_eq!(rows.len(), 1);
    let sentence = "The note says the span keeps 32 bits on purpose; the code now keeps 48.";
    // A verdict is read back only while it is about the note's current version.
    let version = db.get_page(&id).await.unwrap().unwrap().version;
    let key = wenlan_core::code_basis_recheck::recheck_key("digest-1", version);
    assert!(db
        .record_code_basis_recheck(&rows[0], &key, Some(sentence), Some("stale"), 1)
        .await
        .unwrap());

    let read = read_note(&router, &id).await;
    let basis = code_basis_of(&read);
    assert_eq!(basis.len(), 1);
    assert_eq!(basis[0].state, "pending");
    assert_eq!(basis[0].explanation.as_deref(), Some(sentence));
    assert_eq!(basis[0].ai_verdict.as_deref(), Some("stale"));
    assert!(
        read.to_string().contains("\"ai_verdict\":\"stale\""),
        "{read}"
    );

    let (status, brief) = call(
        &router,
        Method::POST,
        "/api/brief",
        Some(json!({ "space": SPACE })),
        Caller::Agent("code-basis-route-contract"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{brief}");
    let brief: BriefReadResponse = typed(brief);
    assert_eq!(brief.code_basis_pending, Some(1));
    assert!(brief.code_basis_pending_explained);

    let locator = json!({
        "repo_root": repo.root(), "path": "src/lib.rs", "line_start": 10, "line_end": 20
    });
    let (status, _) = call(
        &router,
        Method::POST,
        &format!("/api/pages/{id}/code-basis/confirm"),
        Some(locator),
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let read = read_note(&router, &id).await;
    assert_eq!(code_basis_of(&read)[0].state, "ok");
    assert!(!read.to_string().contains("explanation"), "{read}");
    assert!(!read.to_string().contains("ai_verdict"), "{read}");
}

#[tokio::test]
async fn acceptance_7_line_endings_and_trailing_spaces_do_not_notify() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    make_brief(&db).await;
    let repo = Repo::new("a.rs", &numbered(40));
    let (status, created) = new_note(&router, "Crlf", json!([repo.span("a.rs", 10, 20)])).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = typed::<CreatePageResponse>(created).id;

    let crlf: String = numbered(40)
        .lines()
        .map(|line| format!("{line}  \r\n"))
        .collect();
    repo.write("a.rs", &crlf);
    assert_eq!(brief_pending(&router).await, Some(0));
    let rows = code_basis_of(&read_note(&router, &id).await);
    assert_eq!(rows[0].state, "ok");
    assert_eq!((rows[0].line_start, rows[0].line_end), (10, 20));
}

#[tokio::test]
async fn acceptance_8_twenty_notes_on_one_span_are_all_pending_after_one_recheck() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    make_brief(&db).await;
    let repo = Repo::new("a.rs", &numbered(40));
    for i in 0..20 {
        let (status, created) = new_note(
            &router,
            &format!("Note {i}"),
            json!([repo.span("a.rs", 10, 20)]),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created}");
    }
    repo.write("a.rs", &numbered(40).replace("line 12\n", "changed\n"));

    let report = recheck(&router).await;
    assert_eq!(report.len(), 20);
    assert!(report
        .iter()
        .all(|entry| entry.pending.len() == 1 && entry.pending[0].state == "pending"));
    assert_eq!(brief_pending(&router).await, Some(20));
}

// ---------------------------------------------------------------------------
// Refusals (acceptance 6) and old bodies
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptance_6_a_bad_path_span_or_file_is_400_and_nothing_is_written() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    // Registers the Space; the create route refuses an unknown one (422).
    make_brief(&db).await;
    let repo = Repo::new("src/lib.rs", &numbered(30));
    let page = common::create_page_fixture(
        &db,
        "Existing",
        "Original text.",
        Some(SPACE),
        &[],
        "authored",
    )
    .await;

    let bad = [
        ("a path with ..", repo.span("../outside.rs", 1, 2), "path"),
        ("an absolute path", repo.span("/etc/passwd", 1, 2), "path"),
        ("backslashes", repo.span("src\\lib.rs", 1, 2), "path"),
        (
            "end before start",
            repo.span("src/lib.rs", 20, 10),
            "selects no lines",
        ),
        (
            "line zero",
            repo.span("src/lib.rs", 0, 3),
            "selects no lines",
        ),
        (
            "past the end",
            repo.span("src/lib.rs", 25, 40),
            "outside the file",
        ),
        (
            "a missing file",
            repo.span("src/gone.rs", 1, 2),
            "file_missing",
        ),
        (
            "a relative root",
            json!({"repo_root": "relative", "path": "a.rs", "line_start": 1, "line_end": 2}),
            "absolute",
        ),
    ];
    let (_, before) = call(
        &router,
        Method::GET,
        "/api/pages?limit=50",
        None,
        Caller::Reader,
    )
    .await;
    for (what, entry, needle) in bad {
        // Create: refused, no note is left behind.
        let (status, body) = new_note(&router, "Refused", json!([entry.clone()])).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "create with {what}: {body}"
        );
        assert!(
            body["error"].as_str().unwrap().contains(needle),
            "create with {what}: {body}"
        );

        // Refresh: refused, the text and version are untouched.
        let (status, body) = call(
            &router,
            Method::PUT,
            &format!("/api/pages/{page}"),
            Some(json!({
                "content": "Replacement text.",
                "source_memory_ids": [],
                "expected_version": 1,
                "code_basis": [entry],
            })),
            Caller::Agent("claude-code"),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "refresh with {what}: {body}"
        );
        assert!(
            body["error"].as_str().unwrap().contains(needle),
            "refresh with {what}: {body}"
        );
    }
    let (_, after) = call(
        &router,
        Method::GET,
        "/api/pages?limit=50",
        None,
        Caller::Reader,
    )
    .await;
    assert_eq!(
        after["pages"].as_array().unwrap().len(),
        before["pages"].as_array().unwrap().len(),
        "no refused create left a note behind"
    );
    let read = read_note(&router, &page).await;
    assert_eq!(read["page"]["content"], "Original text.");
    assert_eq!(read["page"]["version"], 1);
    assert!(read.get("code_basis").is_none());
}

#[tokio::test]
async fn old_bodies_without_code_basis_still_work_and_responses_stay_quiet() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    // Registers the Space; the create route refuses an unknown one (422).
    make_brief(&db).await;

    let (status, created) = call(
        &router,
        Method::POST,
        "/api/pages",
        Some(json!({
            "title": "Plain",
            "content": "No code here.",
            "space": SPACE,
            "creation_kind": "authored",
        })),
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert!(created.get("code_basis").is_none(), "{created}");
    let id = typed::<CreatePageResponse>(created).id;

    let (status, refreshed) = call(
        &router,
        Method::PUT,
        &format!("/api/pages/{id}"),
        Some(json!({
            "content": "Still no code.",
            "source_memory_ids": [],
            "expected_version": 1,
        })),
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{refreshed}");
    assert!(refreshed.get("code_basis").is_none(), "{refreshed}");
    let _: PageWriteResponse = typed(refreshed);
    let read = read_note(&router, &id).await;
    assert!(read.get("code_basis").is_none());
    assert!(db.list_page_code_basis(&id).await.unwrap().is_empty());

    // A Space without any code basis reads nothing: the field is absent.
    let (_, body) = call(
        &router,
        Method::POST,
        "/api/brief",
        Some(json!({ "space": SPACE })),
        Caller::Agent("code-basis-route-contract"),
    )
    .await;
    assert!(body.get("code_basis_pending").is_none(), "{body}");
}

// ---------------------------------------------------------------------------
// Refresh
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_refresh_records_code_basis_with_the_write_even_when_the_text_is_unchanged() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let repo = Repo::new("a.rs", &numbered(40));
    let page = common::create_page_fixture(
        &db,
        "Existing",
        "Original text.",
        Some(SPACE),
        &[],
        "authored",
    )
    .await;

    // A refresh that changes the text: the rows ride the write.
    let (status, written) = call(
        &router,
        Method::PUT,
        &format!("/api/pages/{page}"),
        Some(json!({
            "content": "Replacement text.",
            "source_memory_ids": [],
            "expected_version": 1,
            "code_basis": [repo.span("a.rs", 1, 5)],
        })),
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{written}");
    let written: PageWriteResponse = typed(written);
    assert_eq!(written.code_basis.len(), 1);
    assert_eq!(written.code_basis[0].state, "ok");

    // The same text again with one more span: nothing to write, the span is
    // still recorded, and the first one is kept.
    let (status, again) = call(
        &router,
        Method::PUT,
        &format!("/api/pages/{page}"),
        Some(json!({
            "content": "Replacement text.",
            "source_memory_ids": [],
            "expected_version": 2,
            "code_basis": [repo.span("a.rs", 10, 20)],
        })),
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    let again: PageWriteResponse = typed(again);
    assert_eq!(again.code_basis.len(), 2, "{again:?}");
    assert_eq!(code_basis_of(&read_note(&router, &page).await).len(), 2);
}

// ---------------------------------------------------------------------------
// Acceptance 9: deleting a page removes its rows
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptance_9_deleting_a_note_removes_its_code_bases() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    // Registers the Space; the create route refuses an unknown one (422).
    make_brief(&db).await;
    let repo = Repo::new("a.rs", &numbered(40));
    let (_, created) = new_note(&router, "Doomed", json!([repo.span("a.rs", 10, 20)])).await;
    let id = typed::<CreatePageResponse>(created).id;
    assert_eq!(db.list_page_code_basis(&id).await.unwrap().len(), 1);

    let (status, body) = call(
        &router,
        Method::DELETE,
        &format!("/api/pages/{id}"),
        None,
        Caller::Person,
    )
    .await;
    assert!(status.is_success(), "{status}: {body}");
    assert!(db.list_page_code_basis(&id).await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Review fixes: gated refresh, and refusals that never touch the disk
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_refresh_staged_as_a_card_says_the_code_basis_was_not_recorded() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let repo = Repo::new("a.rs", &numbered(40));
    let page = common::create_page_fixture(
        &db,
        "Locked",
        "Original text.",
        Some(SPACE),
        &[],
        "authored",
    )
    .await;
    assert!(db.set_page_review_required(&page, true).await.unwrap());

    let (status, body) = call(
        &router,
        Method::PUT,
        &format!("/api/pages/{page}"),
        Some(json!({
            "content": "Replacement text.",
            "source_memory_ids": [],
            "expected_version": 1,
            "code_basis": [repo.span("a.rs", 1, 5)],
        })),
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let written: PageWriteResponse = typed(body);
    assert!(written.gated);
    assert!(written.code_basis.is_empty());
    assert!(
        written
            .warnings
            .iter()
            .any(|w| w.contains("code_basis was not recorded") && w.contains("accepted first")),
        "{:?}",
        written.warnings
    );
    assert!(db.list_page_code_basis(&page).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_stale_version_is_refused_with_409_before_any_file_is_read() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    let page =
        common::create_page_fixture(&db, "Moved", "Text.", Some(SPACE), &[], "authored").await;

    // The file does not exist: were it read first this would be a 400.
    let (status, body) = call(
        &router,
        Method::PUT,
        &format!("/api/pages/{page}"),
        Some(json!({
            "content": "Replacement text.",
            "source_memory_ids": [],
            "expected_version": 99,
            "code_basis": [{
                "repo_root": "/nonexistent-repo-root",
                "path": "gone.rs",
                "line_start": 1,
                "line_end": 2,
            }],
        })),
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // A bad entry on a write that is allowed to proceed is still a 400.
    let (status, body) = call(
        &router,
        Method::PUT,
        &format!("/api/pages/{page}"),
        Some(json!({
            "content": "Replacement text.",
            "source_memory_ids": [],
            "expected_version": 1,
            "code_basis": [{
                "repo_root": "/nonexistent-repo-root",
                "path": "gone.rs",
                "line_start": 1,
                "line_end": 2,
            }],
        })),
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}
