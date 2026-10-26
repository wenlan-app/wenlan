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
