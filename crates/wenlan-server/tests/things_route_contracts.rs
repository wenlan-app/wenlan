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
    SavedInResponse, SessionDetailResponse, ThingActor, ThingDetailResponse, ThingMapAuthor,
    ThingMapOpsResponse, ThingMapResponse, ThingMapState, ThingNightPointsResponse, ThingSummary,
    ThingsResponse,
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
    /// The app: its person marker, no agent name.
    Person,
    /// A request with neither the person marker nor an agent name: an AI
    /// tool that did not say who it is, never the person (UX-WHO-2).
    Unmarked,
    /// An automatic agent read: these routes serve automatic callers only.
    Reader,
    /// An agent writing, named by `x-agent-name`.
    Agent(&'static str),
    /// The app, scoped to one Space, with its person marker.
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
        Caller::Person => {
            request = request.header(
                wenlan_types::requests::CALLER_HEADER,
                wenlan_types::requests::PERSON_CALLER_VALUE,
            )
        }
        Caller::Unmarked => {}
        Caller::Reader => request = request.header("x-agent-name", "thing-route-contract"),
        Caller::Agent(name) => request = request.header("x-agent-name", name),
        Caller::InSpace(space) => {
            request = request.header("x-wenlan-space", space).header(
                wenlan_types::requests::CALLER_HEADER,
                wenlan_types::requests::PERSON_CALLER_VALUE,
            )
        }
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

    // The big map's night points answer typed, not as a thing named
    // "points"; with no night yet there is no thing to draw.
    let (status, body) = call(
        &router,
        Method::GET,
        "/api/things/points",
        None,
        Caller::Reader,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(typed::<ThingNightPointsResponse>(body).things.is_empty());

    // Its map is empty until something writes it.
    let (status, body) = call(
        &router,
        Method::GET,
        &format!("/api/things/{}/map", thing.id),
        None,
        Caller::Reader,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let map: ThingMapResponse = typed(body);
    assert_eq!(map.thing_id, thing.id);
    assert_eq!(map.revision, 0);
    assert!(map.nodes.is_empty() && map.edges.is_empty());

    // The AI writes points; its change to the person's point waits for them.
    let ops_uri = format!("/api/things/{}/map/ops", thing.id);
    let (status, body) = call(
        &router,
        Method::POST,
        &ops_uri,
        Some(json!({ "expected_revision": 0, "ops": [
            { "op": "add", "id": "root", "parent_id": null, "kind": "question",
              "label": "Which tagline?" },
            { "op": "add", "id": "b", "parent_id": "root", "kind": "option",
              "state": "open", "label": "Option B",
              "sources": [{ "session_id": session_id, "from": 2, "to": 3 }] },
        ]})),
        Caller::Agent("codex"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let written: ThingMapOpsResponse = typed(body);
    assert_eq!((written.map.revision, written.proposed), (1, 0));
    assert_eq!(written.map.nodes[1].by, ThingMapAuthor::Ai);
    let (status, body) = call(
        &router,
        Method::POST,
        &ops_uri,
        Some(json!({ "expected_revision": 1, "ops": [
            { "op": "edit", "id": "b", "label": "Option B, shorter" },
        ]})),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &router,
        Method::POST,
        &ops_uri,
        Some(json!({ "expected_revision": 2, "ops": [
            { "op": "edit", "id": "b", "state": "dropped" },
        ]})),
        Caller::Agent("codex"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let proposed: ThingMapOpsResponse = typed(body);
    assert_eq!(proposed.proposed, 1);
    assert_eq!(proposed.map.nodes[1].state, Some(ThingMapState::Open));
    assert_eq!(proposed.map.proposals.len(), 1);
    // A stale revision is a conflict; a bad op changes nothing.
    let (status, _) = call(
        &router,
        Method::POST,
        &ops_uri,
        Some(json!({ "expected_revision": 1, "ops": [{ "op": "remove", "id": "b" }]})),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(
        &router,
        Method::POST,
        &ops_uri,
        Some(json!({ "expected_revision": 3, "ops": [{ "op": "remove", "id": "nope" }]})),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = call(
        &router,
        Method::GET,
        &format!("/api/things/{}/map", thing.id),
        None,
        Caller::Reader,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(typed::<ThingMapResponse>(body), proposed.map);

    // UX-WHO-2: only the app's marked person request answers the proposal.
    // An agent and a request with no marker at all are refused and change
    // nothing; the marked person's Accept lands the change.
    let answer = json!({ "expected_revision": 3, "ops": [
        { "op": "accept", "proposal_id": proposed.map.proposals[0].id },
    ]});
    for refused in [Caller::Agent("codex"), Caller::Unmarked] {
        let (status, body) = call(
            &router,
            Method::POST,
            &ops_uri,
            Some(answer.clone()),
            refused,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(
            body.to_string()
                .contains("only the person answers a proposal"),
            "{body}"
        );
    }
    let (status, body) = call(
        &router,
        Method::GET,
        &format!("/api/things/{}/map", thing.id),
        None,
        Caller::Reader,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(typed::<ThingMapResponse>(body), proposed.map);
    let (status, body) = call(
        &router,
        Method::POST,
        &ops_uri,
        Some(answer),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let answered: ThingMapOpsResponse = typed(body);
    assert_eq!(answered.map.revision, 4);
    assert!(answered.map.proposals.is_empty());
    assert_eq!(answered.map.nodes[1].state, Some(ThingMapState::Dropped));

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
        &format!("/api/things/{}/map", thing.id),
        None,
        Caller::InSpace("other"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &router,
        Method::POST,
        &format!("/api/things/{}/map/ops", thing.id),
        Some(json!({ "expected_revision": 3, "ops": [{ "op": "remove", "id": "root" }]})),
        Caller::InSpace("other"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
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
        Some(json!({ "title": "From an unmarked caller" })),
        Caller::Unmarked,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "no agent name is not the person"
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

    // "Don't show" is the person's too; the thing stays listed, quiet, for
    // search (UX-HOME-5).
    let hide = format!("/api/things/{}/hide", thing.id);
    let (status, _) = call(&router, Method::POST, &hide, None, Caller::Agent("x")).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "only the person hides"
    );
    let (status, body) = call(&router, Method::POST, &hide, None, Caller::Person).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(typed::<ThingSummary>(body).quiet);
    assert!(things(&router, Caller::Person)
        .await
        .iter()
        .any(|t| t.id == thing.id && t.quiet));
    let (status, _) = call(
        &router,
        Method::POST,
        "/api/things/thg_missing/hide",
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

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

#[tokio::test]
async fn ux_chat_7_a_conversation_hears_once_where_it_was_saved() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    db.create_space("other", None, false).await.unwrap();

    let (status, body) = call(
        &router,
        Method::POST,
        "/api/sessions",
        Some(json!({ "session": "claude-code:lisbon", "title": "Plan the Lisbon trip" })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let session_id = body["id"].as_str().unwrap().to_string();
    let saved_in = format!("/api/sessions/{session_id}/saved-in");

    // A Space that cannot see the thing is never told its name.
    let (status, body) = call(
        &router,
        Method::POST,
        &saved_in,
        None,
        Caller::InSpace("other"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(typed::<SavedInResponse>(body).saved_in, None);

    // The first save sorts the conversation and names its thing, with a link
    // into the app. An AI tool asks by its conversation ref.
    let (status, body) = call(
        &router,
        Method::POST,
        "/api/sessions/claude-code:lisbon/saved-in",
        None,
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let first = typed::<SavedInResponse>(body)
        .saved_in
        .expect("named on the first save");
    assert_eq!(first.title, "Plan the Lisbon trip");
    assert_eq!(first.link, format!("wenlan://thing/{}", first.thing_id));

    // Later saves in the same thing say nothing.
    let (_, body) = call(
        &router,
        Method::POST,
        &saved_in,
        None,
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(typed::<SavedInResponse>(body).saved_in, None);

    // Moved to another thing, the next save names the new one once.
    let (status, body) = call(
        &router,
        Method::POST,
        &format!("/api/sessions/{session_id}/things/move"),
        Some(json!({ "from_thing_id": first.thing_id, "new_title": "Sintra day trip" })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = call(
        &router,
        Method::POST,
        &saved_in,
        None,
        Caller::Agent("claude-code"),
    )
    .await;
    let moved = typed::<SavedInResponse>(body)
        .saved_in
        .expect("named after the move");
    assert_eq!(moved.title, "Sintra day trip");
    assert_ne!(moved.thing_id, first.thing_id);
    let (_, body) = call(
        &router,
        Method::POST,
        &saved_in,
        None,
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(typed::<SavedInResponse>(body).saved_in, None);

    // The tool can write the conversation's title after the first save: the
    // thing waits unnamed and says nothing, then is named on the next save.
    let (status, body) = call(
        &router,
        Method::POST,
        "/api/sessions",
        Some(json!({ "session": "claude-code:porto" })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let porto = "/api/sessions/claude-code:porto/saved-in";
    let (status, body) = call(
        &router,
        Method::POST,
        porto,
        None,
        Caller::Agent("claude-code"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(typed::<SavedInResponse>(body).saved_in, None);
    let (status, body) = call(
        &router,
        Method::POST,
        "/api/sessions",
        Some(json!({ "session": "claude-code:porto", "title": "Porto stay in Ribeira" })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = call(
        &router,
        Method::POST,
        porto,
        None,
        Caller::Agent("claude-code"),
    )
    .await;
    let named = typed::<SavedInResponse>(body)
        .saved_in
        .expect("named once the conversation has a title");
    assert_eq!(named.title, "Porto stay in Ribeira");

    // Page changes: a new page is always worth a mention; an update or a
    // suggestion once per page in a conversation; a page the AI has just
    // written is not news again when it revises it.
    let page = |page_id: &str, change: &str| json!({ "page_id": page_id, "change": change });
    for (body, expected) in [
        (page("page_a", "updated"), true),
        (page("page_a", "updated"), false),
        (page("page_a", "proposal"), true),
        (page("page_a", "proposal"), false),
        (page("page_b", "new"), true),
        (page("page_b", "updated"), false),
        (page("page_b", "proposal"), true),
    ] {
        let (status, response) = call(
            &router,
            Method::POST,
            porto,
            Some(body.clone()),
            Caller::Agent("claude-code"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{response}");
        let response = typed::<SavedInResponse>(response);
        assert_eq!(response.report_page, expected, "{body}");
        assert_eq!(response.saved_in, None, "{body}");
    }
    // Another conversation hears about the same page again.
    let (_, response) = call(
        &router,
        Method::POST,
        &saved_in,
        Some(page("page_a", "updated")),
        Caller::Agent("claude-code"),
    )
    .await;
    assert!(typed::<SavedInResponse>(response).report_page);
}

#[tokio::test]
async fn ux_find_1_search_finds_what_was_said_with_the_lines_around_it() {
    let _guard = data_dir_lock().lock().await;
    let vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    db.create_space("other", None, false).await.unwrap();

    let projects = vault._tmp.path().join(".claude/projects/-kyoto");
    std::fs::create_dir_all(&projects).unwrap();
    let transcript = projects.join("kyoto.jsonl");
    std::fs::write(
        &transcript,
        [
            r#"{"type":"user","message":{"role":"user","content":"Where should we stay in Kyoto?"},"timestamp":"2026-11-01T10:00:00Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Quiet, or easy to walk out at night?"}]},"timestamp":"2026-11-01T10:00:05Z"}"#,
            r#"{"type":"user","message":{"role":"user","content":"决定住在祇园附近，晚上散步方便。"},"timestamp":"2026-11-01T10:01:00Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Noted."}]},"timestamp":"2026-11-01T10:01:04Z"}"#,
        ]
        .join("\n"),
    )
    .unwrap();
    let (status, body) = call(
        &router,
        Method::POST,
        "/api/sessions",
        Some(json!({
            "session": "claude-code:kyoto",
            "title": "Kyoto stay",
            "transcript_path": transcript.to_str().unwrap(),
        })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let session_id = body["id"].as_str().unwrap().to_string();
    db.sort_sessions_into_things().await.unwrap();

    let (status, body) = call(
        &router,
        Method::GET,
        "/api/conversations/search?q=%E7%A5%87%E5%9B%AD",
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let hits = typed::<wenlan_types::conversations::ConversationSearchResponse>(body).hits;
    assert_eq!(hits.len(), 1, "{hits:?}");
    let hit = &hits[0];
    assert_eq!(hit.session_id, session_id);
    assert_eq!(hit.title.as_deref(), Some("Kyoto stay"));
    assert_eq!(hit.thing_title.as_deref(), Some("Kyoto stay"));
    assert_eq!(hit.index, 2);
    assert_eq!(hit.line.text, "决定住在祇园附近，晚上散步方便。");
    assert_eq!(
        hit.before.as_ref().map(|l| l.text.as_str()),
        Some("Quiet, or easy to walk out at night?")
    );
    assert_eq!(hit.after.as_ref().map(|l| l.text.as_str()), Some("Noted."));

    // Show all: the whole conversation, oldest first.
    let (status, body) = call(
        &router,
        Method::GET,
        &format!("/api/sessions/{session_id}/lines"),
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let all = typed::<wenlan_types::conversations::ConversationLinesResponse>(body);
    assert_eq!(all.lines.len(), 4);
    assert_eq!(all.first_index, 0);
    assert_eq!(all.thing_title.as_deref(), Some("Kyoto stay"));

    // An empty search finds nothing; an AI tool may not read the words; a
    // Space that cannot see the conversation finds nothing in it.
    let (status, body) = call(
        &router,
        Method::GET,
        "/api/conversations/search?q=%20",
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        typed::<wenlan_types::conversations::ConversationSearchResponse>(body)
            .hits
            .is_empty()
    );
    for uri in [
        "/api/conversations/search?q=Kyoto".to_string(),
        format!("/api/sessions/{session_id}/lines"),
    ] {
        let (status, _) = call(&router, Method::GET, &uri, None, Caller::Agent("codex")).await;
        assert_ne!(status, StatusCode::OK, "{uri} is for the person");
    }
    let (status, body) = call(
        &router,
        Method::GET,
        "/api/conversations/search?q=Kyoto",
        None,
        Caller::InSpace("other"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        typed::<wenlan_types::conversations::ConversationSearchResponse>(body)
            .hits
            .is_empty()
    );
    let (status, _) = call(
        &router,
        Method::GET,
        &format!("/api/sessions/{session_id}/lines"),
        None,
        Caller::InSpace("other"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Once the conversation writes in that Space, the Space finds its words.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/pages")
                .header("content-type", "application/json")
                .header("x-agent-name", "claude-code")
                .header("x-wenlan-session", "claude-code:kyoto")
                .header("x-wenlan-space", "other")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "title": "Gion inn",
                        "content": "Ryokan two streets from Hanamikoji.",
                        "creation_kind": "authored",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (status, body) = call(
        &router,
        Method::GET,
        "/api/conversations/search?q=Kyoto",
        None,
        Caller::InSpace("other"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let hits = typed::<wenlan_types::conversations::ConversationSearchResponse>(body).hits;
    assert_eq!(hits.len(), 1, "{hits:?}");
    let (status, body) = call(
        &router,
        Method::GET,
        &format!("/api/sessions/{session_id}/lines"),
        None,
        Caller::InSpace("other"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // A word inside Chinese text finds the page that contains it, and only
    // the pages that contain it.
    for (title, content) in [
        ("京都住宿", "決定住在祇園附近，晚上散步方便。"),
        ("Lisbon itinerary", "Five nights in Alfama."),
    ] {
        let (status, body) = call(
            &router,
            Method::POST,
            "/api/pages",
            Some(json!({ "title": title, "content": content, "creation_kind": "authored" })),
            Caller::Agent("claude-code"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, body) = call(
        &router,
        Method::POST,
        "/api/pages/search",
        Some(json!({ "query": "祇園", "limit": 5 })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let titles: Vec<&str> = body["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, vec!["京都住宿"]);
}

#[tokio::test]
async fn ux_who_6_a_memory_opens_to_the_lines_it_was_said_in() {
    let _guard = data_dir_lock().lock().await;
    let vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    db.create_space("other", None, false).await.unwrap();

    let projects = vault._tmp.path().join(".claude/projects/-release");
    std::fs::create_dir_all(&projects).unwrap();
    let transcript = projects.join("release.jsonl");
    std::fs::write(
        &transcript,
        [
            r#"{"type":"user","message":{"role":"user","content":"Should 0.21 go out in pieces?"},"timestamp":"2026-11-01T10:00:00Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Pieces are safer, one release is simpler."}]},"timestamp":"2026-11-01T10:00:05Z"}"#,
            r#"{"type":"user","message":{"role":"user","content":"那就一次發 0.21，等 Zotero 那幾個 PR 進來。"},"timestamp":"2026-11-01T10:01:00Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Noted."}]},"timestamp":"2026-11-01T10:01:04Z"}"#,
        ]
        .join("\n"),
    )
    .unwrap();
    let (status, body) = call(
        &router,
        Method::POST,
        "/api/sessions",
        Some(json!({
            "session": "claude-code:release",
            "title": "Release 0.21",
            "transcript_path": transcript.to_str().unwrap(),
        })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let session_id = body["id"].as_str().unwrap().to_string();

    let store = |content: &'static str, session: Option<&'static str>| {
        let router = router.clone();
        async move {
            let mut request = Request::builder()
                .method(Method::POST)
                .uri("/api/memory/store")
                .header("content-type", "application/json")
                .header("x-agent-name", "claude-code");
            if let Some(session) = session {
                request = request.header("x-wenlan-session", session);
            }
            let response = router
                .oneshot(
                    request
                        .body(Body::from(
                            json!({ "content": content, "memory_type": "decision" }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            serde_json::from_slice::<Value>(&bytes).unwrap()["source_id"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let decided = store(
        "決定一次發 0.21，等 Zotero 的 PR 進來。",
        Some("claude-code:release"),
    )
    .await;
    let unrelated = store(
        "The office plant needs water on Fridays.",
        Some("claude-code:release"),
    )
    .await;
    let no_conversation = store("Saved outside any conversation.", None).await;

    let (status, body) = call(
        &router,
        Method::GET,
        &format!("/api/memories/{decided}/said"),
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let said = typed::<wenlan_types::conversations::MemorySaidResponse>(body);
    assert_eq!(said.session_id.as_deref(), Some(session_id.as_str()));
    assert!(!said.kept, "the conversation's own lines, not a copy");
    assert_eq!(said.title.as_deref(), Some("Release 0.21"));
    let indexes: Vec<usize> = said.lines.iter().map(|l| l.index).collect();
    assert_eq!(indexes, vec![2], "{said:?}");
    assert_eq!(
        said.lines[0].line.text,
        "那就一次發 0.21，等 Zotero 那幾個 PR 進來。"
    );
    assert_eq!(
        said.lines[0].before.as_ref().map(|l| l.text.as_str()),
        Some("Pieces are safer, one release is simpler.")
    );

    // A memory no line says the same thing as: the conversation, no lines.
    let (status, body) = call(
        &router,
        Method::GET,
        &format!("/api/memories/{unrelated}/said"),
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        typed::<wenlan_types::conversations::MemorySaidResponse>(body)
            .lines
            .is_empty()
    );

    // Not saved in a conversation, unknown, for an AI tool, or out of Space.
    for (uri, caller, expected) in [
        (
            format!("/api/memories/{no_conversation}/said"),
            Caller::Person,
            StatusCode::NOT_FOUND,
        ),
        (
            "/api/memories/mem_missing/said".to_string(),
            Caller::Person,
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/api/memories/{decided}/said"),
            Caller::InSpace("other"),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (status, body) = call(&router, Method::GET, &uri, None, caller).await;
        assert_eq!(status, expected, "{uri}: {body}");
    }
    let (status, _) = call(
        &router,
        Method::GET,
        &format!("/api/memories/{decided}/said"),
        None,
        Caller::Agent("codex"),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "the lines are for the person");
}

/// A chat in ChatGPT or Claude on the web has no transcript Wenlan can read,
/// so the AI tool sends the lines with the memory it saves, and opening the
/// sentence shows those (UX-WHO-6).
#[tokio::test]
async fn ux_who_6_a_web_chat_memory_shows_the_lines_its_ai_tool_kept() {
    let _guard = data_dir_lock().lock().await;
    let _vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;
    db.create_space("other", None, false).await.unwrap();

    let store = |body: Value, session: Option<&'static str>| {
        let router = router.clone();
        async move {
            let mut request = Request::builder()
                .method(Method::POST)
                .uri("/api/memory/store")
                .header("content-type", "application/json")
                .header("x-agent-name", "chatgpt");
            if let Some(session) = session {
                request = request.header("x-wenlan-session", session);
            }
            let response = router
                .oneshot(request.body(Body::from(body.to_string())).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            serde_json::from_slice::<Value>(&bytes).unwrap()["source_id"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let long = "長".repeat(wenlan_types::conversations::MAX_KEPT_SAID_CHARS + 50);
    let before = chrono::Utc::now().timestamp();
    let web = store(
        json!({
            "content": "決定一次發 0.21，等 Zotero 的 PR 進來。",
            "memory_type": "decision",
            "said": [
                { "speaker": "you", "text": "too early to keep" },
                { "speaker": "ai", "text": "Pieces are safer, one release is simpler.", "at": 1 },
                { "speaker": "you", "text": "   " },
                { "speaker": "you", "text": "  那就一次發 0.21，等 Zotero 那幾個 PR 進來。  " },
                { "speaker": "ai", "text": long },
            ],
        }),
        None,
    )
    .await;

    let (status, body) = call(
        &router,
        Method::GET,
        &format!("/api/memories/{web}/said"),
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let said = typed::<wenlan_types::conversations::MemorySaidResponse>(body);
    assert!(said.kept, "{said:?}");
    assert_eq!(said.session_id, None);
    assert_eq!(said.tool, "chatgpt");
    let texts: Vec<&str> = said.lines.iter().map(|l| l.line.text.as_str()).collect();
    // Blank lines dropped, each trimmed, only the three nearest the save.
    assert_eq!(texts.len(), 3, "{texts:?}");
    assert_eq!(texts[0], "Pieces are safer, one release is simpler.");
    assert_eq!(texts[1], "那就一次發 0.21，等 Zotero 那幾個 PR 進來。");
    assert_eq!(
        said.lines[1].line.speaker,
        wenlan_types::conversations::ConversationSpeaker::You
    );
    // Stamped with the save time, never the caller's own.
    for line in &said.lines {
        assert!(line.line.at.is_some_and(|at| at >= before), "{line:?}");
        assert!(line.before.is_none() && line.after.is_none());
    }

    // A conversation Wenlan can't read (named, no transcript) also falls
    // back to the kept lines.
    let unreadable = store(
        json!({
            "content": "The office plant needs water on Fridays.",
            "said": [{ "speaker": "you", "text": "water the plant on Fridays" }],
        }),
        Some("chatgpt:web-chat-1"),
    )
    .await;
    let (status, body) = call(
        &router,
        Method::GET,
        &format!("/api/memories/{unreadable}/said"),
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let said = typed::<wenlan_types::conversations::MemorySaidResponse>(body);
    assert!(said.kept);
    assert_eq!(said.lines.len(), 1);
    assert_eq!(said.lines[0].line.text, "water the plant on Fridays");

    // Kept lines are the person's like any conversation: not for an AI
    // tool, not outside the memory's Space; nothing kept is still a 404.
    let nothing = store(json!({ "content": "Saved with no lines at all." }), None).await;
    for (uri, caller, expected) in [
        (
            format!("/api/memories/{web}/said"),
            Caller::InSpace("other"),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/api/memories/{nothing}/said"),
            Caller::Person,
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (status, body) = call(&router, Method::GET, &uri, None, caller).await;
        assert_eq!(status, expected, "{uri}: {body}");
    }
    let (status, _) = call(
        &router,
        Method::GET,
        &format!("/api/memories/{web}/said"),
        None,
        Caller::Agent("codex"),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "the lines are for the person");
}

#[tokio::test]
async fn an_untitled_conversation_is_named_by_what_the_person_said_first_for_the_person_only() {
    let _guard = data_dir_lock().lock().await;
    let vault = ScratchVault::new();
    let (router, _tmp, db) = common::test_app_no_gate().await;

    let projects = vault._tmp.path().join(".claude/projects/-untitled");
    std::fs::create_dir_all(&projects).unwrap();
    let transcript = projects.join("untitled.jsonl");
    std::fs::write(
        &transcript,
        [
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"},"timestamp":"2026-11-01T10:00:00Z"}"#,
            r#"{"type":"user","message":{"role":"user","content":"Why does this card say untitled?"},"timestamp":"2026-11-01T10:00:01Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Let me look."}]},"timestamp":"2026-11-01T10:00:05Z"}"#,
        ]
        .join("\n"),
    )
    .unwrap();
    // No title from the tool, and no ai-title in the transcript.
    let (status, body) = call(
        &router,
        Method::POST,
        "/api/sessions",
        Some(json!({
            "session": "claude-code:untitled",
            "transcript_path": transcript.to_str().unwrap(),
        })),
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let session_id = body["id"].as_str().unwrap().to_string();
    db.sort_sessions_into_things().await.unwrap();

    let (status, body) = call(
        &router,
        Method::GET,
        &format!("/api/sessions/{session_id}"),
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let detail = typed::<SessionDetailResponse>(body);
    assert_eq!(
        detail.session.title.as_deref(),
        Some("Why does this card say untitled?")
    );
    let thing_id = detail
        .things
        .first()
        .expect("sorted into a thing")
        .id
        .clone();

    let (status, body) = call(
        &router,
        Method::GET,
        &format!("/api/things/{thing_id}"),
        None,
        Caller::Person,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let thing = typed::<ThingDetailResponse>(body);
    assert_eq!(
        thing.sessions[0].title.as_deref(),
        Some("Why does this card say untitled?")
    );

    // The app reads it with no agent name, as it reads the conversation's
    // lines; an AI tool never gets the person's words as a title (UX-FIND-1).
    let (_, body) = call(
        &router,
        Method::GET,
        &format!("/api/things/{thing_id}"),
        None,
        Caller::Unmarked,
    )
    .await;
    assert!(body
        .to_string()
        .contains("Why does this card say untitled?"));

    for uri in [
        format!("/api/sessions/{session_id}"),
        format!("/api/things/{thing_id}"),
    ] {
        let (status, body) = call(&router, Method::GET, &uri, None, Caller::Reader).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        assert!(
            !body.to_string().contains("Why does this card"),
            "{uri} leaked the person's words: {body}"
        );
    }
}
