// SPDX-License-Identifier: Apache-2.0
//! Desktop quick-capture ingest must round-trip through the Memories readers.
mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::json;
use tower::ServiceExt;
use wenlan_types::responses::{IngestResponse, ListMemoriesResponse, MemoryDetailResponse};

async fn request<T: DeserializeOwned>(
    router: &common::AppRouter,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> T {
    let builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    let response = router
        .clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn distinct_quick_captures_are_both_listed_and_readable_by_id() {
    let (router, _tmp, _db) = common::test_app_no_gate_with_page_root().await;
    let captures = [
        (
            "manual_08d389a3-0415-46ae-a198-f0cc1a15947b",
            "First note",
            "Keep **first** capture.",
        ),
        (
            "manual_3645e435-af62-45a1-91b1-f240b1964a40",
            "Second note",
            "Keep `second` capture.",
        ),
    ];
    for (id, title, content) in captures {
        let stored: IngestResponse = request(
            &router,
            "POST",
            "/api/ingest/memory",
            json!({
                "source": "memory", "source_id": id, "title": title, "content": content,
                "tags": ["capture"], "metadata": {"capture_source": "manual"}
            }),
        )
        .await;
        assert_eq!(stored.document_id, id);
        assert!(stored.chunks_created > 0);
    }
    let listed: ListMemoriesResponse =
        request(&router, "POST", "/api/memory/list", json!({"limit": 20})).await;
    assert_eq!(listed.memories.len(), 2);
    for (id, title, content) in captures {
        assert!(listed.memories.iter().any(|memory| memory.source_id == id));
        let detail: MemoryDetailResponse = request(
            &router,
            "GET",
            &format!("/api/memory/{id}/detail"),
            json!(null),
        )
        .await;
        let memory = detail.memory.expect("capture should open from Memories");
        assert_eq!(memory.source_id, id);
        assert_eq!(memory.title, title);
        assert_eq!(memory.content, content);
    }
}

#[tokio::test]
async fn manual_markdown_body_round_trips_exactly_across_chunks_and_edit() {
    let (router, _tmp, db) = common::test_app_no_gate_with_page_root().await;
    let id = "manual_35e9a927-7dd9-4cfb-b065-5e16082ce4fa";
    let body = format!(
        "  \n\n# My note\n\n| Name | Value |\n| --- | --- |\n| Alpha | 1 |\n\n```sh\nprintf alpha | cat | sort\n```\n\n{}\n  \n",
        "Keep the full authored paragraph with its original spacing.\n\n".repeat(140)
    );
    let stored: IngestResponse = request(
        &router,
        "POST",
        "/api/ingest/memory",
        json!({
            "source": "memory", "source_id": id, "title": "My note", "content": body,
            "metadata": {"capture_source": "manual"}
        }),
    )
    .await;
    assert!(
        stored.chunks_created > 1,
        "the embedding path must still chunk long notes"
    );
    let detail: MemoryDetailResponse = request(
        &router,
        "GET",
        &format!("/api/memory/{id}/detail"),
        json!(null),
    )
    .await;
    assert_eq!(detail.memory.unwrap().content, body);
    let listed: ListMemoriesResponse =
        request(&router, "POST", "/api/memory/list", json!({"limit": 10})).await;
    assert_eq!(listed.memories[0].content, body);
    let versions: wenlan_types::responses::VersionChainResponse = request(
        &router,
        "GET",
        &format!("/api/memory/{id}/versions"),
        json!(null),
    )
    .await;
    assert_eq!(versions.versions[0].content, body);

    let edited = format!(
        "\n  {}\n\n```text\na | b | c\n```\n  \n",
        body.replace("Alpha | 1", "Alpha | 2")
    );
    let updated: wenlan_types::responses::SuccessResponse = request(
        &router,
        "PUT",
        &format!("/api/memory/{id}/update"),
        json!({"content": edited, "expected_content": body}),
    )
    .await;
    assert!(updated.ok);
    let detail: MemoryDetailResponse = request(
        &router,
        "GET",
        &format!("/api/memory/{id}/detail"),
        json!(null),
    )
    .await;
    assert_eq!(detail.memory.unwrap().content, edited);
    let listed: ListMemoriesResponse =
        request(&router, "POST", "/api/memory/list", json!({"limit": 10})).await;
    assert_eq!(listed.memories[0].content, edited);

    // The full authored body is also the optimistic concurrency baseline.
    // A stale writer must not change either content or metadata.
    let stale = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/memory/{id}/update"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"content": "Stale replacement must not commit.",
            "expected_content": body, "memory_type": "decision"})
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let unchanged: MemoryDetailResponse = request(
        &router,
        "GET",
        &format!("/api/memory/{id}/detail"),
        json!(null),
    )
    .await;
    let unchanged = unchanged.memory.unwrap();
    assert_eq!(unchanged.content, edited);
    assert_ne!(unchanged.memory_type.as_deref(), Some("decision"));

    db.create_space("work", None, false).await.unwrap();
    db.create_space("personal", None, false).await.unwrap();
    let moved: wenlan_types::responses::SuccessResponse = request(
        &router,
        "PUT",
        &format!("/api/memory/{id}/update"),
        json!({"space": "work"}),
    )
    .await;
    assert!(moved.ok);
    let hidden: ListMemoriesResponse = request(
        &router,
        "POST",
        "/api/memory/list",
        json!({"space": "personal", "limit": 10}),
    )
    .await;
    assert!(
        hidden.memories.is_empty(),
        "authored body must not bypass Space filtering"
    );
    let visible: ListMemoriesResponse = request(
        &router,
        "POST",
        "/api/memory/list",
        json!({"space": "work", "limit": 10}),
    )
    .await;
    assert_eq!(visible.memories[0].content, edited);
}

#[tokio::test]
async fn surviving_legacy_capture_is_visible_after_database_reopen_and_upgrade() {
    use std::sync::Arc;
    use wenlan_core::{db::MemoryDB, events::NoopEmitter, sources::RawDocument};
    let (router, tmp, db) = common::test_app_no_gate_with_page_root().await;
    let id = "manual_1700000000";
    let body = "A surviving legacy capture with **Markdown**.";
    db.upsert_documents(vec![RawDocument {
        source: "manual".into(),
        source_id: id.into(),
        title: "Legacy note".into(),
        content: body.into(),
        last_modified: 1700000000,
        ..Default::default()
    }])
    .await
    .unwrap();
    drop(router);
    drop(db);
    {
        let raw = libsql::Builder::new_local(tmp.path().join("origin_memory.db"))
            .build()
            .await
            .unwrap();
        let conn = raw.connect().unwrap();
        conn.execute_batch(
            "ALTER TABLE memories DROP COLUMN authored_content; PRAGMA user_version=143;",
        )
        .await
        .unwrap();
    }
    let db = Arc::new(
        MemoryDB::new(tmp.path(), Arc::new(NoopEmitter))
            .await
            .unwrap(),
    );
    assert!(tmp.path().join("pre_migration_144_backup.db").is_file());
    let state = wenlan_server::state::ServerState {
        db: Some(db),
        ..Default::default()
    }
    .with_page_root(tmp.path().join("pages"));
    state.maintenance_coordinator.finish_recovery();
    let router = common::build_router(Arc::new(tokio::sync::RwLock::new(state)));
    let listed: ListMemoriesResponse =
        request(&router, "POST", "/api/memory/list", json!({"limit": 10})).await;
    assert_eq!(listed.memories.len(), 1);
    assert_eq!(listed.memories[0].source_id, id);
    let detail: MemoryDetailResponse = request(
        &router,
        "GET",
        &format!("/api/memory/{id}/detail"),
        json!(null),
    )
    .await;
    assert_eq!(detail.memory.unwrap().content, body);
}
