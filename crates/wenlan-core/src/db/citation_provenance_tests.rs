// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::db::tests::test_db;

const BODY: &str =
    "The shared claim cites two studies [1][2].\n\nAn independent claim cites one study [3].";
fn citation(occurrence: u32, locator: &str) -> PageCitation {
    PageCitation {
        historical: false,
        occurrence,
        marker: occurrence,
        source_kind: "memory".into(),
        locator: locator.into(),
        score: 0.8,
        status: "verified".into(),
        scope: "sentence".into(),
        cited_version: Some(1),
        cited_digest: Some(format!("old-{locator}")),
        disproven: false,
        pdf_target: (occurrence == 1).then(|| wenlan_types::pages::PdfCitationTarget {
            attachment_key: "ATTACH01".into(),
            pdf_digest: "a".repeat(64),
            page: 2,
            quote: "original quote".into(),
        }),
    }
}
fn references() -> Vec<PageCitation> {
    vec![
        citation(1, "paper-a"),
        citation(2, "paper-b"),
        citation(3, "paper-b"),
    ]
}

#[test]
fn source_change_downgrades_union_claim_and_keeps_independent_evidence_and_targets() {
    let original = references();
    let effective = effective_map(
        BODY,
        &original,
        &BTreeSet::from(["paper-a".into()]),
        &BTreeSet::new(),
    );
    assert_eq!(
        effective
            .iter()
            .map(|c| c.status.as_str())
            .collect::<Vec<_>>(),
        vec!["unevaluated", "unevaluated", "verified"]
    );
    assert_eq!(effective[0].pdf_target, original[0].pdf_target);
    assert!(
        !effective[0].historical,
        "active content changes retain the dependency"
    );
    for (old, new) in original.iter().zip(&effective) {
        assert_eq!(old.locator, new.locator);
        assert_eq!(old.cited_version, new.cited_version);
        assert_eq!(old.cited_digest, new.cited_digest);
        assert!(!new.disproven);
    }
    let mut broken = original;
    broken[0].occurrence = 99;
    assert!(effective_map(
        BODY,
        &broken,
        &BTreeSet::from(["paper-a".into()]),
        &BTreeSet::new()
    )
    .iter()
    .all(|c| c.status == "unevaluated"));
}

async fn seed(db: &MemoryDB) -> String {
    let space = db.create_space("Research", None, false).await.unwrap();
    {
        let conn = db.conn.lock().await;
        for source in ["paper-a", "paper-b"] {
            conn.execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type) VALUES(?1,'file',?1,?1,'The shared claim cites two studies. An independent claim cites one study.','Research',0,1,'text')",[source]).await.unwrap();
        }
    }
    db.insert_page_with_kind(
        "review",
        "Human review",
        None,
        BODY,
        None,
        Some("Research"),
        &["paper-a", "paper-b"],
        &chrono::Utc::now().to_rfc3339(),
        "authored",
        "confirmed",
        Some("Research"),
        Some(&serde_json::to_string(&references()).unwrap()),
    )
    .await
    .unwrap();
    space.id
}
fn request(revision: u64, state: SourceLifecycleState) -> SetSourceLifecycleRequest {
    SetSourceLifecycleRequest {
        space: "Research".into(),
        source: "file".into(),
        source_id: "paper-a".into(),
        expected_revision: revision,
        state,
        reason: "Publisher withdrawal reported by researcher".into(),
    }
}

#[tokio::test]
async fn lifecycle_cas_keeps_prose_and_references_across_repeated_null_invalidation() {
    let (db, _temp) = test_db().await;
    let id = seed(&db).await;
    let result = db
        .set_source_lifecycle(&id, request(0, SourceLifecycleState::Retracted), "human")
        .await
        .unwrap();
    assert_eq!(result.record.revision, 1);
    assert_eq!(result.impacts.len(), 1);
    assert!(db
        .get_pages_missing_citations(10)
        .await
        .unwrap()
        .contains(&"review".into()));
    let page = db.get_page("review").await.unwrap().unwrap();
    assert_eq!(page.content, BODY);
    {
        let conn = db.conn.lock().await;
        let mut view = page.clone();
        apply_on(&conn, &mut view).await.unwrap();
        assert_eq!(view.citations.len(), 3);
        assert_eq!(view.citations[0].status, "unevaluated");
        assert_eq!(view.citations[1].status, "unevaluated");
        assert_eq!(view.citations[2].status, "verified");
        assert!(view.citations[0].pdf_target.is_some());
    }
    assert!(db
        .set_source_lifecycle(&id, request(0, SourceLifecycleState::Active), "human")
        .await
        .is_err());
    let result = db
        .set_source_lifecycle(&id, request(1, SourceLifecycleState::Active), "human")
        .await
        .unwrap();
    assert_eq!(result.history.len(), 2);
    let conn = db.conn.lock().await;
    let (stored, saved) = preserved_on(&conn, "review").await.unwrap().unwrap();
    assert_eq!(saved.citations[0].status, "verified");
    assert_eq!(saved.impacts.len(), 2);
    assert_eq!(stored.content, BODY);
}

#[tokio::test]
async fn fallback_requires_sql_null_exact_body_space_version_and_incarnation() {
    let (db, _temp) = test_db().await;
    let id = seed(&db).await;
    db.set_source_lifecycle(&id, request(0, SourceLifecycleState::Excluded), "human")
        .await
        .unwrap();
    let conn = db.conn.lock().await;
    for statement in [
        "UPDATE pages SET citations='[]' WHERE id='review'",
        "UPDATE pages SET citations=NULL,content='changed' WHERE id='review'",
        "UPDATE pages SET content=?1,version=version+1 WHERE id='review'",
        "UPDATE pages SET version=1,space='Other' WHERE id='review'",
        "UPDATE pages SET space='Research',incarnation='new-row' WHERE id='review'",
    ] {
        if statement.contains("?1") {
            conn.execute(statement, [BODY]).await.unwrap();
        } else {
            conn.execute(statement, ()).await.unwrap();
        }
        assert!(
            preserved_on(&conn, "review").await.unwrap().is_none(),
            "{statement}"
        );
    }
}

#[tokio::test]
async fn lifecycle_is_exact_scoped_and_retracted_text_cannot_reverify() {
    let (db, _temp) = test_db().await;
    let id = seed(&db).await;
    let other = db.create_space("Other", None, false).await.unwrap();
    assert!(db
        .source_lifecycle(&other.id, "file", "paper-a")
        .await
        .is_err());
    assert!(db
        .source_lifecycle(&id, "webpage", "paper-a")
        .await
        .is_err());
    db.set_source_lifecycle(&id, request(0, SourceLifecycleState::Retracted), "human")
        .await
        .unwrap();
    let sources = db
        .citation_sources(&["paper-a".into()], &ReadScope::Space("Research".into()))
        .await
        .unwrap();
    let snapshots = sources
        .into_iter()
        .flatten()
        .map(|s| s.snapshot)
        .collect::<Vec<_>>();
    assert!(db.ensure_citation_sources_active(&snapshots).await.is_err());
    // The original remains readable; a status change is never physical deletion.
    assert_eq!(
        db.citation_sources(&["paper-a".into()], &ReadScope::Space("Research".into()))
            .await
            .unwrap()
            .len(),
        1
    );
    let sentence = crate::authorship::sentence_units(BODY)[0].hash.clone();
    assert!(crate::sentences::recheck_sentence(&db, "review", &sentence)
        .await
        .is_err());
    assert!(db
        .resolve_source_reference(&other.id, "paper-a")
        .await
        .is_err());
    assert_eq!(
        db.resolve_source_reference(&id, "paper-a")
            .await
            .unwrap()
            .source_id,
        "paper-a"
    );
}

#[tokio::test]
async fn detached_historical_references_do_not_reattach_active_edges_or_change_targets() {
    let (db, _temp) = test_db().await;
    let _id = seed(&db).await;
    db.replace_page_sources("review", &["paper-b"], "researcher excluded this input")
        .await
        .unwrap();
    let page = db.get_page("review").await.unwrap().unwrap();
    let fence = db.try_get_page_fence("review").await.unwrap().unwrap();
    assert_eq!(
        db.restore_citation_provenance("review", page.version, &fence)
            .await
            .unwrap(),
        Some(true)
    );
    let after = db.get_page("review").await.unwrap().unwrap();
    assert_eq!(after.content, BODY);
    assert_eq!(after.version, page.version);
    assert!(after.citations[0].historical);
    assert_eq!(after.citations[0].locator, "paper-a");
    assert_eq!(after.citations[0].pdf_target, references()[0].pdf_target);
    assert!(!after.citations[1].historical);
    assert_eq!(after.citations[1].status, "unevaluated");
    assert_eq!(after.citations[2].status, "verified");
    let evidence = db.get_page_evidence("review").await.unwrap();
    assert!(!evidence
        .iter()
        .any(|e| e.locator.as_deref() == Some("paper-a")));
    assert!(evidence
        .iter()
        .any(|e| e.locator.as_deref() == Some("paper-b")));
    // Retained bytes/identity remain readable without source reattachment.
    assert!(db.resolve_source_reference(&_id, "paper-a").await.is_ok());
    assert!(!db
        .get_pages_missing_citations(10)
        .await
        .unwrap()
        .contains(&"review".into()));
    assert_eq!(after.stale_reason.as_deref(), Some("source_updated"));
}

#[tokio::test]
async fn active_updates_stay_dependencies_and_new_input_cannot_retarget_old_markers() {
    let (db, _temp) = test_db().await;
    let _id = seed(&db).await;
    {
        let conn = db.conn.lock().await;
        MemoryDB::mark_pages_depending_on_memory_source(&conn, "file", "paper-a")
            .await
            .unwrap();
    }
    let page = db.get_page("review").await.unwrap().unwrap();
    let fence = db.try_get_page_fence("review").await.unwrap().unwrap();
    db.restore_citation_provenance("review", page.version, &fence)
        .await
        .unwrap();
    let page = db.get_page("review").await.unwrap().unwrap();
    assert!(!page.citations[0].historical);
    assert_eq!(page.citations[0].status, "unevaluated");
    assert!(db
        .get_page_evidence("review")
        .await
        .unwrap()
        .iter()
        .any(|e| e.locator.as_deref() == Some("paper-a")));
    // Attach a third source, then let the NULL backfill restoration run. A
    // generic annotation pass must not turn old [1] into today's first input.
    {
        let conn = db.conn.lock().await;
        conn.execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type) VALUES('paper-c','file','paper-c','Third','different evidence','Research',0,1,'text')",()).await.unwrap();
    }
    db.link_page_source("review", "paper-c", "new paper")
        .await
        .unwrap();
    let page = db.get_page("review").await.unwrap().unwrap();
    let fence = db.try_get_page_fence("review").await.unwrap().unwrap();
    db.restore_citation_provenance("review", page.version, &fence)
        .await
        .unwrap();
    let page = db.get_page("review").await.unwrap().unwrap();
    assert_eq!(page.citations[0].locator, "paper-a");
    assert_eq!(page.citations[2].locator, "paper-b");
    {
        let conn = db.conn.lock().await;
        MemoryDB::mark_pages_depending_on_memory_source(&conn, "file", "paper-a")
            .await
            .unwrap();
    }
    assert!(db
        .get_pages_missing_citations(10)
        .await
        .unwrap()
        .contains(&"review".into()));
}

#[tokio::test]
async fn lifecycle_revision_fences_writes_and_client_false_flag_cannot_verify_withdrawn_source() {
    let (db, _temp) = test_db().await;
    let id = seed(&db).await;
    let source = db
        .citation_sources(&["paper-a".into()], &ReadScope::Space("Research".into()))
        .await
        .unwrap()
        .pop()
        .unwrap()
        .unwrap();
    db.set_source_lifecycle(&id, request(0, SourceLifecycleState::Retracted), "human")
        .await
        .unwrap();
    {
        let conn = db.conn.lock().await;
        assert!(
            super::super::citation_sources::guard_on(&conn, &[source.snapshot])
                .await
                .is_err()
        );
    }
    // Even a legacy/client map claiming current support is downgraded on read.
    db.set_page_citations_for_test("review", &serde_json::to_string(&references()).unwrap())
        .await
        .unwrap();
    let page = db.get_page("review").await.unwrap().unwrap();
    assert!(page.citations[0].historical);
    assert_eq!(page.citations[0].status, "unevaluated");
    assert_eq!(page.citations[1].status, "unevaluated");
    assert_eq!(page.citations[2].status, "verified");
}

#[tokio::test]
async fn space_rename_keeps_identity_but_actual_move_refuses_historical_fallback() {
    let (db, _temp) = test_db().await;
    let id = seed(&db).await;
    db.set_source_lifecycle(&id, request(0, SourceLifecycleState::Excluded), "human")
        .await
        .unwrap();
    db.update_space("Research", "Renamed", None).await.unwrap();
    let page = db.get_page("review").await.unwrap().unwrap();
    assert_eq!(page.space.as_deref(), Some("Renamed"));
    assert_eq!(page.citations.len(), 3);
    assert!(page.citations[0].historical);
    assert_eq!(
        db.source_lifecycle(&id, "file", "paper-a")
            .await
            .unwrap()
            .record
            .state,
        SourceLifecycleState::Excluded
    );
    let other = db.create_space("Other", None, false).await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE pages SET space='Other',workspace='Other' WHERE id='review'",
            (),
        )
        .await
        .unwrap();
        assert!(preserved_on(&conn, "review").await.unwrap().is_none());
    }
    assert!(db
        .resolve_source_reference(&other.id, "paper-a")
        .await
        .is_err());
}

#[tokio::test]
async fn physical_source_removal_retains_occurrences_without_restoring_deleted_edges() {
    let (db, _temp) = test_db().await;
    let id = seed(&db).await;
    db.delete_by_source_id("file", "paper-a").await.unwrap();
    let page = db.get_page("review").await.unwrap().unwrap();
    let fence = db.try_get_page_fence("review").await.unwrap().unwrap();
    assert_eq!(
        db.restore_citation_provenance("review", page.version, &fence)
            .await
            .unwrap(),
        Some(true)
    );
    let page = db.get_page("review").await.unwrap().unwrap();
    assert_eq!(page.content, BODY);
    assert!(page.citations[0].historical);
    assert_eq!(page.citations[0].pdf_target, references()[0].pdf_target);
    assert_eq!(page.citations[0].status, "unevaluated");
    assert_eq!(page.citations[1].status, "unevaluated");
    assert_eq!(page.citations[2].status, "verified");
    assert!(!db
        .get_page_evidence("review")
        .await
        .unwrap()
        .iter()
        .any(|e| e.locator.as_deref() == Some("paper-a")));
    assert!(db.resolve_source_reference(&id, "paper-a").await.is_err());
    assert_eq!(page.stale_reason.as_deref(), Some("source_removed"));
}

async fn run_for(db: &MemoryDB, space_id: &str) -> wenlan_types::research::OrganizeRun {
    db.create_research_run(space_id,serde_json::from_value(serde_json::json!({"space":"Research","sources":[{"source":"file","source_id":"paper-a","title":""},{"source":"file","source_id":"paper-b","title":""}]})).unwrap()).await.unwrap()
}
async fn progress(
    db: &MemoryDB,
    space_id: &str,
    run: &wenlan_types::research::OrganizeRun,
    index: usize,
    coverage: &str,
) -> Result<wenlan_types::research::OrganizeRun, WenlanError> {
    let item = &run.items[index];
    let revision = if coverage == "complete" {
        db.read_organize_source(
            space_id,
            &run.id,
            wenlan_types::research::ReadOrganizeSourceRequest {
                space: "Research".into(),
                source: item.source_ref.source.clone(),
                source_id: item.source_ref.source_id.clone(),
                offset: 0,
                limit: 16000,
            },
        )
        .await?
        .run_revision
    } else {
        run.revision
    };
    db.update_research_run(space_id,&run.id,serde_json::from_value(serde_json::json!({"space":"Research","expected_revision":revision,"source":"file","source_id":item.source_ref.source_id,"coverage":coverage,"content_digest":item.content_digest,"error":if coverage=="unavailable"{Some("withdrawn input")}else{None}})).unwrap()).await
}
fn proposal(
    run: &wenlan_types::research::OrganizeRun,
) -> wenlan_types::research::ProposeSourceClassificationsRequest {
    serde_json::from_value(serde_json::json!({"space":"Research","run_id":run.id,"changes":[{"source":"file","source_id":"paper-a","source_content_digest":run.items[0].content_digest,"expected_decision_revision":run.items[0].decision_revision,"add_tags":["retracted"],"remove_tags":[],"reason":"explicit source status"}]})).unwrap()
}
#[tokio::test]
async fn research_lifecycle_fence_keeps_raw_digest_and_allows_fresh_inactive_classification() {
    let (db, _temp) = test_db().await;
    let id = seed(&db).await;
    let old = run_for(&db, &id).await;
    let old = progress(&db, &id, &old, 0, "complete").await.unwrap();
    let pending = db
        .propose_research_classifications(&id, proposal(&old))
        .await
        .unwrap();
    db.set_source_lifecycle(&id, request(0, SourceLifecycleState::Retracted), "human")
        .await
        .unwrap();
    assert!(db
        .accept_research_classifications(
            &pending.id,
            "human",
            None,
            &crate::read_scope::ReadScope::Global
        )
        .await
        .is_err());
    assert!(db
        .propose_research_classifications(&id, proposal(&old))
        .await
        .is_err());
    // The frozen read generation is immutable, even when bytes have not changed.
    assert!(progress(&db, &id, &old, 0, "complete").await.is_err());
    let fresh = run_for(&db, &id).await;
    assert_eq!(fresh.items[0].content_digest, old.items[0].content_digest);
    assert_eq!(fresh.items[0].lifecycle_revision, 1);
    let fresh = progress(&db, &id, &fresh, 0, "complete").await.unwrap();
    let new = db
        .propose_research_classifications(&id, proposal(&fresh))
        .await
        .unwrap();
    assert_ne!(new.id, pending.id);
    db.accept_research_classifications(
        &new.id,
        "human",
        None,
        &crate::read_scope::ReadScope::Global,
    )
    .await
    .unwrap();
    assert!(db
        .prepare_research_write(
            &fresh.id,
            &ReadScope::Space("Research".into()),
            None,
            &["paper-a".into()]
        )
        .await
        .is_err());
    // A withdrawn input can be reported unavailable while another complete,
    // active input remains usable for a partial organizing result.
    let partial = run_for(&db, &id).await;
    let partial = progress(&db, &id, &partial, 0, "unavailable")
        .await
        .unwrap();
    let partial = progress(&db, &id, &partial, 1, "complete").await.unwrap();
    assert!(db
        .prepare_research_write(
            &partial.id,
            &ReadScope::Space("Research".into()),
            None,
            &["paper-b".into()]
        )
        .await
        .is_ok());
    let inventory = db
        .list_indexed_files_scoped(&ReadScope::Space("Research".into()))
        .await
        .unwrap();
    let source = inventory.iter().find(|f| f.source_id == "paper-a").unwrap();
    assert_eq!(
        source.source_lifecycle.state,
        SourceLifecycleState::Retracted
    );
    assert_eq!(source.source_lifecycle.revision, 1);
}

async fn stored_citations(db: &MemoryDB, id: &str) -> Option<Vec<PageCitation>> {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query("SELECT citations FROM pages WHERE id=?1", [id])
        .await
        .unwrap();
    let json: Option<String> = rows.next().await.unwrap().unwrap().get(0).unwrap();
    json.map(|json| serde_json::from_str(&json).unwrap())
}
#[tokio::test]
async fn canonical_insert_and_setters_cannot_store_verified_inactive_union_claims() {
    let (db, _temp) = test_db().await;
    let id = seed(&db).await;
    db.set_source_lifecycle(&id, request(0, SourceLifecycleState::Retracted), "human")
        .await
        .unwrap();
    let mut original = references();
    original[0].cited_version = None;
    original[0].cited_digest = None;
    let json = serde_json::to_string(&original).unwrap();
    db.insert_page_with_kind(
        "automatic-new",
        "New automatic page",
        None,
        BODY,
        None,
        Some("Research"),
        &["paper-a", "paper-b"],
        &chrono::Utc::now().to_rfc3339(),
        "distilled",
        "confirmed",
        Some("Research"),
        Some(&json),
    )
    .await
    .unwrap();
    let raw = stored_citations(&db, "automatic-new").await.unwrap();
    assert_eq!(
        raw.iter().map(|c| c.status.as_str()).collect::<Vec<_>>(),
        vec!["unevaluated", "unevaluated", "verified"]
    );
    assert!(raw[0].historical);
    assert_eq!(raw[0].cited_digest, None);
    assert_eq!(raw[0].cited_version, None);
    assert_eq!(raw[0].pdf_target, original[0].pdf_target);
    let changelog = r#"[{"version":1,"citations_summary":"3 verified"}]"#;
    db.set_page_citations_with_changelog("review", Some(&json), changelog)
        .await
        .unwrap();
    assert_eq!(
        stored_citations(&db, "review").await.unwrap()[0].status,
        "unevaluated"
    );
    let history = db.get_page_changelog("review").await.unwrap();
    assert!(history.contains("1 verified"));
    assert!(history.contains("lifecycle"));
    db.set_page_citations("review", None).await.unwrap();
    assert!(
        stored_citations(&db, "review").await.is_none(),
        "SQL NULL remains the work queue invariant"
    );
    db.set_page_citations("review", Some(&json)).await.unwrap();
    assert_eq!(
        stored_citations(&db, "review").await.unwrap()[1].status,
        "unevaluated"
    );
}
#[tokio::test]
async fn canonical_content_update_groups_the_incoming_body_and_keeps_manual_prose() {
    let (db, _temp) = test_db().await;
    let id = seed(&db).await;
    db.set_source_lifecycle(&id, request(0, SourceLifecycleState::Excluded), "human")
        .await
        .unwrap();
    let body = "First independent claim [1].\n\nSecond claim cites both [2][3].";
    let json = serde_json::to_string(&references()).unwrap();
    let revision = db.get_page_source_revision("review").await.unwrap();
    assert!(db
        .try_update_page_content_with_changelog_at_source_revision(
            "review",
            body,
            &["paper-a", "paper-b"],
            "manual_edit",
            false,
            r#"[{"version":2,"citations_summary":"3 verified"}]"#,
            Some(&json),
            revision,
            None,
            None
        )
        .await
        .unwrap());
    let raw = stored_citations(&db, "review").await.unwrap();
    assert_eq!(
        raw.iter().map(|c| c.status.as_str()).collect::<Vec<_>>(),
        vec!["unevaluated", "verified", "verified"]
    );
    assert_eq!(db.get_page("review").await.unwrap().unwrap().content, body);
    assert!(db
        .get_page_changelog("review")
        .await
        .unwrap()
        .contains("2 verified"));
}

#[tokio::test]
async fn indexed_external_kinds_are_guarded_but_unknown_external_references_are_retained() {
    for (kind, source) in [("external_file", "file"), ("external_url", "webpage")] {
        let (db, _temp) = test_db().await;
        let id = seed(&db).await;
        db.conn
            .lock()
            .await
            .execute("UPDATE memories SET source=?1 WHERE id='paper-a'", [source])
            .await
            .unwrap();
        let mut change = request(0, SourceLifecycleState::Retracted);
        change.source = source.into();
        db.set_source_lifecycle(&id, change, "human").await.unwrap();
        let mut refs = references();
        refs[0].source_kind = kind.into();
        db.set_page_citations("review", Some(&serde_json::to_string(&refs).unwrap()))
            .await
            .unwrap();
        let raw = stored_citations(&db, "review").await.unwrap();
        assert!(raw[0].historical);
        assert_eq!(raw[0].status, "unevaluated");
        assert_eq!(raw[1].status, "unevaluated");
        db.delete_by_source_id(source, "paper-a").await.unwrap();
        // A stale automatic producer cannot restore its deleted external-kind
        // input as verified through either a setter or a new page insert.
        db.set_page_citations("review", Some(&serde_json::to_string(&refs).unwrap()))
            .await
            .unwrap();
        assert!(stored_citations(&db, "review").await.unwrap()[0].historical);
        let mut external = citation(4, "https://outside.example/background");
        external.source_kind = "external_url".into();
        refs.push(external);
        let body = format!(
            "{BODY}

Unindexed background [4]."
        );
        db.insert_page_with_kind(
            "stale-external-new",
            "Historical background",
            None,
            &body,
            None,
            Some("Research"),
            &["paper-a", "paper-b"],
            &chrono::Utc::now().to_rfc3339(),
            "distilled",
            "confirmed",
            Some("Research"),
            Some(&serde_json::to_string(&refs).unwrap()),
        )
        .await
        .unwrap();
        let raw = stored_citations(&db, "stale-external-new").await.unwrap();
        assert!(raw[0].historical);
        assert_eq!(raw[0].status, "unevaluated");
        assert_eq!(raw[1].status, "unevaluated");
        assert_eq!(raw[2].status, "verified");
        assert_eq!(raw[3].status, "verified");
        assert!(!raw[3].historical);
    }
}

#[tokio::test]
async fn active_source_get_previews_claim_groups_without_writes_or_scope_leaks() {
    let (db, _temp) = test_db().await;
    let id = seed(&db).await;
    let other = db.create_space("Other", None, false).await.unwrap();
    let before = {
        let conn = db.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT citations,source_revision FROM pages WHERE id='review'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        (
            row.get::<Option<String>>(0).unwrap(),
            row.get::<i64>(1).unwrap(),
        )
    };
    let response = db.source_lifecycle(&id, "file", "paper-a").await.unwrap();
    assert_eq!(response.record.revision, 0);
    assert!(response.history.is_empty());
    assert!(response.impacts.is_empty());
    assert_eq!(response.preview_impacts.len(), 1);
    let preview = &response.preview_impacts[0];
    assert_eq!(preview.page_id, "review");
    assert_eq!(preview.occurrences, vec![1, 2]);
    assert!(preview.reason.starts_with("Prospective"));
    assert!(db
        .source_lifecycle(&other.id, "file", "paper-a")
        .await
        .is_err());
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT citations,source_revision FROM pages WHERE id='review'",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let after = (
        row.get::<Option<String>>(0).unwrap(),
        row.get::<i64>(1).unwrap(),
    );
    assert_eq!(before, after);
    drop(rows);
    assert!(
        load::<CitationProvenance>(&conn, &key("page", &"review").unwrap())
            .await
            .unwrap()
            .is_none()
    );
    assert!(lifecycle_history(&conn, &id, "file", "paper-a")
        .await
        .unwrap()
        .records
        .is_empty());
}

#[tokio::test]
async fn source_identity_rename_keeps_inactive_history_and_human_tag_tombstones() {
    for state in [
        SourceLifecycleState::Excluded,
        SourceLifecycleState::Retracted,
    ] {
        let (db, _temp) = test_db().await;
        let id = seed(&db).await;
        let other = db.create_space("Other", None, false).await.unwrap();
        db.set_document_tags("file", "paper-a", vec!["keep".into(), "removed".into()])
            .await
            .unwrap();
        db.patch_document_tag(
            "file",
            "paper-a",
            "removed",
            "remove",
            &ReadScope::Space("Research".into()),
        )
        .await
        .unwrap();
        let decisions = db
            .research_classification_decisions(&id, "file", "paper-a")
            .await
            .unwrap();
        let previous = db
            .set_source_lifecycle(&id, request(0, state), "human")
            .await
            .unwrap();
        let foreign_key = key("source", &(&other.id, "file", "paper-a")).unwrap();
        let foreign = SourceHistory {
            records: vec![SourceLifecycleRecord {
                state: SourceLifecycleState::Excluded,
                reason: "Foreign historical decision".into(),
                ..default_record(&other.id, "file", "paper-a")
            }],
        };
        {
            let conn = db.conn.lock().await;
            save(&conn, &foreign_key, &foreign).await.unwrap();
        }
        let run = run_for(&db, &id).await;
        db.rebind_source_id("file", "paper-a", "renamed-file")
            .await
            .unwrap();
        let after = db
            .source_lifecycle(&id, "file", "renamed-file")
            .await
            .unwrap();
        assert_eq!(after.record.state, state);
        assert_eq!(after.record.reason, previous.record.reason);
        assert_eq!(after.record.source_id, "renamed-file");
        assert_eq!(after.record.revision, previous.record.revision + 1);
        assert_eq!(after.history[0], previous.history[0]);
        let moved = db
            .research_classification_decisions(&id, "file", "renamed-file")
            .await
            .unwrap();
        assert_eq!(moved.decisions, decisions.decisions);
        assert_eq!(moved.revision, decisions.revision + 1);
        assert!(moved
            .decisions
            .iter()
            .any(|d| d.tag == "removed" && !d.present));
        assert_eq!(
            db.get_document_tags("file", "renamed-file").await.unwrap(),
            vec!["keep"]
        );
        assert!(
            progress(&db, &id, &run, 0, "complete").await.is_err(),
            "frozen old run is not retargeted"
        );
        let conn = db.conn.lock().await;
        assert_eq!(
            serde_json::to_string(
                &load::<SourceHistory>(&conn, &foreign_key)
                    .await
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_string(&foreign).unwrap()
        );
        assert!(load::<SourceHistory>(
            &conn,
            &key("source", &(&other.id, "file", "renamed-file")).unwrap()
        )
        .await
        .unwrap()
        .is_none());
        assert_eq!(
            load::<SourceHistory>(&conn, &key("source", &(&id, "file", "paper-a")).unwrap())
                .await
                .unwrap()
                .unwrap()
                .records,
            previous.history
        );
    }
}
#[tokio::test]
async fn source_identity_rename_refuses_target_decisions_and_ambiguous_source_spaces_atomically() {
    let (db, _temp) = test_db().await;
    let id = seed(&db).await;
    db.set_source_lifecycle(&id, request(0, SourceLifecycleState::Excluded), "human")
        .await
        .unwrap();
    let key = super::super::research_runs::key("decisions", &("file", "renamed-file")).unwrap();
    let target = wenlan_types::research::SourceClassificationDecisions {
        revision: 9,
        decisions: vec![wenlan_types::research::TagDecision {
            tag: "target-human-removal".into(),
            present: false,
            actor: "human".into(),
            decision_id: "distinct".into(),
        }],
    };
    {
        let conn = db.conn.lock().await;
        save(&conn, &key, &target).await.unwrap();
    }
    assert!(db
        .rebind_source_id("file", "paper-a", "renamed-file")
        .await
        .is_err());
    assert_eq!(
        db.source_lifecycle(&id, "file", "paper-a")
            .await
            .unwrap()
            .record
            .state,
        SourceLifecycleState::Excluded
    );
    {
        let conn = db.conn.lock().await;
        assert_eq!(
            load::<wenlan_types::research::SourceClassificationDecisions>(&conn, &key)
                .await
                .unwrap()
                .unwrap(),
            target
        );
        assert!(load::<SourceHistory>(
            &conn,
            &self::key("source", &(&id, "file", "renamed-file")).unwrap()
        )
        .await
        .unwrap()
        .is_none());
    }
    let other = db.create_space("Other", None, false).await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type) VALUES('foreign-copy','file','paper-a','Foreign','foreign text','Other',0,1,'text')",()).await.unwrap();
    }
    assert!(db
        .rebind_source_id("file", "paper-a", "another-file")
        .await
        .is_err());
    assert!(
        db.resolve_source_reference(&other.id, "foreign-copy")
            .await
            .is_err(),
        "split-source resolver still refuses incomplete/cross-Space identity"
    );
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT source_id FROM memories WHERE id IN ('paper-a','foreign-copy') ORDER BY id",
            (),
        )
        .await
        .unwrap();
    while let Some(row) = rows.next().await.unwrap() {
        assert_eq!(row.get::<String>(0).unwrap(), "paper-a");
    }
}

#[tokio::test]
async fn source_identity_rename_refuses_distinct_target_lifecycle_and_stale_metadata_cas() {
    let (db, _temp) = test_db().await;
    let id = seed(&db).await;
    db.set_source_lifecycle(&id, request(0, SourceLifecycleState::Excluded), "human")
        .await
        .unwrap();
    let old_key = key("source", &(&id, "file", "paper-a")).unwrap();
    let target_key = key("source", &(&id, "file", "renamed-file")).unwrap();
    let target = SourceHistory {
        records: vec![SourceLifecycleRecord {
            revision: 7,
            state: SourceLifecycleState::Retracted,
            reason: "Independent target decision".into(),
            ..default_record(&id, "file", "renamed-file")
        }],
    };
    {
        let conn = db.conn.lock().await;
        save(&conn, &target_key, &target).await.unwrap();
    }
    assert!(db
        .rebind_source_id("file", "paper-a", "renamed-file")
        .await
        .is_err());
    assert_eq!(
        db.source_lifecycle(&id, "file", "paper-a")
            .await
            .unwrap()
            .record
            .state,
        SourceLifecycleState::Excluded
    );
    let conn = db.conn.lock().await;
    assert_eq!(
        serde_json::to_string(
            &load::<SourceHistory>(&conn, &target_key)
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_string(&target).unwrap()
    );
    let unused_key = key("source", &(&id, "file", "unused-target")).unwrap();
    assert!(
        copy_identity_metadata_on(&conn, &old_key, "stale expected value", &unused_key, "{}")
            .await
            .is_err()
    );
    assert!(metadata_value_on(&conn, &unused_key)
        .await
        .unwrap()
        .is_none());
}
