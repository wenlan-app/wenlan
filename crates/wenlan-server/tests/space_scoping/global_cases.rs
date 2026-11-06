// SPDX-License-Identifier: Apache-2.0

use super::case_runner::assert_global_executed_keys;
use super::fixture::ScopeFixture;
use axum::body::{to_bytes, Body};
use axum::http::{Method as HttpMethod, Request, Response, StatusCode};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tower::ServiceExt;
use wenlan_core::sources::zotero::ZoteroClient;
use wenlan_server::sensitive_read_routes::Method;

async fn response_parts(response: Response<Body>) -> (StatusCode, Vec<u8>) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, bytes.to_vec())
}

async fn zotero_collections_ignore_space(fixture: &ScopeFixture) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/api", listener.local_addr().unwrap());
    let client = ZoteroClient::for_test_endpoint(&endpoint).unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let upstream = axum::Router::new().route(
        "/api/users/0/collections",
        axum::routing::get(move || {
            let observed = observed.clone();
            async move {
                observed.fetch_add(1, Ordering::SeqCst);
                (
                    [
                        ("Zotero-Server-ID", "scope-fixture"),
                        ("Total-Results", "1"),
                    ],
                    axum::Json(serde_json::json!([{
                        "key": "TEST1234", "version": 1,
                        "data": {"name": "Global Zotero collection"},
                        "meta": {"numItems": 3}
                    }])),
                )
            }
        }),
    ).route(
        "/api/users/0/collections/TEST1234/items/top",
        axum::routing::get(|| async {
            (
                [("Zotero-Server-ID", "scope-fixture"), ("Total-Results", "1")],
                axum::Json(serde_json::json!([{
                    "key": "ARTICLE1", "version": 1,
                    "data": {"itemType": "journalArticle", "title": "Global selectable paper", "collections": ["TEST1234"]}
                }])),
            )
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let mut responses = Vec::new();
    for space in [None, Some("work"), Some("missing-space")] {
        // Request-local extension: no process environment or production router
        // configuration changes, and no request can reach the user's Zotero.
        let mut request = Request::builder()
            .uri("/api/zotero/collections")
            .extension(client.clone());
        if let Some(space) = space {
            request = request.header("x-wenlan-space", space);
        }
        let response = fixture
            .router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        responses.push(response_parts(response).await);
    }
    let mut item_responses = Vec::new();
    for space in [None, Some("work"), Some("missing-space")] {
        let mut request = Request::builder()
            .uri("/api/zotero/items?collection_key=TEST1234&expected_server_id=scope-fixture")
            .extension(client.clone());
        if let Some(space) = space {
            request = request.header("x-wenlan-space", space);
        }
        item_responses.push(
            response_parts(
                fixture
                    .router
                    .clone()
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap(),
            )
            .await,
        );
    }
    server.abort();
    let _ = server.await;
    // Item discovery also verifies this exact collection before reading it.
    assert_eq!(requests.load(Ordering::SeqCst), 6);
    let baseline = &responses[0];
    assert_eq!(baseline.0, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&baseline.1).unwrap(),
        serde_json::json!({
            "server_id": "scope-fixture",
            "collections": [{"key": "TEST1234", "name": "Global Zotero collection", "item_count": 3}]
        })
    );
    for selected in &responses[1..] {
        assert_eq!(
            selected, baseline,
            "Global Zotero collection route honored Space"
        );
    }
    let item_baseline = &item_responses[0];
    assert_eq!(item_baseline.0, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&item_baseline.1).unwrap(),
        serde_json::json!({"server_id":"scope-fixture", "collection_key":"TEST1234", "complete":true,
            "items":[{"key":"ARTICLE1", "title":"Global selectable paper", "item_type":"journalArticle"}]})
    );
    for selected in &item_responses[1..] {
        assert_eq!(
            selected, item_baseline,
            "Global Zotero article discovery honored Space"
        );
    }
}

async fn zotero_disconnect_preview_is_global(fixture: &ScopeFixture) {
    let space = fixture.db.get_space("work").await.unwrap().unwrap();
    let identity = wenlan_types::zotero::ZoteroRegistrationIdentity {
        registration_id: "scope-zotero-retained".into(),
        locator: "zotero-local/73636f70652d66697874757265/users/0/collections/TEST1234".into(),
        server_id: "scope-fixture".into(),
        space_id: space.id,
    };
    fixture
        .db
        .bind_zotero_registration(&identity)
        .await
        .unwrap();
    fixture.db.begin_zotero_disconnect(&identity).await.unwrap();
    let uri = "/api/zotero/sources/scope-zotero-retained/disconnect-preview";
    let baseline = response_parts(fixture.send(HttpMethod::GET, uri, None, None).await).await;
    assert_eq!(baseline.0, StatusCode::OK);
    let preview: wenlan_types::zotero::ZoteroDisconnectPreview =
        serde_json::from_slice(&baseline.1).unwrap();
    assert_eq!(preview.identity, identity);
    assert_eq!(preview.space_name, "work");
    assert!(preview.pending_disconnect);
    assert!(preview.impact_scope_known);
    assert!(preview.canonical_item_ids.is_empty());
    assert!(preview.affected_page_ids.is_empty());
    let json: serde_json::Value = serde_json::from_slice(&baseline.1).unwrap();
    assert!(
        json.get("content").is_none(),
        "global preview must not return Page prose"
    );
    for selected in ["work", "personal", "missing-space"] {
        assert_eq!(
            response_parts(
                fixture
                    .send(HttpMethod::GET, uri, None, Some(selected))
                    .await
            )
            .await,
            baseline,
            "registration impact metadata must not change under caller Space header"
        );
    }
    let missing = response_parts(
        fixture
            .send(
                HttpMethod::GET,
                "/api/zotero/sources/missing-zotero/disconnect-preview",
                None,
                None,
            )
            .await,
    )
    .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
    assert!(
        !String::from_utf8(missing.1)
            .unwrap()
            .contains("scope-zotero-retained"),
        "missing registration cannot disclose another retained identity"
    );
}

#[tokio::test]
async fn zotero_disconnect_preview_global_scope_metadata() {
    let fixture = ScopeFixture::new().await;
    zotero_disconnect_preview_is_global(&fixture).await;
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
        ("/api/capture-stats", "/api/capture-stats"),
        ("/api/activity", "/api/activity"),
        ("/api/decisions/domains", "/api/decisions/domains"),
        ("/api/snapshots", "/api/snapshots"),
        ("/api/palace/layout", "/api/palace/layout"),
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
    zotero_collections_ignore_space(&fixture).await;
    executed.push((Method::Get, "/api/zotero/collections"));
    executed.push((Method::Get, "/api/zotero/items"));
    zotero_disconnect_preview_is_global(&fixture).await;
    executed.push((Method::Get, "/api/zotero/sources/{id}/disconnect-preview"));
    assert_global_executed_keys(executed);
}
