// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::db::tests::test_db;
use wenlan_types::pages::PageCitation;
fn scope() -> ReadScope {
    ReadScope::Space("Research".into())
}
fn citation() -> PageCitation {
    serde_json::from_value(serde_json::json!({"occurrence":1,"marker":1,"source_kind":"memory","locator":"paper","score":1.0,"status":"verified","scope":"sentence"})).unwrap()
}
async fn seed(db: &MemoryDB) {
    db.create_space("Research", None, false).await.unwrap();
    let conn = db.conn.lock().await;
    conn.execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type,source_agent,content_hash) VALUES('paper','file','paper','Old source title','Actual source text','Research',0,1,'text','zotero','import1')",()).await.unwrap();
    conn.execute("INSERT INTO pages(id,title,content,source_memory_ids,version,status,created_at,last_compiled,last_modified,space,workspace,citations,incarnation) VALUES('page','Saved title','Claim [1]','[\"paper\"]',1,'active','2026-01-01','2026-01-01','2026-01-01','Research','Research',?1,'incarnation1')",[serde_json::to_string(&vec![citation()]).unwrap()]).await.unwrap();
    conn.execute("INSERT INTO page_history(page_id,version,title,content,source_memory_ids,edited_by,created_at) SELECT id,version,title,content,source_memory_ids,'test',1 FROM pages WHERE id='page'",()).await.unwrap();
}
async fn publish(db: &MemoryDB, title: &str, hash: &str) -> Result<(), WenlanError> {
    let space = db.get_space("Research").await.unwrap().unwrap();
    db.publish_export_bibliography(PublishExportBibliography {
        space_id: space.id,
        source_id: "paper".into(),
        import_content_hash: hash.into(),
        title: title.into(),
        bibtex: format!("@article{{paper, title={{{title}}}, url={{https://example.org/old}}}}"),
        url: Some("https://example.org/old".into()),
    })
    .await
}
#[tokio::test]
async fn page_export_old_body_references_and_bibliography_are_immutable() {
    let (db, _tmp) = test_db().await;
    seed(&db).await;
    publish(&db, "Old bibliography", "import1").await.unwrap();
    let old = db.capture_page_export("page", &scope()).await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute("UPDATE memories SET title='New title',content='New source text',content_hash='import2' WHERE source_id='paper'",()).await.unwrap();
        conn.execute(
            "UPDATE pages SET content='New body [1]',version=2 WHERE id='page'",
            (),
        )
        .await
        .unwrap();
    }
    publish(&db, "New bibliography", "import2").await.unwrap();
    let new = db.capture_page_export("page", &scope()).await.unwrap();
    assert_ne!(old.id, new.id);
    assert_eq!(new.version, 2);
    let frozen = db
        .page_export_bundle("page", &old.id, &scope())
        .await
        .unwrap();
    assert_eq!(frozen.content, "Claim [1]");
    assert_eq!(frozen.sources[0].title, "Old bibliography");
    assert!(frozen.bibliography[0]
        .bibtex
        .as_deref()
        .unwrap()
        .contains("Old bibliography"));
    assert_eq!(old.digest, frozen.digest);
    assert!(frozen.captured_at > 1_000_000_000_000);
    drop(db);
    let reopened = libsql::Builder::new_local(_tmp.path().join("origin_memory.db"))
        .build()
        .await
        .unwrap();
    let conn = reopened.connect().unwrap();
    let key = format!("export_v1:bundle:{}:{}", hash("page\0incarnation1"), old.id);
    let durable: PageExportBundle = load(&conn, &key).await.unwrap().unwrap();
    assert_eq!(durable.digest, old.digest);
}
#[tokio::test]
async fn page_export_citation_only_revision_and_duplicate_capture() {
    let (db, _tmp) = test_db().await;
    seed(&db).await;
    let first = db.capture_page_export("page", &scope()).await.unwrap();
    let same = db.capture_page_export("page", &scope()).await.unwrap();
    assert_eq!(first.id, same.id);
    let mut c = citation();
    c.status = "unevaluated".into();
    db.set_page_citations("page", Some(&serde_json::to_string(&vec![c]).unwrap()))
        .await
        .unwrap();
    assert_eq!(
        db.page_export_versions("page", &scope())
            .await
            .unwrap()
            .versions
            .len(),
        2,
        "real citation setter journals without an export dialog"
    );
    let second = db.capture_page_export("page", &scope()).await.unwrap();
    assert_eq!(first.version, second.version);
    assert_ne!(first.id, second.id);
    assert!(second.export_revision > first.export_revision);
    assert_eq!(
        db.page_export_versions("page", &scope())
            .await
            .unwrap()
            .versions
            .len(),
        2
    );
}
#[tokio::test]
async fn page_export_snapshot_rolls_back_with_page_transaction() {
    let (db, _tmp) = test_db().await;
    seed(&db).await;
    {
        let conn = db.conn.lock().await;
        conn.execute("BEGIN", ()).await.unwrap();
        conn.execute(
            "UPDATE pages SET content='Not committed [1]',version=2 WHERE id='page'",
            (),
        )
        .await
        .unwrap();
        MemoryDB::append_page_history(&conn, "page", "test", 2)
            .await
            .unwrap();
        conn.execute("ROLLBACK", ()).await.unwrap();
    }
    let list = db.page_export_versions("page", &scope()).await.unwrap();
    assert!(list.versions.iter().all(|v| v.bundle_id.is_none()));
    assert_eq!(db.get_page("page").await.unwrap().unwrap().version, 1);
}
#[tokio::test]
async fn page_export_missing_and_mismatched_bibliography_are_explicit() {
    let (db, _tmp) = test_db().await;
    seed(&db).await;
    assert!(publish(&db, "Wrong generation", "old").await.is_err());
    let missing = db.capture_page_export("page", &scope()).await.unwrap();
    assert_eq!(
        missing.bibliography[0].availability,
        BibliographyAvailability::Missing
    );
    assert!(missing.bibliography[0].bibtex.is_none());
    publish(&db, "Correct", "import1").await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET content_hash='import2' WHERE source_id='paper'",
            (),
        )
        .await
        .unwrap();
    }
    let mismatch = db.capture_page_export("page", &scope()).await.unwrap();
    assert_eq!(
        mismatch.bibliography[0].availability,
        BibliographyAvailability::GenerationMismatch
    );
    assert!(db
        .page_export_bundle("page", &missing.id, &scope())
        .await
        .unwrap()
        .bibliography[0]
        .bibtex
        .is_none());
}
#[tokio::test]
async fn page_export_removed_source_does_not_rewrite_retained_document() {
    let (db, _tmp) = test_db().await;
    seed(&db).await;
    publish(&db, "Retained", "import1").await.unwrap();
    let saved = db.capture_page_export("page", &scope()).await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute("DELETE FROM memories WHERE source_id='paper'", ())
            .await
            .unwrap();
    }
    let frozen = db
        .page_export_bundle("page", &saved.id, &scope())
        .await
        .unwrap();
    assert_eq!(saved.digest, frozen.digest);
    assert!(frozen.bibliography[0]
        .bibtex
        .as_ref()
        .unwrap()
        .contains("Retained"));
}
async fn pending(db: &MemoryDB, id: &str, body: &str) {
    let structured=serde_json::json!({"revision_kind":"page_write","target_kind":"page","revises_page":"page","page_version":1,"source_revision":0,"page_incarnation":"incarnation1","citation_body_digest":hash(body),"citations":[citation()],"source_memory_ids":["paper"]}).to_string();
    let conn = db.conn.lock().await;
    conn.execute("INSERT INTO memories(id,source,source_id,title,content,source_text,space,chunk_index,last_modified,chunk_type,pending_revision,supersedes,structured_fields) VALUES(?1,'memory',?1,'Pending',?2,?2,'Research',0,1,'text',1,'page',?3)",libsql::params![id,body,structured]).await.unwrap();
}
#[tokio::test]
async fn page_export_pending_is_separate_fenced_and_never_default() {
    let (db, _tmp) = test_db().await;
    seed(&db).await;
    pending(&db, "card", "Pending claim [1]").await;
    let saved = db.capture_page_export("page", &scope()).await.unwrap();
    assert_eq!(saved.kind, PageExportKind::Saved);
    let list = db.page_export_versions("page", &scope()).await.unwrap();
    let draft = list
        .versions
        .iter()
        .find(|v| v.kind == PageExportKind::PendingDraft)
        .unwrap();
    let id = draft.bundle_id.as_ref().unwrap();
    assert_eq!(
        db.page_export_bundle("page", id, &scope())
            .await
            .unwrap()
            .content,
        "Pending claim [1]"
    );
    assert_eq!(
        saved.id,
        db.capture_page_export("page", &scope()).await.unwrap().id
    );
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET content='Edited claim [1]' WHERE source_id='card'",
            (),
        )
        .await
        .unwrap();
    }
    assert!(db.page_export_bundle("page", id, &scope()).await.is_err());
    assert!(db
        .page_export_versions("page", &scope())
        .await
        .unwrap()
        .versions
        .iter()
        .all(|v| v.kind != PageExportKind::PendingDraft));
}
#[tokio::test]
async fn page_export_dismissal_and_adoption_do_not_relabel_pending() {
    let (db, _tmp) = test_db().await;
    seed(&db).await;
    pending(&db, "card", "Pending claim [1]").await;
    db.capture_page_export("page", &scope()).await.unwrap();
    let list = db.page_export_versions("page", &scope()).await.unwrap();
    let draft = list
        .versions
        .iter()
        .find(|v| v.kind == PageExportKind::PendingDraft)
        .unwrap()
        .bundle_id
        .clone()
        .unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute("DELETE FROM memories WHERE source_id='card'", ())
            .await
            .unwrap();
        conn.execute(
            "UPDATE pages SET content='Pending claim [1]',version=2 WHERE id='page'",
            (),
        )
        .await
        .unwrap();
    }
    let adopted = db.capture_page_export("page", &scope()).await.unwrap();
    assert_eq!(adopted.kind, PageExportKind::Saved);
    assert_eq!(adopted.version, 2);
    assert_ne!(adopted.id, draft);
    assert!(db
        .page_export_bundle("page", &draft, &scope())
        .await
        .is_err());
}
#[tokio::test]
async fn page_export_space_rename_move_recreate_and_page_incarnation() {
    let (db, _tmp) = test_db().await;
    seed(&db).await;
    let saved = db.capture_page_export("page", &scope()).await.unwrap();
    db.update_space("Research", "Renamed", None).await.unwrap();
    let renamed = ReadScope::Space("Renamed".into());
    assert!(db
        .page_export_bundle("page", &saved.id, &renamed)
        .await
        .is_ok());
    db.create_space("Other", None, false).await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE pages SET space='Other',workspace='Other' WHERE id='page'",
            (),
        )
        .await
        .unwrap();
    }
    assert!(db
        .page_export_bundle("page", &saved.id, &ReadScope::Global)
        .await
        .is_err());
    {
        let conn = db.conn.lock().await;
        conn.execute("UPDATE pages SET space='Renamed',workspace='Renamed',incarnation='incarnation2' WHERE id='page'",()).await.unwrap();
    }
    assert!(db
        .page_export_bundle("page", &saved.id, &renamed)
        .await
        .is_err());
}
#[tokio::test]
async fn page_export_visibility_loss_and_unknown_scope_fail_closed() {
    let (db, _tmp) = test_db().await;
    seed(&db).await;
    let saved = db.capture_page_export("page", &scope()).await.unwrap();
    assert!(db
        .page_export_bundle("page", &saved.id, &ReadScope::Space("Other".into()))
        .await
        .is_err());
    db.set_app_metadata("truth_cutover_fence", "1:preparing")
        .await
        .unwrap();
    assert!(db
        .page_export_bundle("page", &saved.id, &scope())
        .await
        .is_err());
    db.set_app_metadata("truth_cutover_fence", "2:off")
        .await
        .unwrap();
    db.set_truth_cutover_generation(1).await.unwrap();
    db.set_app_metadata("claim_promoter_enforcement", "1")
        .await
        .unwrap();
    {
        let conn = db.conn.lock().await;
        let tx = conn.transaction().await.unwrap();
        MemoryDB::ensure_claim_identity_tables(&tx).await.unwrap();
        tx.commit().await.unwrap();
        conn.execute("INSERT INTO page_truth_state(page_id,page_version,support_status,human_reviewed,updated_at,evaluated_at) VALUES('page',1,'provisional',0,1,1)",()).await.unwrap();
        conn.execute("INSERT OR REPLACE INTO claim_derivation_markers(page_id,page_version,page_version_digest,extractor_version,inventory_count,created_at) VALUES('page',1,?1,?2,1,0)",libsql::params![crate::provenance::revision_content_digest("Claim [1]"),crate::db::claim_derivation::EXTRACTOR_VERSION]).await.unwrap();
    }
    assert_eq!(
        db.page_visibility(
            &crate::truth_contract::TruthGrant::Automatic,
            &["page".into()]
        )
        .await
        .unwrap()["page"],
        crate::truth_contract::Visibility::Hidden
    );
    assert!(db
        .page_export_bundle("page", &saved.id, &scope())
        .await
        .is_err());
}
#[tokio::test]
async fn page_export_legacy_is_unavailable_and_get_list_has_no_capture() {
    let (db, _tmp) = test_db().await;
    seed(&db).await;
    let before = db.page_export_versions("page", &scope()).await.unwrap();
    assert_eq!(before.versions[0].kind, PageExportKind::LegacyUnavailable);
    assert!(before.versions[0].bundle_id.is_none());
    assert!(!before.versions[0].references_available);
    assert_eq!(
        db.page_export_versions("page", &scope())
            .await
            .unwrap()
            .versions
            .len(),
        1
    );
    let saved = db.capture_page_export("page", &scope()).await.unwrap();
    assert!(db
        .page_export_versions("page", &scope())
        .await
        .unwrap()
        .versions
        .iter()
        .any(|v| v.bundle_id.as_deref() == Some(saved.id.as_str())));
    db.insert_page(
        "fresh",
        "Fresh plain note",
        None,
        "No references",
        None,
        Some("Research"),
        &[],
        "2026-01-01T00:00:00Z",
    )
    .await
    .unwrap();
    // The actual creation funnel has its own history INSERT and must journal v1.
    let fresh = db
        .page_export_versions("fresh", &ReadScope::Global)
        .await
        .unwrap();
    assert!(fresh
        .versions
        .iter()
        .any(|v| v.kind == PageExportKind::Saved && v.bundle_id.is_some()));
}

#[tokio::test]
async fn page_export_archived_saved_versions_retain_existing_export_access() {
    let (db, _tmp) = test_db().await;
    seed(&db).await;
    publish(&db, "Retained bibliography", "import1")
        .await
        .unwrap();
    let saved = db.capture_page_export("page", &scope()).await.unwrap();
    db.archive_page("page").await.unwrap();
    let frozen = db
        .page_export_bundle("page", &saved.id, &scope())
        .await
        .unwrap();
    assert_eq!(frozen.digest, saved.digest);
    let current = db.capture_page_export("page", &scope()).await.unwrap();
    assert_eq!(current.kind, PageExportKind::Saved);
    assert!(current.version > saved.version);
    let versions = db.page_export_versions("page", &scope()).await.unwrap();
    assert!(versions
        .versions
        .iter()
        .any(|v| v.version == current.version && v.bundle_id.is_some()));
    assert!(db
        .page_export_bundle("page", &saved.id, &ReadScope::Space("Other".into()))
        .await
        .is_err());
}

#[tokio::test]
async fn page_export_malformed_source_downgrades_only_new_bundle() {
    for column in ["content", "content_hash"] {
        let (db, _tmp) = test_db().await;
        seed(&db).await;
        publish(&db, "Retained bibliography", "import1")
            .await
            .unwrap();
        let old = db.capture_page_export("page", &scope()).await.unwrap();
        let old_bytes = serde_json::to_vec(&old).unwrap();
        assert_eq!(old.citations.as_ref().unwrap()[0].status, "verified");
        assert!(!old.citations.as_ref().unwrap()[0].historical);
        assert!(!old.sources[0].historical);
        {
            let conn = db.conn.lock().await;
            let sql = match column {
                "content" => "UPDATE memories SET content=x'80' WHERE id='paper'",
                "content_hash" => "UPDATE memories SET content_hash=x'80' WHERE id='paper'",
                _ => unreachable!(),
            };
            conn.execute(sql, ()).await.unwrap();
        }
        let fresh = db.capture_page_export("page", &scope()).await.unwrap();
        assert_ne!(
            fresh.id, old.id,
            "{column}: the trust downgrade is a new export revision"
        );
        assert_eq!(fresh.content, old.content);
        assert_eq!(fresh.version, old.version);
        let citation = &fresh.citations.as_ref().unwrap()[0];
        assert!(
            citation.historical,
            "{column}: invalid bytes cannot remain current"
        );
        assert_eq!(citation.status, "unevaluated");
        assert_eq!(citation.score, 0.0);
        assert!(fresh.sources[0].historical);
        assert_eq!(fresh.sources[0].title, "Retained bibliography");
        assert!(
            fresh.references_available,
            "the retained historical map remains available"
        );
        assert_eq!(fresh.bibliography[0].bibtex, old.bibliography[0].bibtex);
        let frozen = db
            .page_export_bundle("page", &old.id, &scope())
            .await
            .unwrap();
        assert_eq!(serde_json::to_vec(&frozen).unwrap(), old_bytes);
        assert_eq!(
            db.get_page("page").await.unwrap().unwrap().content,
            old.content
        );
    }
}

#[tokio::test]
async fn page_export_non_indexed_references_are_not_newly_historical() {
    let (db, _tmp) = test_db().await;
    seed(&db).await;
    let conn = db.conn.lock().await;
    let page = state_on(&conn, "page").await.unwrap().unwrap();
    for (kind, locator) in [
        ("external_url", "https://example.org/unindexed"),
        ("external_file", "attachment.pdf"),
        ("memory", "page"),
    ] {
        let (source, _) = source_presentation(&conn, &page, locator, kind, None, false, &[])
            .await
            .unwrap();
        assert!(
            !source.historical,
            "{kind}: ordinary non-indexed references are unchanged"
        );
    }
}
