// SPDX-License-Identifier: Apache-2.0

use super::case_runner::assert_global_executed_keys;
use super::fixture::ScopeFixture;
use axum::body::{to_bytes, Body};
use axum::http::{Method as HttpMethod, Response, StatusCode};
use wenlan_server::sensitive_read_routes::Method;

async fn response_parts(response: Response<Body>) -> (StatusCode, Vec<u8>) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, bytes.to_vec())
}

pub async fn global_routes_enforce_scope_contracts() {
    let fixture = ScopeFixture::new().await;
    let probes = [
        ("/api/profile", "/api/profile"),
        ("/api/agents", "/api/agents"),
        ("/api/agents/missing-agent", "/api/agents/{name}"),
        ("/api/memory/stats", "/api/memory/stats"),
        ("/api/spaces", "/api/spaces"),
        ("/api/spaces/default", "/api/spaces/default"),
        ("/api/sources", "/api/sources"),
        (
            "/api/sources/missing-source/ai-changes?path=Plan.md",
            "/api/sources/{id}/ai-changes",
        ),
        ("/api/notes/proposals", "/api/notes/proposals"),
        (
            "/api/notes/proposals/missing-proposal",
            "/api/notes/proposals/{id}",
        ),
        ("/api/profile/narrative", "/api/profile/narrative"),
        ("/api/knowledge/count", "/api/knowledge/count"),
        ("/api/onboarding/milestones", "/api/onboarding/milestones"),
        ("/api/import/state", "/api/import/state"),
        ("/api/import/batches/active", "/api/import/batches/active"),
        (
            "/api/import/batches/missing-batch/status",
            "/api/import/batches/{batch_id}/status",
        ),
        ("/api/memory/rejections", "/api/memory/rejections"),
        ("/api/refinery/queue", "/api/refinery/queue"),
        ("/api/capture-stats", "/api/capture-stats"),
        ("/api/activity", "/api/activity"),
        ("/api/decisions/domains", "/api/decisions/domains"),
        ("/api/snapshots", "/api/snapshots"),
    ];
    let mut executed = Vec::new();

    for (uri, catalog_path) in probes {
        let baseline = response_parts(fixture.send(HttpMethod::GET, uri, None, None).await).await;
        let selected = response_parts(
            fixture
                .send(HttpMethod::GET, uri, None, Some("missing-space"))
                .await,
        )
        .await;

        assert_eq!(selected, baseline, "Global route honored Space: {uri}");
        executed.push((Method::Get, catalog_path));
    }

    let created = response_parts(
        fixture
            .send(
                HttpMethod::POST,
                "/api/knowledge/folders",
                Some(serde_json::json!({"parent_path": "", "name": "Global-folder"})),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(created.0, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&created.1).unwrap()["path"],
        "Global-folder"
    );
    assert!(fixture._tmp.path().join("pages/Global-folder").is_dir());

    let scoped_create = response_parts(
        fixture
            .send(
                HttpMethod::POST,
                "/api/knowledge/folders",
                Some(serde_json::json!({"parent_path": "", "name": "Forbidden-folder"})),
                Some("work"),
            )
            .await,
    )
    .await;
    assert_eq!(scoped_create.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!fixture._tmp.path().join("pages/Forbidden-folder").exists());

    executed.push((Method::Post, "/api/knowledge/folders"));

    assert_global_executed_keys(executed);
}
