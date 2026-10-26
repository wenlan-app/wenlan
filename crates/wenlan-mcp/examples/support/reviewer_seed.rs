// SPDX-License-Identifier: Apache-2.0
//! Canonical synthetic library shared by local protocol checks and operator seeding.
use serde_json::json;
use std::path::Path;
use wenlan_core::{
    db::MemoryDB,
    post_write::{page_write, PageWrite},
};
use wenlan_types::{requests::CreateConceptRequest, RawDocument};

pub const SPACE: &str = "atlas-review";
pub const OTHER: &str = "private-sentinel";
pub const AGENT: &str = "reviewer-real-backend";

pub async fn seed(db: &MemoryDB, pages: &Path) {
    for space in [SPACE, OTHER] {
        db.create_space(space, None, false).await.unwrap();
    }
    db.apply_brief_update(
        &serde_json::from_value(json!({
            "space": SPACE, "caller_id": AGENT, "operation_id": "reviewer-seed",
            "summary": {"text": "Atlas uses signed requests.", "expected_version": 0},
            "mutations": [
                {"kind": "add", "text": "Use signed requests", "state": "active"},
                {"kind": "add", "text": "Document offline fallback", "state": "backlog"}
            ]
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    for (id, title, content, space) in [
        (
            "mem_atlas-auth",
            "Atlas authentication decision",
            "Atlas authentication decision: use signed requests to authenticate relay traffic.",
            SPACE,
        ),
        (
            "mem_atlas-unavailable",
            "Historical evidence",
            "Historical project evidence, moved outside the shared Space.",
            SPACE,
        ),
        (
            "mem_private-sentinel",
            "Atlas authentication decision",
            "Atlas authentication decision signed requests UNAUTHORIZED_SENTINEL",
            OTHER,
        ),
    ] {
        db.upsert_documents(vec![RawDocument {
            source: "memory".into(),
            source_id: id.into(),
            title: title.into(),
            content: content.into(),
            last_modified: 1_788_912_000,
            confirmed: Some(true),
            stability: Some("confirmed".into()),
            memory_type: Some("decision".into()),
            space: Some(space.into()),
            ..Default::default()
        }])
        .await
        .unwrap();
    }
    for (id, title, content, source, space) in [
        (
            "page_atlas-auth",
            "Atlas authentication",
            "Atlas uses signed requests to authenticate relay traffic.",
            "mem_atlas-auth",
            SPACE,
        ),
        (
            "page_atlas-unavailable",
            "Historical evidence index",
            "Historical evidence is no longer available in this shared library.",
            "mem_atlas-unavailable",
            SPACE,
        ),
        (
            "page_private-sentinel",
            "Private authentication",
            "UNAUTHORIZED_SENTINEL private evidence.",
            "mem_private-sentinel",
            OTHER,
        ),
    ] {
        // Use the canonical page-write pipeline, including real source links.
        let written = page_write(
            db,
            PageWrite::Create {
                page_id: Some(id),
                req: CreateConceptRequest {
                    title: title.into(),
                    content: content.into(),
                    summary: None,
                    entity_id: None,
                    space: Some(space.to_string()).into(),
                    source_memory_ids: vec![source.into()],
                    creation_kind: Some("authored".into()),
                    workspace: Some(space.into()),
                },
                agent: AGENT,
                knowledge_path: Some(pages),
                page_min_cluster_size: 1,
                page_match_threshold: 0.95,
                citations_json: None,
                basis_marks: Vec::new(),
                basis_space_guard: None,
                skip_protected_title_match: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(written.id, id);
        db.set_page_review_status(id, "confirmed").await.unwrap();
    }
    db.update_memory_space("mem_atlas-unavailable", OTHER)
        .await
        .unwrap();
    assert_eq!(
        db.get_page_sources("page_atlas-unavailable").await.unwrap()[0].memory_source_id,
        "mem_atlas-unavailable"
    );
    assert_eq!(
        db.get_memory_space("mem_atlas-unavailable")
            .await
            .unwrap()
            .as_deref(),
        Some(OTHER)
    );
}
