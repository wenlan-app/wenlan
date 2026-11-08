// SPDX-License-Identifier: Apache-2.0
//! `POST /api/spaces/{name}/shared/sync`: one sync pass of a Space with its
//! shared folder. The pass itself is tested in `wenlan_core::shared_space`;
//! this pins the route's contract.

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;
use wenlan_types::shared_space::{SharedSyncCounts, SharedSyncRequest};

async fn post_sync(
    router: &common::AppRouter,
    space: &str,
    req: &SharedSyncRequest,
) -> (StatusCode, Vec<u8>) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/spaces/{space}/shared/sync"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

#[tokio::test]
async fn shared_sync_writes_the_space_pages_and_returns_counts() {
    // The settings file goes under the data root: keep it in a temp folder.
    let data = tempfile::tempdir().unwrap();
    std::env::set_var("WENLAN_DATA_DIR", data.path());
    let (router, _tmp, db) = common::test_app_no_gate().await;
    db.create_space("Wedding", None, false).await.unwrap();
    let page = common::create_page_fixture(
        &db,
        "Seating plan",
        "Table 1: Ana and Ben.",
        Some("Wedding"),
        &[],
        "authored",
    )
    .await;
    let shared = tempfile::tempdir().unwrap();
    let req = SharedSyncRequest {
        folder: shared.path().to_string_lossy().into_owned(),
        member_name: "Qi-Xuan".to_string(),
    };

    let (status, _) = post_sync(&router, "Nowhere", &req).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let relative = SharedSyncRequest {
        folder: "shared".to_string(),
        ..req.clone()
    };
    let (status, _) = post_sync(&router, "Wedding", &relative).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, body) = post_sync(&router, "Wedding", &req).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let counts: SharedSyncCounts = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        counts,
        SharedSyncCounts {
            exported: 1,
            ..SharedSyncCounts::default()
        }
    );
    assert!(shared.path().join("space.json").is_file());
    let settings = wenlan_core::shared_space::share_settings_at(
        &data.path().join(wenlan_core::shared_space::SETTINGS_FILE),
        "Wedding",
    )
    .unwrap()
    .expect("the folder and member are remembered");
    let versions = shared
        .path()
        .join("members")
        .join(&settings.member_id)
        .join("pages")
        .join(&page);
    assert_eq!(std::fs::read_dir(versions).unwrap().count(), 1);

    let (status, body) = post_sync(&router, "Wedding", &req).await;
    assert_eq!(status, StatusCode::OK);
    let counts: SharedSyncCounts = serde_json::from_slice(&body).unwrap();
    assert_eq!(counts, SharedSyncCounts::default());
}
