// SPDX-License-Identifier: Apache-2.0
//! HTTP contract for things (一件事, UX-THING-1): every conversation is in a
//! thing, you can see which, and the person renames a thing or moves a
//! conversation in one step.

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::OnceLock;
use tower::ServiceExt;
use wenlan_types::{
    SessionDetailResponse, ThingActor, ThingDetailResponse, ThingSummary, ThingsResponse,
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
        Caller::Reader => request = request.header("x-agent-name", "thing-route-contract"),
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

async fn things(router: &common::AppRouter, caller: Caller) -> Vec<ThingSummary> {
    let (status, body) = call(router, Method::GET, "/api/things", None, caller).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    typed::<ThingsResponse>(body).things
}

#[tokio::test]
async fn ux_thing_1_a_conversation_is_in_a_thing_you_can_see_and_change_in_one_step() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    db.create_space("other", None, false).await.unwrap();

    let (status, body) = call(
        &router,
        Method::POST,
        "/api/sessions",
        Some(json!({ "session": "codex:tagline", "title": "Tagline copy" })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let session_id = body["id"].as_str().unwrap().to_string();
    db.sort_sessions_into_things().await.unwrap();

    let listed = things(&router, Caller::Reader).await;
    assert_eq!(listed.len(), 1, "{listed:?}");
    let thing = listed[0].clone();
    assert_eq!(thing.title.as_deref(), Some("Tagline copy"));
    assert_eq!(thing.title_by, ThingActor::Ai);
    assert!(thing.id.starts_with("thg_"));

    // You can see which thing it is in.
    let (status, body) = call(
        &router,
        Method::GET,
        &format!("/api/sessions/{session_id}"),
        None,
        Caller::Reader,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let card: SessionDetailResponse = typed(body);
    assert_eq!(card.things.len(), 1);
    assert_eq!(card.things[0].id, thing.id);

    let (status, body) = call(
        &router,
        Method::GET,
        &format!("/api/things/{}", thing.id),
        None,
        Caller::Reader,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let detail: ThingDetailResponse = typed(body);
    assert_eq!(detail.thing, thing);
    assert_eq!(detail.sessions.len(), 1);
    assert_eq!(detail.sessions[0].id, session_id);

    // Another Space does not see it; an unknown thing is absent.
    let (status, _) = call(
        &router,
        Method::GET,
        &format!("/api/things/{}", thing.id),
        None,
        Caller::InSpace("other"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(things(&router, Caller::InSpace("other")).await.is_empty());
    let (status, _) = call(
        &router,
        Method::GET,
        "/api/things/thg_missing",
        None,
        Caller::Reader,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Renaming is the person's.
    let rename = format!("/api/things/{}", thing.id);
    let (status, _) = call(
        &router,
        Method::PATCH,
        &rename,
        Some(json!({ "title": "From an agent" })),
        Caller::Agent("x"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "only the person renames"
    );
    let (status, _) = call(
        &router,
        Method::PATCH,
        &rename,
        Some(json!({ "title": " " })),
        Caller::Person,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a thing needs a name"
    );
    let (status, body) = call(
        &router,
        Method::PATCH,
        &rename,
        Some(json!({ "title": "The new tagline" })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let renamed: ThingSummary = typed(body);
    assert_eq!(renamed.title.as_deref(), Some("The new tagline"));
    assert_eq!(renamed.title_by, ThingActor::Person);

    // Moving it in one step: exactly one target, the person only.
    let move_uri = format!("/api/sessions/{session_id}/things/move");
    let (status, _) = call(
        &router,
        Method::POST,
        &move_uri,
        Some(json!({ "from_thing_id": thing.id })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "no target");
    let (status, _) = call(
        &router,
        Method::POST,
        &move_uri,
        Some(json!({ "from_thing_id": thing.id, "new_title": "Launch week" })),
        Caller::Agent("x"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "only the person moves"
    );
    let (status, _) = call(
        &router,
        Method::POST,
        "/api/sessions/ses_missing/things/move",
        Some(json!({ "from_thing_id": thing.id, "new_title": "Launch week" })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = call(
        &router,
        Method::POST,
        &move_uri,
        Some(json!({ "from_thing_id": thing.id, "new_title": "Launch week" })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let target: ThingSummary = typed(body);
    assert_eq!(target.title.as_deref(), Some("Launch week"));
    assert_eq!(target.title_by, ThingActor::Person);
    let (status, _) = call(
        &router,
        Method::POST,
        &move_uri,
        Some(json!({ "from_thing_id": thing.id, "to_thing_id": target.id })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the emptied thing is gone");
    let listed = things(&router, Caller::Reader).await;
    assert_eq!(
        listed.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        [target.id.as_str()]
    );
    db.sort_sessions_into_things().await.unwrap();
    assert_eq!(
        things(&router, Caller::Reader).await.len(),
        1,
        "a move is not re-sorted"
    );
}
