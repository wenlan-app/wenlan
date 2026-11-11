// SPDX-License-Identifier: Apache-2.0
//! `POST /api/spaces/{name}/shared/sync`: one sync pass of a Space with its
//! shared folder, `GET /api/spaces/{name}/shared`: how it is shared and its
//! last pass, `DELETE` to stop, the same three for one page under
//! `/api/pages/{id}/shared`, the same for the whole library under
//! `/api/library/shared`, `GET /api/shared` and `POST /api/shared/folder`.
//! The pass itself is tested in `wenlan_core::shared_space`; this pins the
//! routes' contract. One test, because the settings file follows
//! `WENLAN_DATA_DIR`, which is the whole process's.

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;
use wenlan_types::shared_space::{
    SharedFolderResponse, SharedKind, SharedListResponse, SharedSyncCounts, SharedSyncRequest,
    SharedSyncStatus,
};

async fn call(
    router: &common::AppRouter,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(json) => {
            request = request.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&json).unwrap())
        }
        None => Body::empty(),
    };
    let response = router
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

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

async fn get_status(router: &common::AppRouter, space: &str) -> (StatusCode, Vec<u8>) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/spaces/{space}/shared"))
                .body(Body::empty())
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
        member_name: Some("Qi-Xuan".to_string()),
        host: None,
        member_id: None,
    };

    let (status, _) = post_sync(&router, "Nowhere", &req).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get_status(&router, "Wedding").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "not shared yet");
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
        &wenlan_core::shared_space::Scope::Space("Wedding".into()),
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

    let (status, body) = get_status(&router, "Wedding").await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let shared: SharedSyncStatus = serde_json::from_slice(&body).unwrap();
    assert_eq!(shared.member_name, "Qi-Xuan");
    assert_eq!(shared.folder, req.folder);
    let last = shared.last.expect("the route's pass is the last one");
    assert_eq!(last.counts, counts);
    assert_eq!(last.paused, None);

    let (status, body) = post_sync(&router, "Wedding", &req).await;
    assert_eq!(status, StatusCode::OK);
    let counts: SharedSyncCounts = serde_json::from_slice(&body).unwrap();
    assert_eq!(counts, SharedSyncCounts::default());

    // One page of another Space, shared alone, with no name given: the
    // profile's display name is used, and without one it is refused.
    db.create_space("Private", None, false).await.unwrap();
    let venue = common::create_page_fixture(
        &db,
        "Venue",
        "The old mill.",
        Some("Private"),
        &[],
        "authored",
    )
    .await;
    let venue_folder = tempfile::tempdir().unwrap();
    let venue_folder = venue_folder.path().join("Venue");
    let page_req = serde_json::json!({ "folder": venue_folder.to_string_lossy() });
    let uri = format!("/api/pages/{venue}/shared/sync");
    std::env::set_var("WENLAN_TEST_NO_COMPUTER_NAME", "1");
    let (status, _) = call(&router, Method::POST, &uri, Some(page_req.clone())).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "no name to show");
    std::env::remove_var("WENLAN_TEST_NO_COMPUTER_NAME");
    // With no display name, the computer account's name becomes it.
    std::env::set_var("USER", "qixuan-test-account");
    let (status, body) = call(
        &router,
        Method::POST,
        "/api/spaces/Wedding/shared/sync",
        Some(serde_json::json!({ "folder": req.folder })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let profile = db.get_profile().await.unwrap().expect("a profile");
    assert!(
        profile.display_name.is_some(),
        "the computer's name is kept"
    );
    db.update_profile(&profile.id, None, Some("Qi-Xuan L"), None, None, None)
        .await
        .unwrap();
    let (status, body) = call(&router, Method::POST, &uri, Some(page_req)).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let counts: SharedSyncCounts = serde_json::from_slice(&body).unwrap();
    assert_eq!(counts.exported, 1);
    // A page of the shared Space is not shared alone.
    let (status, _) = call(
        &router,
        Method::POST,
        &format!("/api/pages/{page}/shared/sync"),
        Some(serde_json::json!({ "folder": venue_folder.with_file_name("Seating").to_string_lossy() })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, body) = call(
        &router,
        Method::POST,
        "/api/shared/folder",
        Some(serde_json::json!({ "folder": venue_folder.to_string_lossy() })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let peek: SharedFolderResponse = serde_json::from_slice(&body).unwrap();
    let peek = peek.shared.expect("the folder shares the page");
    assert_eq!(peek.kind, SharedKind::Page);
    assert_eq!(peek.name, "Venue");
    assert_eq!(peek.page_id.as_deref(), Some(venue.as_str()));
    assert_eq!(peek.members[0].name, "Qi-Xuan L");
    let (status, body) = call(
        &router,
        Method::POST,
        "/api/shared/folder",
        Some(
            serde_json::json!({ "folder": venue_folder.with_file_name("empty").to_string_lossy() }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let peek: SharedFolderResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(peek.shared, None);

    let (status, body) = call(&router, Method::GET, "/api/shared", None).await;
    assert_eq!(status, StatusCode::OK);
    let list: SharedListResponse = serde_json::from_slice(&body).unwrap();
    let kinds: Vec<_> = list
        .shares
        .iter()
        .map(|share| (share.kind, share.target.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (SharedKind::Space, "Wedding"),
            (SharedKind::Page, venue.as_str())
        ]
    );
    let (status, body) = call(
        &router,
        Method::GET,
        &format!("/api/pages/{venue}/shared"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let page_status: SharedSyncStatus = serde_json::from_slice(&body).unwrap();
    assert_eq!(page_status.member_name, "Qi-Xuan L");

    // Stop sharing both; the pages stay.
    for uri in [
        format!("/api/pages/{venue}/shared"),
        "/api/spaces/Wedding/shared".to_string(),
    ] {
        let (status, _) = call(&router, Method::DELETE, &uri, None).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        let (status, _) = call(&router, Method::DELETE, &uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    let (_, body) = call(&router, Method::GET, "/api/shared", None).await;
    let list: SharedListResponse = serde_json::from_slice(&body).unwrap();
    assert!(list.shares.is_empty());
    assert!(db.get_page(&venue).await.unwrap().is_some());
    assert!(db.get_page(&page).await.unwrap().is_some());

    // The whole library, between this person's own computers.
    let (status, _) = call(&router, Method::GET, "/api/library/shared", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "not shared yet");
    let library_folder = tempfile::tempdir().unwrap();
    let (status, body) = call(
        &router,
        Method::POST,
        "/api/library/shared/sync",
        Some(serde_json::json!({
            "folder": library_folder.path().to_string_lossy(),
            "member_name": "Qi-Xuan",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let counts: SharedSyncCounts = serde_json::from_slice(&body).unwrap();
    assert_eq!(counts.exported, 2, "both pages, none shared elsewhere now");
    let (status, body) = call(&router, Method::GET, "/api/library/shared", None).await;
    assert_eq!(status, StatusCode::OK);
    let library: SharedSyncStatus = serde_json::from_slice(&body).unwrap();
    assert_eq!(library.kind, SharedKind::Library);
    let (_, body) = call(
        &router,
        Method::POST,
        "/api/shared/folder",
        Some(serde_json::json!({ "folder": library_folder.path().to_string_lossy() })),
    )
    .await;
    let peek: SharedFolderResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(peek.shared.unwrap().kind, SharedKind::Library);
    let (_, body) = call(&router, Method::GET, "/api/shared", None).await;
    let list: SharedListResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(list.shares.len(), 1);
    assert_eq!(list.shares[0].kind, SharedKind::Library);
    let (status, _) = call(&router, Method::DELETE, "/api/library/shared", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&router, Method::DELETE, "/api/library/shared", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(library_folder.path().join("space.json").exists());
    assert!(db.get_page(&venue).await.unwrap().is_some());
}
