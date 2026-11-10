// SPDX-License-Identifier: Apache-2.0
//! Real in-process HTTP contracts for moving a page between Spaces; every DB
//! and projection path is scratch.
mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use wenlan_types::responses::{PageProjectionStatus, SetPageSpaceResponse};

async fn send(
    app: &common::AppRouter,
    path: &str,
    body: Value,
    space: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json")
        .header("x-wenlan-truth-contract", "1")
        .header("x-wenlan-reader-intent", "explicit");
    if let Some(space) = space {
        request = request.header("x-wenlan-space", space);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

/// The one visible projected Markdown file whose frontmatter names `id`; a
/// move must not leave a second copy behind. Hidden folders (the writer's own
/// `.wenlan` recovery copies) are not part of the pages a person browses.
fn projected_file(root: &std::path::Path, id: &str) -> String {
    let needle = format!("origin_id: {id}\n");
    let mut stack = vec![root.to_path_buf()];
    let mut found = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let hidden = path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'));
            if path.is_dir() {
                if !hidden {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|ext| ext == "md") {
                let text = std::fs::read_to_string(&path).unwrap();
                if text.contains(&needle) {
                    found.push((path.display().to_string(), text));
                }
            }
        }
    }
    assert_eq!(
        found.len(),
        1,
        "visible projected files for {id}: {found:#?}"
    );
    found.pop().unwrap().1
}

#[tokio::test]
async fn space_route_moves_the_page_and_its_projected_space_line() {
    let (app, tmp, db) = common::test_app().await;
    db.create_space("work", None, false).await.unwrap();
    let id = common::create_page_fixture(
        &db,
        "Moving note",
        "Body that stays put.",
        None,
        &[],
        "authored",
    )
    .await;
    let version = db.get_page(&id).await.unwrap().unwrap().version;
    let path = format!("/api/pages/{id}/space");

    let (status, value) = send(
        &app,
        &path,
        json!({"space":"work","expected_version":version}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let moved: SetPageSpaceResponse = serde_json::from_value(value).unwrap();
    assert_eq!(
        (
            moved.page_id.as_str(),
            moved.space.as_deref(),
            moved.version
        ),
        (id.as_str(), Some("work"), version + 1)
    );
    assert!(moved.unreadable_source_positions.is_empty());
    assert_eq!(moved.projection_status, PageProjectionStatus::Synced);
    let page = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(page.space.as_deref(), Some("work"));
    assert_eq!(page.content, "Body that stays put.");
    let file = projected_file(&tmp.path().join("pages"), &id);
    assert!(file.contains("space: \"work\"\n"), "{file}");
    assert!(file.contains(&format!("origin_version: {}\n", version + 1)));

    // Moving back to no Space drops the `space:` line again.
    let (status, value) = send(
        &app,
        &path,
        json!({"space":"","expected_version":version + 1}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let back: SetPageSpaceResponse = serde_json::from_value(value).unwrap();
    assert_eq!((back.space, back.version), (None, version + 2));
    assert_eq!(back.projection_status, PageProjectionStatus::Synced);
    let file = projected_file(&tmp.path().join("pages"), &id);
    assert!(!file.contains("\nspace: "), "{file}");
}

#[tokio::test]
async fn space_route_refusals_are_typed_and_write_nothing() {
    let (app, _tmp, db) = common::test_app().await;
    db.create_space("work", None, false).await.unwrap();
    let id = common::create_page_fixture(
        &db,
        "Moving note",
        "Body that stays put.",
        None,
        &[],
        "authored",
    )
    .await;
    let version = db.get_page(&id).await.unwrap().unwrap().version;
    let path = format!("/api/pages/{id}/space");
    let cases = [
        (
            json!({"space":"nowhere","expected_version":version}),
            None,
            StatusCode::UNPROCESSABLE_ENTITY,
            json!("page_space_unknown"),
            json!(version),
        ),
        (
            json!({"space":"work","expected_version":version + 1}),
            None,
            StatusCode::CONFLICT,
            json!("page_space_version_conflict"),
            json!(version),
        ),
        (
            json!({"space":"work","expected_version":version}),
            Some("work"),
            StatusCode::FORBIDDEN,
            json!("page_space_needs_global_scope"),
            Value::Null,
        ),
    ];
    for (body, space, status_expected, code, current) in cases {
        let (status, value) = send(&app, &path, body, space).await;
        assert_eq!(status, status_expected, "{value}");
        assert_eq!(value["code"], code, "{value}");
        assert_eq!(value["current_version"], current, "{value}");
        assert!(
            value["error"]
                .as_str()
                .is_some_and(|error| error.starts_with(&format!("{}:", code.as_str().unwrap()))),
            "{value}"
        );
    }
    let (status, value) = send(
        &app,
        &path,
        json!({"space":"work","expected_version":version,"extra":true}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{value}");
    let (status, value) = send(
        &app,
        "/api/pages/no-such-page/space",
        json!({"space":"work","expected_version":1}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{value}");
    let page = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!((page.space, page.version), (None, version));
}
