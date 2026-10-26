// SPDX-License-Identifier: Apache-2.0
//! On demand, a new-page claim hands out the organizing its sources need and
//! `POST /api/steep/new/organize` lands the small model's answers. The test
//! daemon has no everyday model, so it is On demand.

mod common;

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use serde_json::{json, Value};
use tower::ServiceExt;
use wenlan_server::router::AppRouter;
use wenlan_types::{SteepNewClaimResponse, SteepOrganizeCommitResponse};

const MIRA: &str = "Mira fires the kiln in Oslo every Sunday. She keeps the glaze recipes \
                    in a red notebook and tests each glaze on a small tile first.";
const KILN: &str = "Mira says the kiln needs eight hours to cool before anyone opens it. \
                    Opening it early cracked two bowls last winter.";
const CLASSIFY: &str = r#"{"memory_type": "lesson", "domain": "ceramics", "tags": ["kiln"], "quality": "high", "importance": 6}"#;
const ENTITIES: &str = r#"[{"i": 1, "entities": [{"name": "Mira", "type": "person"}], "observations": [], "relations": []}]"#;

async fn send(
    app: &AppRouter,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_new_page_claim_hands_out_organizing_and_takes_the_answers() {
    let (app, _dir, db) = common::test_app().await;
    let now = chrono::Utc::now().timestamp();
    common::insert_memory(
        &db,
        "org_mira",
        MIRA,
        "memory",
        Some("obsidian"),
        None,
        false,
        now,
    )
    .await;
    common::insert_memory(
        &db,
        "org_kiln",
        KILN,
        "memory",
        Some("obsidian"),
        None,
        false,
        now,
    )
    .await;

    let (status, body) = send(
        &app,
        Method::POST,
        "/api/steep/new/claim",
        Some(json!({"item_ids": ["org_mira", "org_kiln"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let claim: SteepNewClaimResponse = serde_json::from_value(body).unwrap();
    let work = claim
        .organize
        .expect("no everyday model: the caller organizes");
    assert!(
        work.stages
            .iter()
            .any(|stage| stage.stage == "classify" && stage.instructions.contains("memory_type")),
        "{:?}",
        work.stages
    );
    let answers: Vec<Value> = work
        .tasks
        .iter()
        .map(|task| {
            json!({
                "n": task.n,
                "answer": if task.stage == "classify" { CLASSIFY } else { ENTITIES },
            })
        })
        .collect();
    let classify_tasks = work.tasks.iter().filter(|t| t.stage == "classify").count();
    assert_eq!(classify_tasks, 2, "{:?}", work.tasks);

    // The helper fetches the same tasks with only the token.
    let (status, body) = send(
        &app,
        Method::POST,
        "/api/steep/new/tasks",
        Some(json!({"token": claim.token})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["organize"]["tasks"].as_array().map(Vec::len),
        Some(work.tasks.len()),
        "{body}"
    );

    let (status, body) = send(
        &app,
        Method::POST,
        "/api/steep/new/organize",
        Some(json!({"token": claim.token, "answers": answers})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let done: SteepOrganizeCommitResponse = serde_json::from_value(body).unwrap();
    assert_eq!(done.classified, 2);
    assert_eq!(done.remaining, 0);
    assert_eq!(done.linked, 2, "{done:?}");

    // An answer lands once.
    let (status, body) = send(
        &app,
        Method::POST,
        "/api/steep/new/organize",
        Some(json!({"token": claim.token, "answers": [{"n": 1, "answer": CLASSIFY}]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["unknown"], json!([1]));

    let (status, body) = send(
        &app,
        Method::POST,
        "/api/steep/new/tasks",
        Some(json!({"token": claim.token})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.get("organize").is_none(),
        "every task answered: {body}"
    );
}

#[tokio::test]
async fn organize_with_a_lost_token_is_refused() {
    let (app, _dir, _db) = common::test_app().await;
    let (status, body) = send(
        &app,
        Method::POST,
        "/api/steep/new/organize",
        Some(json!({"token": "gone", "answers": [{"n": 1, "answer": "{}"}]})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}

/// A group whose topic names an existing page goes into it: the list offers
/// the page's title for grouping, the claim names it, `links` reads it
/// fresh, and the `attach` commit adds the sources to it.
#[tokio::test]
async fn a_group_named_after_an_existing_page_attaches_to_it() {
    let (app, _dir, db) = common::test_app().await;
    let now = chrono::Utc::now().timestamp();
    for (id, text) in [("att_mira", MIRA), ("att_kiln", KILN)] {
        common::insert_memory(&db, id, text, "memory", Some("obsidian"), None, false, now).await;
    }
    let page = wenlan_core::post_write::page_write(
        &db,
        wenlan_core::post_write::PageWrite::Create {
            page_id: None,
            req: wenlan_types::requests::CreateConceptRequest {
                title: "Kiln Renovation".into(),
                content: "The kiln was rebuilt.".into(),
                summary: None,
                entity_id: None,
                space: wenlan_types::WriteSpaceTarget::Uncategorized,
                source_memory_ids: Vec::new(),
                creation_kind: Some("authored".into()),
                workspace: None,
            },
            agent: "test",
            knowledge_path: None,
            page_min_cluster_size: 0,
            page_match_threshold: 2.0,
            citations_json: None,
            basis_marks: Vec::new(),
            basis_space_guard: None,
            skip_protected_title_match: false,
        },
    )
    .await
    .unwrap();

    let (status, body) = send(&app, Method::GET, "/api/steep/new", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["page_titles"], json!(["Kiln Renovation"]), "{body}");

    let (status, body) = send(
        &app,
        Method::POST,
        "/api/steep/new/claim",
        Some(json!({"item_ids": ["att_mira", "att_kiln"], "topic": "Kiln renovation 2024"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let claim: SteepNewClaimResponse = serde_json::from_value(body).unwrap();
    let existing = claim.existing_page.expect("the topic names the page");
    assert_eq!(existing.page_id, page.id);
    assert!(claim.note.contains("\"attach\""), "{}", claim.note);

    let (status, body) = send(
        &app,
        Method::POST,
        "/api/steep/new/links",
        Some(json!({"token": claim.token})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["existing_page"]["title"], "Kiln Renovation", "{body}");

    let (status, body) = send(
        &app,
        Method::POST,
        "/api/steep/new/commit",
        Some(json!({"token": claim.token, "action": "attach"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "attached", "{body}");
    assert_eq!(body["title"], "Kiln Renovation", "{body}");
    let attached = db.get_page(&page.id).await.unwrap().unwrap();
    assert_eq!(attached.stale_reason.as_deref(), Some("source_updated"));

    let (status, body) = send(&app, Method::GET, "/api/steep/new", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 0, "{body}");

    let (status, body) = send(
        &app,
        Method::POST,
        "/api/steep/new/links",
        Some(json!({"token": claim.token})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "the claim ended: {body}");
}
