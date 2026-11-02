// SPDX-License-Identifier: Apache-2.0

use super::claim_derivation_test::publish_supported_page_for_rename_test;
use super::tests::test_db;
use crate::{
    export::knowledge::KnowledgeProjectionWrite, post_write::rename_page, read_scope::ReadScope,
};
use std::{path::Path, sync::Arc};

struct Fixture {
    db: Arc<super::MemoryDB>,
    _db_dir: tempfile::TempDir,
    page_root: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let (mut db, db_dir) = test_db().await;
    // Keep the fallback-embedding assertions deterministic: `test_db()` loads
    // the shared model, while this suite explicitly exercises rename without it.
    db.embedder = std::sync::OnceLock::new();
    db.conn
        .lock()
        .await
        .execute_batch(
            "INSERT INTO spaces (id,name,created_at,updated_at)
             VALUES ('space-work','work',1,1);
             INSERT INTO pages
                 (id,title,summary,content,space,source_memory_ids,version,status,embedding,
                  created_at,last_compiled,last_modified,workspace,creation_kind,review_status)
             VALUES
                 ('page-rename','Old title','Stable summary','Keep [[Linked page]] body','work',
                  '[\"memory-a\"]',7,'active',NULL,'2026-01-01T00:00:00Z',
                  '2026-01-02T00:00:00Z','2026-01-03T00:00:00Z','work','distilled','confirmed'),
                 ('page-collision','Other','Other summary','Other body','work','[]',1,'active',
                  NULL,'2026-01-01T00:00:00Z','2026-01-02T00:00:00Z',
                  '2026-01-03T00:00:00Z','work','distilled','confirmed'),
                 ('page-unicode','ÄBC','Unicode summary','Unicode body','work','[]',1,'active',
                  NULL,'2026-01-01T00:00:00Z','2026-01-02T00:00:00Z',
                  '2026-01-03T00:00:00Z','work','distilled','confirmed');
             INSERT INTO edges
                 (edge_id,src_id,src_kind,dst_id,dst_kind,edge_type,lineage,grounded,
                  root_id,space,created_at,payload)
             VALUES ('link-edge','page-rename','page','page-collision','page','links',
                     'assertion',0,NULL,'work',1,'{\"label\":\"Other\"}');
             INSERT INTO edges
                 (edge_id,src_id,src_kind,dst_id,dst_kind,edge_type,lineage,grounded,
                  root_id,space,created_at,payload)
             VALUES ('backlink-edge','page-collision','page','page-rename','page','links',
                     'assertion',0,NULL,'work',1,'{\"label\":\"Old title\"}');",
        )
        .await
        .unwrap();
    db.conn
        .lock()
        .await
        .execute(
            "UPDATE pages SET citations=?1 WHERE id='page-rename'",
            libsql::params![serde_json::json!([{
                "occurrence": 1,
                "marker": 1,
                "source_kind": "memory",
                "locator": "memory-a",
                "score": 1.0,
                "status": "verified",
                "scope": "sentence"
            }])
            .to_string()],
        )
        .await
        .unwrap();
    let original_embedding = super::MemoryDB::vec_to_sql(&vec![0.125; 768]);
    db.conn
        .lock()
        .await
        .execute(
            "UPDATE pages SET embedding=vector32(?1) WHERE id='page-rename'",
            libsql::params![original_embedding],
        )
        .await
        .unwrap();
    let db = Arc::new(db);
    let page_root = tempfile::tempdir().unwrap();
    for id in ["page-rename", "page-collision", "page-unicode"] {
        let page = db.get_page(id).await.unwrap().unwrap();
        KnowledgeProjectionWrite::new(page_root.path().to_path_buf(), &db)
            .write_page(&page)
            .unwrap();
    }
    Fixture {
        db,
        _db_dir: db_dir,
        page_root,
    }
}

fn projected_filename(root: &Path, id: &str) -> String {
    serde_json::from_slice::<serde_json::Value>(
        &std::fs::read(root.join(".wenlan/state.json")).unwrap(),
    )
    .unwrap()["pages"][id]["file"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn page_embedding_bytes(db: &super::MemoryDB) -> Vec<u8> {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query("SELECT embedding FROM pages WHERE id='page-rename'", ())
        .await
        .unwrap();
    rows.next()
        .await
        .unwrap()
        .unwrap()
        .get::<Vec<u8>>(0)
        .unwrap()
}

async fn seed_supported_truth(db: &super::MemoryDB, id: &str, version: i64) {
    {
        let conn = db.conn.lock().await;
        super::MemoryDB::append_page_history(
            &conn,
            id,
            "rename_truth_fixture",
            chrono::Utc::now().timestamp(),
        )
        .await
        .unwrap();
    }
    publish_supported_page_for_rename_test(db, id, version).await;
    db.set_truth_cutover_generation(1).await.unwrap();
    db.set_app_metadata("claim_promoter_enforcement", "1")
        .await
        .unwrap();
}

async fn effective_truth(db: &super::MemoryDB, id: &str) -> (crate::truth_contract::Support, bool) {
    let truth = db
        .page_truth_states(&[id.to_string()])
        .await
        .unwrap()
        .get(id)
        .copied()
        .unwrap();
    (truth.support, truth.human_reviewed)
}

#[tokio::test]
async fn rename_preserves_identity_sources_body_history_and_pinned_projection() {
    let fixture = fixture().await;
    {
        let conn = fixture.db.conn.lock().await;
        super::MemoryDB::append_page_history(&conn, "page-rename", "create", 1)
            .await
            .unwrap();
    }
    let history_before = fixture
        .db
        .list_page_history("page-rename", 10)
        .await
        .unwrap();
    let before = fixture.db.get_page("page-rename").await.unwrap().unwrap();
    let path_before = projected_filename(fixture.page_root.path(), "page-rename");
    let embedding_before = page_embedding_bytes(&fixture.db).await;
    let index_before = std::fs::read(fixture.page_root.path().join("index.md")).unwrap();

    let response = rename_page(
        &fixture.db,
        "page-rename",
        "  New title  ",
        7,
        &ReadScope::Space("work".to_string()),
        Some(fixture.page_root.path()),
    )
    .await
    .unwrap();

    let after = fixture.db.get_page("page-rename").await.unwrap().unwrap();
    assert_eq!(response.id, before.id);
    assert_eq!(response.title, "New title");
    assert_eq!(response.version, 8);
    assert_eq!(after.id, before.id);
    assert_eq!(after.title, "New title");
    assert_eq!(after.content, before.content);
    assert_eq!(after.source_memory_ids, before.source_memory_ids);
    assert_eq!(
        serde_json::to_value(&after.citations).unwrap(),
        serde_json::to_value(&before.citations).unwrap()
    );
    assert_eq!(after.created_at, before.created_at);
    assert_eq!(after.user_edited, before.user_edited);
    assert_eq!(after.version, before.version + 1);
    assert_eq!(page_embedding_bytes(&fixture.db).await, embedding_before);
    assert_eq!(
        projected_filename(fixture.page_root.path(), "page-rename"),
        path_before
    );
    let markdown = std::fs::read_to_string(fixture.page_root.path().join(path_before)).unwrap();
    assert!(markdown.contains("New title"));
    assert!(markdown.contains("[[Linked page]]"));
    let index_after = std::fs::read_to_string(fixture.page_root.path().join("index.md")).unwrap();
    assert!(index_after.contains("New title"));
    assert_ne!(index_after.as_bytes(), index_before.as_slice());
    let conn = fixture.db.conn.lock().await;
    let mut matches = conn
        .query(
            "SELECT COUNT(*) FROM pages_fts
             JOIN pages ON pages.rowid=pages_fts.rowid
             WHERE pages_fts MATCH ?1 AND pages.id=?2",
            libsql::params!["New title", "page-rename"],
        )
        .await
        .unwrap();
    assert_eq!(
        matches
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<i64>(0)
            .unwrap(),
        1
    );
    drop(conn);

    let outbound = fixture
        .db
        .get_page_outbound_links_scoped("page-rename", &ReadScope::Global)
        .await
        .unwrap();
    assert_eq!(outbound.len(), 1);
    assert_eq!(
        outbound[0].target_page_id.as_deref(),
        Some("page-collision")
    );
    assert_eq!(outbound[0].label, "Other");

    let history = fixture
        .db
        .list_page_history("page-rename", 10)
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].version, 8);
    assert_eq!(history[0].title.as_deref(), Some("New title"));
    assert_eq!(history[0].content, before.content);
    assert_eq!(history[0].source_memory_ids, before.source_memory_ids);
    assert_eq!(&history[1..], history_before.as_slice());
    assert_eq!(history[1].title.as_deref(), Some("Old title"));
    let changelog: Vec<wenlan_types::responses::PageChangelogEntry> =
        serde_json::from_str(&after.changelog.unwrap()).unwrap();
    let entry = changelog.last().unwrap();
    assert_eq!(entry.version, 8);
    assert_eq!(entry.edited_by, "title_rename");
    assert_eq!(
        entry.delta_summary.as_deref(),
        Some("Title changed from ‘Old title’ to ‘New title’")
    );
}

#[tokio::test]
async fn rename_titles_remain_in_history_after_the_changelog_is_trimmed() {
    let fixture = fixture().await;
    {
        let conn = fixture.db.conn.lock().await;
        super::MemoryDB::append_page_history(&conn, "page-rename", "create", 1)
            .await
            .unwrap();
    }
    let original = fixture
        .db
        .list_page_history("page-rename", 100)
        .await
        .unwrap();
    for step in 1..=25 {
        rename_page(
            &fixture.db,
            "page-rename",
            &format!("Historical title {step}"),
            6 + step,
            &ReadScope::Global,
            None,
        )
        .await
        .unwrap();
    }
    let history = fixture
        .db
        .list_page_history("page-rename", 100)
        .await
        .unwrap();
    assert_eq!(history.len(), 26);
    assert_eq!(&history[25..], original.as_slice());
    for (index, snapshot) in history[..25].iter().enumerate() {
        let step = 25 - index;
        assert_eq!(snapshot.title, Some(format!("Historical title {step}")));
        assert_eq!(snapshot.version, 7 + step as i64);
        assert_eq!(snapshot.content, original[0].content);
        assert_eq!(snapshot.source_memory_ids, original[0].source_memory_ids);
    }
    let page = fixture.db.get_page("page-rename").await.unwrap().unwrap();
    let changelog: Vec<wenlan_types::responses::PageChangelogEntry> =
        serde_json::from_str(&page.changelog.unwrap()).unwrap();
    assert_eq!(changelog.len(), 20);
    assert_eq!(changelog[0].version, 13);
    assert_eq!(history[24].title.as_deref(), Some("Historical title 1"));
}

#[tokio::test]
async fn rename_keeps_a_page_in_its_pinned_nested_folder() {
    let fixture = fixture().await;
    let flat_path = projected_filename(fixture.page_root.path(), "page-rename");
    crate::export::knowledge::create_knowledge_folder(fixture.page_root.path(), "", "Research")
        .unwrap();
    let nested_path = crate::export::knowledge::move_projected_page(
        fixture.page_root.path(),
        "page-rename",
        &flat_path,
        "Research",
        "rename-folder-setup",
    )
    .unwrap();

    rename_page(
        &fixture.db,
        "page-rename",
        "Nested folder title",
        7,
        &ReadScope::Global,
        Some(fixture.page_root.path()),
    )
    .await
    .unwrap();

    assert_eq!(
        projected_filename(fixture.page_root.path(), "page-rename"),
        nested_path
    );
    let markdown = std::fs::read_to_string(fixture.page_root.path().join(&nested_path)).unwrap();
    assert!(markdown.contains("title: \"Nested folder title\""));
    assert!(!fixture.page_root.path().join(&flat_path).exists());
    let index = std::fs::read_to_string(fixture.page_root.path().join("index.md")).unwrap();
    assert!(index.contains("Nested folder title"));
}

#[tokio::test]
async fn rename_accepts_legacy_projection_state_without_content_hash() {
    let fixture = fixture().await;
    let state_path = fixture.page_root.path().join(".wenlan/state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    state["pages"]["page-rename"]
        .as_object_mut()
        .unwrap()
        .remove("content_sha256");
    std::fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();

    rename_page(
        &fixture.db,
        "page-rename",
        "Legacy state title",
        7,
        &ReadScope::Global,
        Some(fixture.page_root.path()),
    )
    .await
    .unwrap();

    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(state["pages"]["page-rename"]["title"], "Legacy state title");
    assert!(state["pages"]["page-rename"]["content_sha256"].is_string());
}

#[tokio::test]
async fn rename_refuses_unknown_projection_state_fields_without_changing_bytes() {
    for location in [
        "top_level",
        "page_entry",
        "other_page_entry",
        "malformed_other_page_entry",
        "unsafe_other_page_path",
    ] {
        let fixture = fixture().await;
        let state_path = fixture.page_root.path().join(".wenlan/state.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
        if location == "top_level" {
            state["future_state_field"] = serde_json::json!({"keep": true});
        } else {
            let entry_id = if location == "page_entry" {
                "page-rename"
            } else {
                "page-collision"
            };
            if location == "unsafe_other_page_path" {
                state["pages"][entry_id]["file"] = serde_json::json!("../outside.md");
            } else if location == "malformed_other_page_entry" {
                state["pages"][entry_id]["version"] = serde_json::json!("not-an-integer");
            } else {
                state["pages"][entry_id]["future_page_field"] = serde_json::json!("keep");
            }
        }
        std::fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
        let state_before = std::fs::read(&state_path).unwrap();
        let target_path = projected_filename(fixture.page_root.path(), "page-rename");
        let target = fixture.page_root.path().join(&target_path);
        let target_before = std::fs::read(&target).unwrap();

        assert!(rename_page(
            &fixture.db,
            "page-rename",
            "Must not drop unknown fields",
            7,
            &ReadScope::Global,
            Some(fixture.page_root.path()),
        )
        .await
        .is_err());

        let page = fixture.db.get_page("page-rename").await.unwrap().unwrap();
        assert_eq!(page.title, "Old title", "{location}");
        assert_eq!(page.version, 7, "{location}");
        assert_eq!(
            std::fs::read(&state_path).unwrap(),
            state_before,
            "{location}"
        );
        assert_eq!(std::fs::read(&target).unwrap(), target_before, "{location}");
    }
}

#[tokio::test]
async fn rename_keeps_current_human_review_but_requeues_machine_derivation() {
    let fixture = fixture().await;
    seed_supported_truth(&fixture.db, "page-rename", 7).await;
    let body = fixture
        .db
        .get_page("page-rename")
        .await
        .unwrap()
        .unwrap()
        .content;
    let body_digest = crate::provenance::revision_content_digest(&body);
    fixture
        .db
        .conn
        .lock()
        .await
        .execute(
            "UPDATE page_truth_state
                SET human_reviewed=1,reviewed_page_version=7,reviewed_page_digest=?1
              WHERE page_id='page-rename'",
            libsql::params![body_digest.clone()],
        )
        .await
        .unwrap();
    let original_receipt = serde_json::json!({
        "page_id": "page-rename",
        "human_reviewed": true,
        "reviewed_page_version": 7,
        "reviewed_page_digest": body_digest,
        "protocol_version": 1
    })
    .to_string();
    fixture
        .db
        .conn
        .lock()
        .await
        .execute(
            "INSERT INTO presence_receipts
                 (caller_id,operation_id,request_digest,response_json,created_at)
             VALUES ('reviewer','review-op','request-digest',?1,1)",
            libsql::params![original_receipt.clone()],
        )
        .await
        .unwrap();

    let before = effective_truth(&fixture.db, "page-rename").await;
    assert_eq!(before.0, crate::truth_contract::Support::Supported);
    assert!(before.1);
    assert_eq!(
        fixture
            .db
            .page_visibility(
                &crate::truth_contract::TruthGrant::Automatic,
                &["page-rename".to_string()],
            )
            .await
            .unwrap()["page-rename"],
        crate::truth_contract::Visibility::Full
    );

    rename_page(
        &fixture.db,
        "page-rename",
        "New title",
        7,
        &ReadScope::Global,
        None,
    )
    .await
    .unwrap();

    let after = effective_truth(&fixture.db, "page-rename").await;
    assert_eq!(after.0, crate::truth_contract::Support::Unevaluated);
    assert!(after.1);
    let conn = fixture.db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT page_version,support_status,provisional_reason,evaluated_at,
                    reviewed_page_version
               FROM page_truth_state WHERE page_id='page-rename'",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<i64>(0).unwrap(), 7);
    assert_eq!(row.get::<String>(1).unwrap(), "provisional");
    assert!(row.get::<Option<String>>(2).unwrap().is_some());
    assert_eq!(row.get::<Option<i64>>(3).unwrap(), None);
    assert_eq!(row.get::<i64>(4).unwrap(), 8);
    drop(rows);
    let mut markers = conn
        .query(
            "SELECT COUNT(*) FROM claim_derivation_markers
              WHERE page_id='page-rename' AND page_version=8",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        markers
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<i64>(0)
            .unwrap(),
        0
    );
    let mut memberships = conn
        .query(
            "SELECT COUNT(*) FROM page_version_claims
              WHERE page_id='page-rename' AND page_version=8",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        memberships
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<i64>(0)
            .unwrap(),
        0
    );
    let mut jobs = conn
        .query(
            "SELECT status FROM claim_derivation_jobs
              WHERE page_id='page-rename' AND page_version=8",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        jobs.next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(0)
            .unwrap(),
        "pending"
    );
    let mut receipts = conn
        .query(
            "SELECT response_json FROM presence_receipts
              WHERE caller_id='reviewer' AND operation_id='review-op'",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        receipts
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(0)
            .unwrap(),
        original_receipt
    );
    drop(receipts);
}

#[tokio::test]
async fn unsupported_rename_is_refused_without_projecting() {
    let fixture = fixture().await;
    seed_supported_truth(&fixture.db, "page-rename", 7).await;
    fixture
        .db
        .conn
        .lock()
        .await
        .execute(
            "UPDATE page_truth_state SET support_status='provisional',
                    provisional_reason='unsupported fixture',evaluated_at=11,
                    human_reviewed=0,reviewed_page_version=NULL,reviewed_page_digest=NULL
              WHERE page_id='page-rename'",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        effective_truth(&fixture.db, "page-rename").await.0,
        crate::truth_contract::Support::Unsupported
    );
    let empty_projection_root = tempfile::tempdir().unwrap();

    let before = fixture.db.get_page("page-rename").await.unwrap().unwrap();
    let error = rename_page(
        &fixture.db,
        "page-rename",
        "Hidden title",
        7,
        &ReadScope::Global,
        Some(empty_projection_root.path()),
    )
    .await
    .expect_err("unsupported pages require review before rename");
    assert!(matches!(
        error,
        crate::error::WenlanError::Conflict(message)
            if message == "page_review_required: review this page before renaming"
    ));

    let after_page = fixture.db.get_page("page-rename").await.unwrap().unwrap();
    assert_eq!(after_page.title, before.title);
    assert_eq!(after_page.version, before.version);
    let after = effective_truth(&fixture.db, "page-rename").await;
    assert_eq!(after.0, crate::truth_contract::Support::Unsupported);
    assert!(!after.1);
    assert_eq!(
        fixture
            .db
            .page_visibility(
                &crate::truth_contract::TruthGrant::Automatic,
                &["page-rename".to_string()],
            )
            .await
            .unwrap()["page-rename"],
        crate::truth_contract::Visibility::Hidden
    );
    assert_eq!(
        std::fs::read_dir(empty_projection_root.path())
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn stale_derivation_and_review_receipts_are_not_carried_as_current() {
    for corrupt in ["digest", "extractor", "review_version", "review_digest"] {
        let fixture = fixture().await;
        seed_supported_truth(&fixture.db, "page-rename", 7).await;
        let body = fixture
            .db
            .get_page("page-rename")
            .await
            .unwrap()
            .unwrap()
            .content;
        let body_digest = crate::provenance::revision_content_digest(&body);
        {
            let conn = fixture.db.conn.lock().await;
            let (reviewed_version, reviewed_digest) = if corrupt == "review_digest" {
                (7, "stale")
            } else {
                (6, body_digest.as_str())
            };
            conn.execute(
                "UPDATE page_truth_state SET human_reviewed=1,
                        reviewed_page_version=?1,reviewed_page_digest=?2
                  WHERE page_id='page-rename'",
                libsql::params![reviewed_version, reviewed_digest],
            )
            .await
            .unwrap();
            match corrupt {
                "digest" => {
                    conn.execute(
                        "UPDATE claim_derivation_markers SET page_version_digest='stale'
                          WHERE page_id='page-rename' AND page_version=7",
                        (),
                    )
                    .await
                    .unwrap();
                }
                "extractor" => {
                    conn.execute(
                        "UPDATE claim_derivation_markers SET extractor_version=999
                          WHERE page_id='page-rename' AND page_version=7",
                        (),
                    )
                    .await
                    .unwrap();
                }
                _ => {}
            }
        }
        rename_page(
            &fixture.db,
            "page-rename",
            "Fresh metadata title",
            7,
            &ReadScope::Global,
            None,
        )
        .await
        .unwrap();
        let truth = effective_truth(&fixture.db, "page-rename").await;
        assert!(!truth.1, "case {corrupt}");
        assert_eq!(truth.0, crate::truth_contract::Support::Unevaluated);
        let conn = fixture.db.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM claim_derivation_markers
                  WHERE page_id='page-rename' AND page_version=8",
                (),
            )
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );
        let mut jobs = conn
            .query(
                "SELECT status FROM claim_derivation_jobs
                  WHERE page_id='page-rename' AND page_version=8",
                (),
            )
            .await
            .unwrap();
        assert_eq!(
            jobs.next()
                .await
                .unwrap()
                .unwrap()
                .get::<String>(0)
                .unwrap(),
            "pending"
        );
    }
}

#[tokio::test]
async fn rename_still_refuses_during_preparing_truth_cutover() {
    let fixture = fixture().await;
    fixture
        .db
        .set_app_metadata("truth_cutover_fence", "1:preparing")
        .await
        .unwrap();
    assert!(rename_page(
        &fixture.db,
        "page-rename",
        "No rename during cutover",
        7,
        &ReadScope::Global,
        None,
    )
    .await
    .is_err());
    let page = fixture.db.get_page("page-rename").await.unwrap().unwrap();
    assert_eq!(page.version, 7);
    assert_eq!(page.title, "Old title");
}

#[tokio::test]
async fn rename_noop_validation_collision_and_stale_version_are_fenced() {
    let fixture = fixture().await;
    let original = fixture.db.get_page("page-rename").await.unwrap().unwrap();
    let no_op = rename_page(
        &fixture.db,
        "page-rename",
        "Old title",
        7,
        &ReadScope::Global,
        None,
    )
    .await
    .unwrap();
    assert_eq!(no_op.version, 7);
    assert!(rename_page(
        &fixture.db,
        "page-rename",
        " \n ",
        7,
        &ReadScope::Global,
        None,
    )
    .await
    .is_err());
    assert!(rename_page(
        &fixture.db,
        "page-rename",
        "Valid\nTitle",
        7,
        &ReadScope::Global,
        None,
    )
    .await
    .is_err());
    assert!(rename_page(
        &fixture.db,
        "page-rename",
        "Other",
        7,
        &ReadScope::Global,
        None,
    )
    .await
    .is_err());
    assert!(rename_page(
        &fixture.db,
        "page-rename",
        " äbc ",
        7,
        &ReadScope::Global,
        None,
    )
    .await
    .is_err());
    assert!(rename_page(
        &fixture.db,
        "page-rename",
        &"x".repeat(501),
        7,
        &ReadScope::Global,
        None,
    )
    .await
    .is_err());
    assert!(rename_page(
        &fixture.db,
        "page-rename",
        "Renamed",
        6,
        &ReadScope::Global,
        None,
    )
    .await
    .is_err());
    let after_failures = fixture.db.get_page("page-rename").await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&after_failures).unwrap(),
        serde_json::to_value(&original).unwrap()
    );

    let absent_projection_root = fixture.page_root.path().join("not-created");
    let db_only = rename_page(
        &fixture.db,
        "page-rename",
        "New title",
        7,
        &ReadScope::Global,
        Some(&absent_projection_root),
    )
    .await
    .unwrap();
    assert_eq!(db_only.version, 8);
    assert!(!absent_projection_root.exists());
}

#[tokio::test]
async fn rename_scope_is_checked_and_concurrent_cas_has_one_winner() {
    let fixture = fixture().await;
    assert!(rename_page(
        &fixture.db,
        "page-rename",
        "Should not change",
        7,
        &ReadScope::Space("elsewhere".to_string()),
        None,
    )
    .await
    .is_err());

    let a = {
        let db = Arc::clone(&fixture.db);
        tokio::spawn(async move {
            rename_page(&db, "page-rename", "Winner A", 7, &ReadScope::Global, None).await
        })
    };
    let b = {
        let db = Arc::clone(&fixture.db);
        tokio::spawn(async move {
            rename_page(&db, "page-rename", "Winner B", 7, &ReadScope::Global, None).await
        })
    };
    let results = [a.await.unwrap(), b.await.unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    assert_eq!(
        fixture
            .db
            .get_page("page-rename")
            .await
            .unwrap()
            .unwrap()
            .version,
        8
    );
}

#[tokio::test]
async fn generated_entity_and_source_pages_cannot_be_renamed() {
    let fixture = fixture().await;
    fixture
        .db
        .conn
        .lock()
        .await
        .execute_batch(
            "INSERT INTO pages
                 (id,title,summary,content,space,source_memory_ids,version,status,embedding,
                  created_at,last_compiled,last_modified,workspace,kind,creation_kind,review_status)
             VALUES
                 ('page-source','Generated source','Summary','Body','work','[]',1,'active',NULL,
                  '2026-01-01T00:00:00Z','2026-01-02T00:00:00Z','2026-01-03T00:00:00Z',
                  'work','source','source','confirmed'),
                 ('page-entity','Generated entity','Summary','Body','work','[]',1,'active',NULL,
                  '2026-01-01T00:00:00Z','2026-01-02T00:00:00Z','2026-01-03T00:00:00Z',
                  'work','entity','entity','confirmed'),
                 ('page-imported','Legacy imported','Summary','Body','work','[]',1,'active',NULL,
                  '2026-01-01T00:00:00Z','2026-01-02T00:00:00Z','2026-01-03T00:00:00Z',
                  'work','concept','imported','confirmed');",
        )
        .await
        .unwrap();

    for id in ["page-source", "page-entity", "page-imported"] {
        assert!(rename_page(
            &fixture.db,
            id,
            "Renamed generated page",
            1,
            &ReadScope::Global,
            None,
        )
        .await
        .is_err());
    }
    assert!(rename_page(
        &fixture.db,
        "page-rename",
        "Generated entity",
        7,
        &ReadScope::Global,
        None,
    )
    .await
    .is_err());
    let conn = fixture.db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT id,title,version FROM pages
             WHERE id IN ('page-source','page-entity','page-imported') ORDER BY id",
            (),
        )
        .await
        .unwrap();
    let entity = rows.next().await.unwrap().unwrap();
    assert_eq!(entity.get::<String>(0).unwrap(), "page-entity");
    assert_eq!(entity.get::<String>(1).unwrap(), "Generated entity");
    assert_eq!(entity.get::<i64>(2).unwrap(), 1);
    let imported = rows.next().await.unwrap().unwrap();
    assert_eq!(imported.get::<String>(0).unwrap(), "page-imported");
    assert_eq!(imported.get::<String>(1).unwrap(), "Legacy imported");
    assert_eq!(imported.get::<i64>(2).unwrap(), 1);
    let source = rows.next().await.unwrap().unwrap();
    assert_eq!(source.get::<String>(0).unwrap(), "page-source");
    assert_eq!(source.get::<String>(1).unwrap(), "Generated source");
    assert_eq!(source.get::<i64>(2).unwrap(), 1);
}

#[tokio::test]
async fn renaming_to_and_from_overview_rederives_page_kind() {
    let fixture = fixture().await;
    let overview_title = crate::synthesis::overview::OVERVIEW_PAGE_TITLE;
    rename_page(
        &fixture.db,
        "page-rename",
        overview_title,
        7,
        &ReadScope::Global,
        None,
    )
    .await
    .unwrap();
    let conn = fixture.db.conn.lock().await;
    let mut rows = conn
        .query("SELECT title,kind FROM pages WHERE id='page-rename'", ())
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<String>(0).unwrap(), overview_title);
    assert_eq!(row.get::<String>(1).unwrap(), "overview");
    drop(rows);
    drop(conn);

    rename_page(
        &fixture.db,
        "page-rename",
        "Ordinary page again",
        8,
        &ReadScope::Global,
        None,
    )
    .await
    .unwrap();
    let conn = fixture.db.conn.lock().await;
    let mut rows = conn
        .query("SELECT title,kind FROM pages WHERE id='page-rename'", ())
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<String>(0).unwrap(), "Ordinary page again");
    assert_eq!(row.get::<String>(1).unwrap(), "concept");
}

#[tokio::test]
async fn renamed_target_identity_survives_source_reindex_and_old_title_reuse() {
    let fixture = fixture().await;
    {
        let conn = fixture.db.conn.lock().await;
        // Locked for review, so the rename leaves its `[[Old title]]` as
        // written (the link update waits as a revision card) and the link
        // keeps its identity by page id.
        conn.execute(
            "UPDATE pages SET content='Read [[Old title]]',review_required=1
              WHERE id='page-collision'",
            (),
        )
        .await
        .unwrap();
        super::MemoryDB::append_page_history(&conn, "page-collision", "create", 1)
            .await
            .unwrap();
    }
    let source_history = fixture
        .db
        .list_page_history("page-collision", 10)
        .await
        .unwrap();
    rename_page(
        &fixture.db,
        "page-rename",
        "New title",
        7,
        &ReadScope::Global,
        None,
    )
    .await
    .unwrap();
    fixture
        .db
        .conn
        .lock()
        .await
        .execute(
            "INSERT INTO pages
                 (id,title,summary,content,space,source_memory_ids,version,status,embedding,
                  created_at,last_compiled,last_modified,workspace,creation_kind,review_status)
             VALUES ('page-reused-title','Old title','Summary','Body','work','[]',1,'active',
                     NULL,'2026-01-01T00:00:00Z','2026-01-02T00:00:00Z',
                     '2026-01-03T00:00:00Z','work','distilled','confirmed')",
            (),
        )
        .await
        .unwrap();

    fixture
        .db
        .replace_page_links(
            "page-collision",
            &[crate::synthesis::wikilinks::Wikilink {
                label: "Old title".to_string(),
                target_page_id: Some("page-reused-title".to_string()),
            }],
        )
        .await
        .unwrap();

    let outbound = fixture
        .db
        .get_page_outbound_links_scoped("page-collision", &ReadScope::Global)
        .await
        .unwrap();
    assert_eq!(outbound.len(), 1);
    assert_eq!(outbound[0].label, "Old title");
    assert_eq!(outbound[0].target_page_id.as_deref(), Some("page-rename"));
    assert_eq!(
        fixture
            .db
            .list_page_history("page-collision", 10)
            .await
            .unwrap(),
        source_history
    );
    assert_eq!(source_history[0].content, "Read [[Old title]]");
}

#[tokio::test]
async fn rename_refuses_to_overwrite_unsynced_projection_edits() {
    let fixture = fixture().await;
    let filename = projected_filename(fixture.page_root.path(), "page-rename");
    let target = fixture.page_root.path().join(&filename);
    let state_path = fixture.page_root.path().join(".wenlan/state.json");
    let index_path = fixture.page_root.path().join("index.md");
    let before_target = std::fs::read(&target).unwrap();
    let before_state = std::fs::read(&state_path).unwrap();
    let before_index = std::fs::read(&index_path).unwrap();
    let mut edited = String::from_utf8(before_target.clone()).unwrap();
    edited.push_str("\nunsynced user edit\n");
    std::fs::write(&target, edited.as_bytes()).unwrap();

    assert!(rename_page(
        &fixture.db,
        "page-rename",
        "New title",
        7,
        &ReadScope::Global,
        Some(fixture.page_root.path()),
    )
    .await
    .is_err());

    let after = fixture.db.get_page("page-rename").await.unwrap().unwrap();
    assert_eq!(after.title, "Old title");
    assert_eq!(after.version, 7);
    assert_eq!(std::fs::read(&target).unwrap(), edited.as_bytes());
    assert_eq!(std::fs::read(state_path).unwrap(), before_state);
    assert_eq!(std::fs::read(index_path).unwrap(), before_index);
}

#[tokio::test]
async fn projection_failure_after_write_restores_projection_and_database() {
    let fixture = fixture().await;
    let flat_path = projected_filename(fixture.page_root.path(), "page-rename");
    crate::export::knowledge::create_knowledge_folder(fixture.page_root.path(), "", "Research")
        .unwrap();
    crate::export::knowledge::move_projected_page(
        fixture.page_root.path(),
        "page-rename",
        &flat_path,
        "Research",
        "rename-rollback-folder-setup",
    )
    .unwrap();
    seed_supported_truth(&fixture.db, "page-rename", 7).await;
    let content = fixture
        .db
        .get_page("page-rename")
        .await
        .unwrap()
        .unwrap()
        .content;
    let digest = crate::provenance::revision_content_digest(&content);
    fixture
        .db
        .conn
        .lock()
        .await
        .execute(
            "UPDATE page_truth_state SET human_reviewed=1,
                    reviewed_page_version=7,reviewed_page_digest=?1
              WHERE page_id='page-rename'",
            libsql::params![digest],
        )
        .await
        .unwrap();
    let truth_before = effective_truth(&fixture.db, "page-rename").await;
    let history_before = fixture
        .db
        .list_page_history("page-rename", 10)
        .await
        .unwrap();
    let filename = projected_filename(fixture.page_root.path(), "page-rename");
    let target = fixture.page_root.path().join(&filename);
    let state_path = fixture.page_root.path().join(".wenlan/state.json");
    let index_path = fixture.page_root.path().join("index.md");
    let before_target = std::fs::read(&target).unwrap();
    let before_state = std::fs::read(&state_path).unwrap();
    let before_index = std::fs::read(&index_path).unwrap();

    let result = super::page_rename::with_failure_after_projection_write(rename_page(
        &fixture.db,
        "page-rename",
        "New title",
        7,
        &ReadScope::Global,
        Some(fixture.page_root.path()),
    ))
    .await;

    assert!(result.is_err());
    let after = fixture.db.get_page("page-rename").await.unwrap().unwrap();
    assert_eq!(after.title, "Old title");
    assert_eq!(after.version, 7);
    assert_eq!(
        fixture
            .db
            .list_page_history("page-rename", 10)
            .await
            .unwrap(),
        history_before
    );
    assert_eq!(
        effective_truth(&fixture.db, "page-rename").await,
        truth_before
    );
    assert_eq!(std::fs::read(target).unwrap(), before_target);
    assert_eq!(std::fs::read(state_path).unwrap(), before_state);
    assert_eq!(std::fs::read(index_path).unwrap(), before_index);
    let conn = fixture.db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM claim_derivation_markers
              WHERE page_id='page-rename' AND page_version=8",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
    let mut jobs = conn
        .query(
            "SELECT COUNT(*) FROM claim_derivation_jobs
              WHERE page_id='page-rename' AND page_version=8",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        jobs.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
}
