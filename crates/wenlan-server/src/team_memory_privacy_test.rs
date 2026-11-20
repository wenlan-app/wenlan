// SPDX-License-Identifier: AGPL-3.0-only

use super::{secret, KIND_HEADER, MEMBER_HEADER, SECRET_HEADER};
use crate::router::{build_router, AppRouter};
use crate::state::ServerState;
use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;
use wenlan_core::{db::MemoryDB, events::NoopEmitter, sources::RawDocument};

const SPACE: &str = "Design";
const AGENT: &str = "privacy-test";

async fn request(
    app: &AppRouter,
    method: Method,
    path: &str,
    body: Value,
    forwarded: bool,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("x-agent-name", AGENT)
        .header(crate::space_header::HEADER_NAME, SPACE);
    if forwarded {
        req = req
            .header(SECRET_HEADER, secret())
            .header(MEMBER_HEADER, "reader")
            .header(KIND_HEADER, "person");
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn post_as_member(app: &AppRouter, path: &str, body: Value) -> Value {
    let (status, body) = request(app, Method::POST, path, body, true).await;
    assert_eq!(status, StatusCode::OK, "{path}");
    body
}

async fn fixture() -> (AppRouter, Arc<MemoryDB>, tempfile::TempDir) {
    let scratch = tempfile::tempdir().unwrap();
    let db = Arc::new(
        MemoryDB::new(scratch.path(), Arc::new(NoopEmitter))
            .await
            .unwrap(),
    );
    db.create_space(SPACE, None, false).await.unwrap();
    db.apply_brief_update(
        &serde_json::from_value(json!({
            "space": SPACE,
            "caller_id": "host",
            "operation_id": "public-brief",
            "mutations": [{"kind": "add", "text": "Review the design", "state": "active"}]
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    let state = ServerState {
        db: Some(db.clone()),
        ..ServerState::default()
    }
    .with_brief_status_root(scratch.path().join("status"));
    let app = build_router(Arc::new(RwLock::new(state)));
    (app, db, scratch)
}

#[tokio::test]
async fn hosted_reads_skip_private_retrieval_and_library_onboarding() {
    let (app, db, _scratch) = fixture().await;
    // The empty library makes it safe to check that recall never enters the
    // private search bookkeeping, and Brief never computes local material counts.
    let recall = post_as_member(&app, "/api/memory/search", json!({"query": "design"})).await;
    let brief = post_as_member(&app, "/api/brief", json!({"topic": "design"})).await;
    assert_eq!(recall["results"], json!([]));
    assert_eq!(brief["brief"]["active"][0]["text"], "Review the design");
    let searches = db.list_agent_activity(10, Some(AGENT), None).await.unwrap();
    assert_eq!(
        (searches.len(), brief.get("getting_started").is_some()),
        (0, false),
        "a hosted read entered private search bookkeeping or exposed library onboarding"
    );
}

#[tokio::test]
async fn hosted_readers_keep_shared_pages_without_private_memory_data() {
    const PRIVATE_ID: &str = "fixture-private-evidence";
    const PRIVATE_TEXT: &str = "design citrinefixtureproof";
    const SHARED_TITLE: &str = "Design meeting";
    const SHARED_TEXT: &str = "The design meeting starts Tuesday. [1]";
    let (app, db, _scratch) = fixture().await;
    db.upsert_documents(vec![RawDocument {
        source: "memory".into(),
        source_id: PRIVATE_ID.into(),
        title: "Fixture memory".into(),
        content: PRIVATE_TEXT.into(),
        space: Some(SPACE.into()),
        confirmed: Some(true),
        ..Default::default()
    }])
    .await
    .unwrap();
    let page = wenlan_core::post_write::create_page_with_floor(
        &db,
        wenlan_types::requests::CreateConceptRequest {
            title: SHARED_TITLE.into(),
            content: SHARED_TEXT.into(),
            summary: None,
            entity_id: None,
            space: Some(SPACE.to_string()).into(),
            source_memory_ids: vec![PRIVATE_ID.into()],
            creation_kind: Some("distilled".into()),
            workspace: Some(SPACE.into()),
        },
        AGENT,
        None,
        1,
    )
    .await
    .unwrap();
    let proof = json!([{
        "occurrence": 1, "marker": 1, "source_kind": "memory",
        "locator": PRIVATE_ID, "score": 1.0, "status": "verified",
    }]);
    let history = json!([{
        "version": 1, "at": 1, "edited_by": "agent_refresh",
        "incoming_source_ids": [PRIVATE_ID], "sources_added": 1,
    }]);
    db.set_page_citations_with_changelog(&page.id, Some(&proof.to_string()), &history.to_string())
        .await
        .unwrap();

    let query = json!({"query": "design"});
    let (status, local) = request(
        &app,
        Method::POST,
        "/api/memory/search",
        query.clone(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(local["results"].to_string().contains(PRIVATE_TEXT));
    let source_path = format!("/api/pages/{}/sources", page.id);
    let (status, local) = request(&app, Method::GET, &source_path, Value::Null, false).await;
    assert_eq!(status, StatusCode::OK);
    assert!(local.to_string().contains(PRIVATE_TEXT));

    for rerank in [false, true] {
        let recall = post_as_member(
            &app,
            "/api/memory/search",
            json!({"query": "design", "rerank": rerank}),
        )
        .await;
        assert_eq!(recall["results"], json!([]));
        assert_eq!(recall["supplemental_pages"][0]["content"], SHARED_TEXT);
    }
    let brief = post_as_member(&app, "/api/brief", json!({"topic": "design"})).await;
    assert_eq!(
        brief["related_context"]["results"][0]["content"],
        SHARED_TEXT
    );
    assert_eq!(
        brief["related_context"]["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(brief.get("getting_started").is_none());

    let (_, sources) = request(&app, Method::GET, &source_path, Value::Null, true).await;
    assert_eq!(sources, json!([]));
    let consumers = [
        (Method::GET, format!("/api/pages/{}", page.id), SHARED_TEXT),
        (
            Method::GET,
            format!("/api/pages/{}/revisions", page.id),
            "agent_refresh",
        ),
        (Method::GET, "/api/pages".to_string(), SHARED_TITLE),
        (Method::POST, "/api/pages/search".to_string(), SHARED_TITLE),
        (
            Method::GET,
            "/api/pages/recent?since_ms=1".to_string(),
            SHARED_TITLE,
        ),
        (
            Method::GET,
            "/api/pages/recent-changes".to_string(),
            SHARED_TITLE,
        ),
        (Method::GET, "/api/memory/graph".to_string(), SHARED_TITLE),
    ];
    for (method, path, expected) in consumers {
        let (status, body) = request(&app, method, &path, query.clone(), true).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        let wire = body.to_string();
        assert!(wire.contains(expected), "{path} lost the shared page");
        assert!(
            !wire.contains(PRIVATE_ID) && !wire.contains(PRIVATE_TEXT),
            "{path}"
        );
    }
    let (status, counts) = request(&app, Method::GET, "/api/spaces", Value::Null, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(counts[0]["name"], SPACE);
    assert_eq!(counts[0]["memory_count"], 0);
    assert_eq!(counts[0]["entity_count"], 0);
    let (status, rules) = request(&app, Method::GET, "/api/skills/rules", Value::Null, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rules["space_name"], SPACE);

    let (status, _) = request(
        &app,
        Method::POST,
        "/api/pages",
        json!({"title": "Team update", "content": SHARED_TEXT, "source_memory_ids": [PRIVATE_ID]}),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let written = post_as_member(
        &app,
        "/api/pages",
        json!({"title": "Team update", "content": "Publish the new meeting agenda."}),
    )
    .await;
    assert!(written["id"].as_str().is_some());
}

#[tokio::test]
async fn hosted_forwarding_does_not_admit_private_readers() {
    let (app, _, _scratch) = fixture().await;
    for (method, path) in [
        (Method::POST, "/api/search"),
        (Method::POST, "/api/context"),
        (Method::GET, "/api/memory/stats"),
        (Method::POST, "/api/memory/entities/list"),
        (Method::GET, "/api/memory/synthetic/detail"),
        (Method::GET, "/api/memory/by-ids"),
        (Method::GET, "/api/steep/worklist"),
    ] {
        let (status, _) = request(&app, method, path, Value::Null, true).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
}
