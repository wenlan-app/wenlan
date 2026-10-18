// SPDX-License-Identifier: Apache-2.0

use super::case_runner::{assert_wave_2_records_executed_keys, WAVE_2_RECORDS};
use super::fixture::ScopeFixture;
use axum::body::{to_bytes, Body};
use axum::http::{Method as HttpMethod, Response, StatusCode};
use serde_json::Value;
use wenlan_server::sensitive_read_routes::Method;

async fn body_bytes(response: Response<Body>) -> (StatusCode, Vec<u8>) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, bytes.to_vec())
}

async fn json_body(response: Response<Body>) -> (StatusCode, Value) {
    let (status, bytes) = body_bytes(response).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

pub async fn unknown_selectors_are_rejected() {
    let fixture = ScopeFixture::new().await;
    let probes = [
        (
            "/api/memory/record/enrichment-status",
            "/api/memory/{source_id}/enrichment-status",
        ),
        ("/api/memory/record/revisions", "/api/memory/{id}/revisions"),
        ("/api/indexed-files", "/api/indexed-files"),
        ("/api/chunks/record", "/api/chunks/{source_id}"),
        (
            "/api/suggest-tags?source=memory&source_id=record",
            "/api/suggest-tags",
        ),
        ("/api/memory/record/detail", "/api/memory/{id}/detail"),
        ("/api/memory/by-ids?ids=record", "/api/memory/by-ids"),
        ("/api/memory/record/versions", "/api/memory/{id}/versions"),
        ("/api/decisions", "/api/decisions"),
        ("/api/brief/history", "/api/brief/history"),
        ("/api/brief/history/1", "/api/brief/history/{version}"),
        (
            "/api/memory/pending-revisions",
            "/api/memory/pending-revisions",
        ),
        (
            "/api/memory/pending-revision/record",
            "/api/memory/pending-revision/{source_id}",
        ),
    ];

    let mut executed = Vec::new();
    for (uri, key) in probes {
        let response = fixture
            .send(HttpMethod::GET, uri, None, Some("missing-space"))
            .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY, "{uri}");
        executed.push((Method::Get, key));
    }
    assert_wave_2_records_executed_keys(executed);
}

pub async fn direct_routes_do_not_disclose_mismatched_ids() {
    let fixture = ScopeFixture::new().await;
    fixture
        .seed_record(
            "memory",
            "personal-record",
            Some("personal"),
            "fact",
            20,
            None,
            false,
        )
        .await;
    fixture
        .seed_record(
            "memory",
            "personal-revision",
            Some("personal"),
            "fact",
            21,
            Some("personal-record"),
            true,
        )
        .await;
    fixture
        .db
        .record_enrichment_step("personal-record", "extract", "ok", None)
        .await
        .unwrap();
    fixture
        .seed_record(
            "file",
            "detail-collision",
            Some("work"),
            "fact",
            22,
            None,
            false,
        )
        .await;
    fixture
        .db
        .store_raw_import_memory(
            "detail-collision",
            "personal memory hidden behind a work file",
            Some("detail collision"),
            None,
            0,
        )
        .await
        .unwrap();

    let pairs = [
        (
            "/api/memory/personal-record/enrichment-status",
            "/api/memory/missing/enrichment-status",
        ),
        (
            "/api/memory/personal-record/revisions",
            "/api/memory/missing/revisions",
        ),
        ("/api/chunks/personal-record", "/api/chunks/missing"),
        (
            "/api/suggest-tags?source=memory&source_id=personal-record",
            "/api/suggest-tags?source=memory&source_id=missing",
        ),
        (
            "/api/memory/personal-record/detail",
            "/api/memory/missing/detail",
        ),
        (
            "/api/memory/detail-collision/detail",
            "/api/memory/missing/detail",
        ),
        (
            "/api/memory/personal-record/versions",
            "/api/memory/missing/versions",
        ),
        (
            "/api/memory/pending-revision/personal-record",
            "/api/memory/pending-revision/missing",
        ),
    ];

    for (mismatch_uri, missing_uri) in pairs {
        let mismatch = body_bytes(
            fixture
                .send(HttpMethod::GET, mismatch_uri, None, Some("work"))
                .await,
        )
        .await;
        let missing = body_bytes(
            fixture
                .send(HttpMethod::GET, missing_uri, None, Some("work"))
                .await,
        )
        .await;
        assert_eq!(mismatch.0, StatusCode::NOT_FOUND, "{mismatch_uri}");
        assert_eq!(missing.0, StatusCode::NOT_FOUND, "{missing_uri}");
        assert_eq!(mismatch.1, missing.1, "{mismatch_uri}");
    }
}

pub async fn collections_filter_before_materialization() {
    let fixture = ScopeFixture::new().await;
    fixture
        .seed_record(
            "memory",
            "work-decision",
            Some("work"),
            "decision",
            10,
            None,
            false,
        )
        .await;
    fixture
        .seed_record(
            "memory",
            "personal-decision",
            Some("personal"),
            "decision",
            20,
            None,
            false,
        )
        .await;
    fixture
        .seed_record(
            "memory",
            "work-decision-two",
            Some("work"),
            "decision",
            25,
            None,
            false,
        )
        .await;
    fixture
        .seed_record(
            "memory",
            "uncategorized-decision",
            None,
            "decision",
            15,
            None,
            false,
        )
        .await;
    fixture
        .seed_record(
            "memory",
            "work-revision",
            Some("work"),
            "fact",
            30,
            Some("work-decision"),
            true,
        )
        .await;
    fixture
        .seed_record(
            "memory",
            "personal-revision",
            Some("personal"),
            "fact",
            40,
            Some("personal-decision"),
            true,
        )
        .await;
    fixture
        .seed_record(
            "memory",
            "dangling-revision",
            Some("work"),
            "fact",
            50,
            Some("dangling-target"),
            true,
        )
        .await;

    for uri in [
        "/api/indexed-files",
        "/api/memory/by-ids?ids=work-decision-two,personal-decision,work-decision,missing",
        "/api/decisions?limit=1",
        "/api/memory/pending-revisions?limit=1",
    ] {
        let (status, body) =
            json_body(fixture.send(HttpMethod::GET, uri, None, Some("work")).await).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        let serialized = body.to_string();
        assert!(serialized.contains("work-"), "{uri}: {serialized}");
        assert!(!serialized.contains("personal-"), "{uri}: {serialized}");
        if uri.starts_with("/api/memory/pending-revisions") {
            assert!(!serialized.contains("dangling-"), "{uri}: {serialized}");
        }
    }

    let (status, body) = json_body(
        fixture
            .send(
                HttpMethod::GET,
                "/api/memory/by-ids?ids=work-decision-two,personal-decision,work-decision,missing",
                None,
                Some("work"),
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids = body["memories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["source_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["work-decision-two", "work-decision"]);

    let (status, global) = json_body(
        fixture
            .send(HttpMethod::GET, "/api/decisions", None, None)
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{global}");
    let serialized = global.to_string();
    assert!(serialized.contains("work-decision"), "{serialized}");
    assert!(serialized.contains("personal-decision"), "{serialized}");
    assert!(
        serialized.contains("uncategorized-decision"),
        "{serialized}"
    );

    let (status, uncategorized) = json_body(
        fixture
            .send(
                HttpMethod::GET,
                "/api/decisions",
                None,
                Some("uncategorized"),
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{uncategorized}");
    let serialized = uncategorized.to_string();
    assert!(
        serialized.contains("uncategorized-decision"),
        "{serialized}"
    );
    assert!(!serialized.contains("work-decision"), "{serialized}");
    assert!(!serialized.contains("personal-decision"), "{serialized}");

    let (status, global_pending) = json_body(
        fixture
            .send(HttpMethod::GET, "/api/memory/pending-revisions", None, None)
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{global_pending}");
    assert!(!global_pending.to_string().contains("dangling-"));

    let dangling = body_bytes(
        fixture
            .send(
                HttpMethod::GET,
                "/api/memory/pending-revision/dangling-target",
                None,
                None,
            )
            .await,
    )
    .await;
    let missing = body_bytes(
        fixture
            .send(
                HttpMethod::GET,
                "/api/memory/pending-revision/missing",
                None,
                None,
            )
            .await,
    )
    .await;
    assert_eq!(dangling.0, StatusCode::NOT_FOUND);
    assert_eq!(dangling, missing);
}

pub async fn history_and_chunk_source_priority_are_scoped() {
    let fixture = ScopeFixture::new().await;
    fixture
        .seed_record(
            "memory",
            "personal-old",
            Some("personal"),
            "fact",
            10,
            None,
            false,
        )
        .await;
    fixture
        .seed_record(
            "memory",
            "work-current",
            Some("work"),
            "fact",
            20,
            Some("personal-old"),
            false,
        )
        .await;
    fixture
        .seed_record("file", "collision", Some("work"), "fact", 40, None, false)
        .await;
    fixture
        .db
        .store_raw_import_memory(
            "collision",
            "record canary null collision",
            Some("null-collision"),
            None,
            0,
        )
        .await
        .unwrap();

    for uri in [
        "/api/memory/work-current/revisions",
        "/api/memory/work-current/versions",
    ] {
        let (status, body) =
            json_body(fixture.send(HttpMethod::GET, uri, None, Some("work")).await).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        let serialized = body.to_string();
        assert!(serialized.contains("work-current"), "{uri}: {serialized}");
        assert!(!serialized.contains("personal-old"), "{uri}: {serialized}");
    }

    let (status, chunks) = json_body(
        fixture
            .send(HttpMethod::GET, "/api/chunks/collision", None, Some("work"))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{chunks}");
    let serialized = chunks.to_string();
    assert!(serialized.contains("collision"));
    assert!(!serialized.contains("null-collision"), "{serialized}");
}

pub fn registry_matches_catalog() {
    super::case_runner::assert_wave_2_records_catalog_contract();
    assert_eq!(WAVE_2_RECORDS.len(), 13);
}
