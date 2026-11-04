use super::*;
use crate::citations::{check_write_citations, WriteCitations};
use crate::post_write::{page_write_with_citation_snapshot, PageWrite};

const CLAIM: &str = "Neural retrieval calibration improves citation precision through structured contextual evidence";

async fn fixture() -> (MemoryDB, tempfile::TempDir) {
    let (db, dir) = super::super::tests::test_db().await;
    let conn = db.conn.lock().await;
    for (index, text) in [
        "Unrelated background methodology ".repeat(90),
        CLAIM.repeat(12),
    ]
    .into_iter()
    .enumerate()
    {
        conn.execute("INSERT INTO memories (id,source,source_id,title,content,chunk_index,last_modified,space,version,content_hash,source_agent,chunk_type)
            VALUES (?1,'file','zotero::paper','Paper',?2,?3,10,'Research',1,'hash-a','zotero','text')",
            libsql::params![format!("chunk-{index}"),text,index as i64]).await.unwrap();
    }
    drop(conn);
    (db, dir)
}
async fn checked(db: &MemoryDB) -> (String, Vec<CitationSourceSnapshot>) {
    match check_write_citations(
        db,
        &format!("{CLAIM} [1]."),
        &["zotero::paper".into()],
        &ReadScope::Space("Research".into()),
    )
    .await
    .unwrap()
    .unwrap()
    {
        WriteCitations::Checked {
            citations_json,
            snapshots,
            ..
        } => (citations_json, snapshots),
        _ => panic!("expected checked citations"),
    }
}
async fn create(
    db: &MemoryDB,
    id: &str,
    content: &str,
    json: Option<&str>,
    guards: &[CitationSourceSnapshot],
) -> Result<(), WenlanError> {
    db.insert_page_with_basis(
        id,
        id,
        None,
        content,
        None,
        Some("Research"),
        &[],
        "2026-11-04T00:00:00Z",
        "distilled",
        "unconfirmed",
        Some("Research"),
        json,
        &[],
        "test",
        None,
        guards,
    )
    .await
}
async fn mutate(db: &MemoryDB) {
    db.conn
        .lock()
        .await
        .execute(
            "UPDATE memories SET content='Changed in the same generation' WHERE id='chunk-1'",
            (),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn citation_snapshot_late_body_and_exact_file_kind_are_supported() {
    let (db, _dir) = fixture().await;
    let (json, snapshots) = checked(&db).await;
    let citations: Vec<wenlan_types::pages::PageCitation> = serde_json::from_str(&json).unwrap();
    assert_eq!(citations[0].source_kind, "memory");
    assert_eq!(citations[0].locator, "zotero::paper");
    assert_eq!(citations[0].status, "verified");
    assert_eq!(snapshots[0].chunk_count, 2);
    assert_eq!(
        citations[0].cited_digest.as_deref(),
        Some(snapshots[0].text_digest.as_str())
    );
    let sources = db
        .citation_source_metadata(
            &["zotero::paper".into()],
            &ReadScope::Space("Research".into()),
        )
        .await
        .unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_id, "zotero::paper");
    // Causal control: the same claim cannot pass using only the old prefix.
    db.conn
        .lock()
        .await
        .execute(
            "UPDATE memories SET content='Unrelated tail' WHERE id='chunk-1'",
            (),
        )
        .await
        .unwrap();
    assert!(check_write_citations(
        &db,
        &format!("{CLAIM} [1]."),
        &["zotero::paper".into()],
        &ReadScope::Space("Research".into())
    )
    .await
    .unwrap()
    .is_err());
}

#[tokio::test]
async fn citation_snapshot_refuses_wrong_space_and_mixed_chunk_generations() {
    for update in [
        "UPDATE memories SET space='Other' WHERE id='chunk-1'",
        "UPDATE memories SET version=2 WHERE id='chunk-1'",
        "UPDATE memories SET content_hash='hash-b' WHERE id='chunk-1'",
        "UPDATE memories SET chunk_index=2 WHERE id='chunk-1'",
        "UPDATE memories SET source='webpage' WHERE id='chunk-1'",
        "INSERT INTO memories (id,source,source_id,title,content,chunk_index,last_modified,supersedes,supersede_mode,pending_revision,chunk_type) VALUES ('hide','memory','replacement','new','hidden',0,11,'zotero::paper','hide',0,'text')",
    ] {
        let (db,_dir)=fixture().await;
        db.conn.lock().await.execute(update,()).await.unwrap();
        assert!(db.citation_sources(&["zotero::paper".into()],&ReadScope::Space("Research".into())).await.unwrap()[0].is_none(),"{update}");
    }
    let (db, _dir) = fixture().await;
    for body in [CLAIM.to_string(), format!("{CLAIM} [1].")] {
        assert!(check_write_citations(
            &db,
            &body,
            &["zotero::paper".into()],
            &ReadScope::Space("Other".into())
        )
        .await
        .is_err());
    }
}

#[tokio::test]
async fn citation_snapshot_distributed_tokens_and_tiny_tail_are_not_evidence() {
    let (db, _dir) = fixture().await;
    let scattered = CLAIM
        .split_whitespace()
        .map(|word| {
            format!(
                "{} {word} ",
                "irrelevant methodology background ".repeat(50)
            )
        })
        .collect::<String>();
    db.conn
        .lock()
        .await
        .execute(
            "UPDATE memories SET content=?1 WHERE id='chunk-1'",
            libsql::params![scattered],
        )
        .await
        .unwrap();
    assert!(check_write_citations(
        &db,
        &format!("{CLAIM} [1]."),
        &["zotero::paper".into()],
        &ReadScope::Space("Research".into())
    )
    .await
    .unwrap()
    .is_err());
}

#[tokio::test]
async fn citation_snapshot_check_to_create_update_and_stage_races_write_nothing() {
    let (db, _dir) = fixture().await;
    let (json, snapshots) = checked(&db).await;
    create(
        &db,
        "existing",
        "Original human-independent body",
        None,
        &[],
    )
    .await
    .unwrap();
    let before = db.get_page("existing").await.unwrap().unwrap();
    let source_revision = db
        .try_get_page_source_revision("existing")
        .await
        .unwrap()
        .unwrap();
    mutate(&db).await; // Not linked to existing, so its source_revision remains unchanged.
    assert_eq!(
        db.try_get_page_source_revision("existing")
            .await
            .unwrap()
            .unwrap(),
        source_revision
    );
    assert!(matches!(
        create(&db, "new", CLAIM, Some(&json), &snapshots).await,
        Err(WenlanError::Conflict(_))
    ));
    assert!(db.get_page("new").await.unwrap().is_none());
    let result = page_write_with_citation_snapshot(
        &db,
        PageWrite::AgentUpdate {
            page_id: "existing",
            req: wenlan_types::requests::UpdatePageRequest {
                content: format!("{CLAIM} [1]."),
                source_memory_ids: vec!["zotero::paper".into()],
                expected_version: Some(before.version),
                caller_id: None,
                operation_id: None,
            },
            agent: Some("test"),
            expected_source_revision: source_revision,
            knowledge_path: None,
            basis: super::super::BasisWrite::default(),
            citations: Some((json, "1 verified".into())),
        },
        &snapshots,
    )
    .await;
    assert!(
        matches!(result, Err(WenlanError::Conflict(_))),
        "{result:?}"
    );
    let after = db.get_page("existing").await.unwrap().unwrap();
    assert_eq!(
        (after.version, after.content),
        (before.version, before.content.clone())
    );
    assert!(matches!(
        db.link_page_sources_with_citation_snapshot(
            "existing",
            &["zotero::paper".into()],
            "same_title",
            &snapshots
        )
        .await,
        Err(WenlanError::Conflict(_))
    ));
    assert!(db
        .get_page("existing")
        .await
        .unwrap()
        .unwrap()
        .source_memory_ids
        .is_empty());
    let pending_before = db
        .scalar_for_test("SELECT COUNT(*) FROM memories WHERE pending_revision=1")
        .await;
    let result = crate::post_write::stage_page_revision_card_with_citation_snapshot(
        &db,
        &before,
        &format!("{CLAIM} [1]."),
        &["zotero::paper".into()],
        source_revision,
        "agent_refresh",
        Some("test"),
        None,
        &[],
        Some(&ReadScope::Space("Research".into())),
        &snapshots,
    )
    .await;
    assert!(
        matches!(result, Err(WenlanError::Conflict(_))),
        "{result:?}"
    );
    assert_eq!(
        db.scalar_for_test("SELECT COUNT(*) FROM memories WHERE pending_revision=1")
            .await,
        pending_before
    );
}

#[tokio::test]
async fn citation_snapshot_accept_rechecks_and_stores_verified_late_body() {
    let (db, _dir) = fixture().await;
    let (_, snapshots) = checked(&db).await;
    create(&db, "existing", "Original body", None, &[])
        .await
        .unwrap();
    let page = db.get_page("existing").await.unwrap().unwrap();
    let revision = db
        .try_get_page_source_revision("existing")
        .await
        .unwrap()
        .unwrap();
    let staged = crate::post_write::stage_page_revision_card_with_citation_snapshot(
        &db,
        &page,
        &format!("{CLAIM} [1]."),
        &["zotero::paper".into()],
        revision,
        "agent_refresh",
        Some("test"),
        None,
        &[],
        Some(&ReadScope::Space("Research".into())),
        &snapshots,
    )
    .await
    .unwrap();
    crate::post_write::accept_pending_revision_with_knowledge_path(
        &db,
        staged.revision_card_id.as_deref().unwrap(),
        "human",
        None,
    )
    .await
    .unwrap();
    let accepted = db.get_page("existing").await.unwrap().unwrap();
    assert_eq!(accepted.citations.len(), 1);
    assert_eq!(accepted.citations[0].status, "verified");
    assert_eq!(accepted.citations[0].source_kind, "memory");
}

#[tokio::test]
async fn citation_snapshot_chunk_row_ids_preserve_single_chunk_scope() {
    let (db, _dir) = fixture().await;
    for kind in ["file", "webpage"] {
        db.conn
            .lock()
            .await
            .execute("UPDATE memories SET source=?1", libsql::params![kind])
            .await
            .unwrap();
        let scope = ReadScope::Space("Research".into());
        let sources = db
            .citation_sources(&["chunk-0".into(), "chunk-1".into()], &scope)
            .await
            .unwrap();
        assert!(!sources[0].as_ref().unwrap().text.contains(CLAIM));
        assert!(sources[1].as_ref().unwrap().text.contains(CLAIM));
        assert_eq!(sources[1].as_ref().unwrap().snapshot.locator, "chunk-1");
        assert!(
            check_write_citations(&db, &format!("{CLAIM} [1]."), &["chunk-0".into()], &scope)
                .await
                .unwrap()
                .is_err()
        );
        assert!(
            check_write_citations(&db, &format!("{CLAIM} [1]."), &["chunk-1".into()], &scope)
                .await
                .unwrap()
                .is_ok()
        );
    }
}

#[tokio::test]
async fn citation_snapshot_human_changes_stage_whole_card_and_source_change_blocks_accept() {
    for (locked, change_source) in [(false, false), (false, true), (true, false), (true, true)] {
        let (db, _dir) = fixture().await;
        create(&db, "protected", "Original body", None, &[])
            .await
            .unwrap();
        crate::post_write::update_page(
            &db,
            "protected",
            wenlan_types::requests::UpdatePageRequest {
                content: "A person wrote this protected sentence. [1]".into(),
                source_memory_ids: vec![],
                expected_version: None,
                caller_id: None,
                operation_id: None,
            },
            "manual_edit",
            false,
            None,
            None,
        )
        .await
        .unwrap();
        db.set_page_citations(
            "protected",
            Some(
                &serde_json::json!([{
                    "occurrence":1,"marker":1,"source_kind":"memory","locator":"zotero::paper",
                    "score":0.0,"status":"unverified","scope":"sentence","disproven":true
                }])
                .to_string(),
            ),
        )
        .await
        .unwrap();
        db.set_page_review_required("protected", locked)
            .await
            .unwrap();
        let page = db.get_page("protected").await.unwrap().unwrap();
        let revision = db
            .try_get_page_source_revision("protected")
            .await
            .unwrap()
            .unwrap();
        let (json, snapshots) = checked(&db).await;
        let result = page_write_with_citation_snapshot(
            &db,
            PageWrite::AgentUpdate {
                page_id: "protected",
                req: wenlan_types::requests::UpdatePageRequest {
                    content: format!("{CLAIM} [1]."),
                    source_memory_ids: vec!["zotero::paper".into()],
                    expected_version: Some(page.version),
                    caller_id: None,
                    operation_id: None,
                },
                agent: Some("test"),
                expected_source_revision: revision,
                knowledge_path: None,
                basis: super::super::BasisWrite::default(),
                citations: Some((json, "1 verified".into())),
            },
            &snapshots,
        )
        .await
        .unwrap();
        assert!(
            result.gated && result.revision_card_id.is_some(),
            "{result:?}"
        );
        assert_eq!(
            db.get_page("protected").await.unwrap().unwrap().content,
            page.content
        );
        assert_eq!(
            db.scalar_for_test("SELECT COUNT(*) FROM sentence_history WHERE op='sentence.propose'")
                .await,
            0
        );
        if !change_source {
            crate::post_write::accept_pending_revision_with_knowledge_path(
                &db,
                result.revision_card_id.as_deref().unwrap(),
                "human",
                None,
            )
            .await
            .unwrap();
            let adopted = db.get_page("protected").await.unwrap().unwrap();
            assert_eq!(adopted.content, format!("{CLAIM} [1]."));
            assert_eq!(adopted.citations.len(), 1);
            assert_eq!(adopted.citations[0].status, "verified");
            assert!(db
                .pending_memory_revision_payload(result.revision_card_id.as_deref().unwrap())
                .await
                .unwrap()
                .is_none());
            continue;
        }
        mutate(&db).await;
        let accepted = crate::post_write::accept_pending_revision_with_knowledge_path(
            &db,
            result.revision_card_id.as_deref().unwrap(),
            "human",
            None,
        )
        .await;
        assert!(accepted.is_err());
        assert_eq!(
            db.get_page("protected").await.unwrap().unwrap().content,
            page.content
        );
        assert!(db
            .pending_memory_revision_payload(result.revision_card_id.as_deref().unwrap())
            .await
            .unwrap()
            .is_some());
    }
}

#[tokio::test]
async fn citation_snapshot_twenty_long_sources_record_bounded_window_timing() {
    let (db, _dir) = super::super::tests::test_db().await;
    let mut ids = Vec::new();
    let mut body = String::new();
    for i in 0..20 {
        let id = format!("paper-{i}");
        let suffix = char::from(b'a' + i as u8);
        let claim = format!("Study{suffix} retrieval{suffix} calibration{suffix} precision{suffix} evidence{suffix} context{suffix}");
        let text = format!(
            "{}\n{claim}",
            "Unrelated methodological background detail ".repeat(2500)
        );
        db.conn.lock().await.execute("INSERT INTO memories (id,source,source_id,title,content,chunk_index,last_modified,space,version,chunk_type) VALUES (?1,'file',?1,?1,?2,0,10,'Research',1,'text')",libsql::params![id.as_str(),text]).await.unwrap();
        body.push_str(&format!("{claim} [{}].\n\n", i + 1));
        ids.push(id);
    }
    body.push_str("Studya retrievala calibrationa precisiona evidencea contexta Studyb retrievalb calibrationb precisionb evidenceb contextb [1][2].");
    let started = std::time::Instant::now();
    let result = check_write_citations(&db, &body, &ids, &ReadScope::Space("Research".into()))
        .await
        .unwrap();
    eprintln!(
        "citation_snapshot_20_sources_100k_chars_elapsed_ms={}",
        started.elapsed().as_millis()
    );
    let WriteCitations::Checked { citations_json, .. } = result.unwrap() else {
        panic!("checked")
    };
    let citations: Vec<wenlan_types::pages::PageCitation> =
        serde_json::from_str(&citations_json).unwrap();
    assert_eq!(citations.len(), 22);
    assert!(citations
        .iter()
        .all(|citation| citation.status == "verified"));
    for (i, citation) in citations.iter().take(20).enumerate() {
        assert_eq!(citation.locator, ids[i]);
    }
    ids.swap(0, 1);
    let WriteCitations::Checked { citations_json, .. } =
        check_write_citations(&db, &body, &ids, &ReadScope::Space("Research".into()))
            .await
            .unwrap()
            .unwrap()
    else {
        panic!("checked")
    };
    let swapped: Vec<wenlan_types::pages::PageCitation> =
        serde_json::from_str(&citations_json).unwrap();
    assert_eq!(swapped[0].status, "unverified");
    assert_eq!(swapped[1].status, "unverified");
}

#[tokio::test]
async fn citation_snapshot_attach_preserves_indexed_ids_and_chunk_late_body() {
    let (db, _dir) = fixture().await;
    create(&db, "attached", "Existing prose", None, &[])
        .await
        .unwrap();
    let (_, snapshots) = checked(&db).await;
    db.link_page_sources_with_citation_snapshot(
        "attached",
        &["zotero::paper".into()],
        "same_title",
        &snapshots,
    )
    .await
    .unwrap();
    assert_eq!(
        db.get_page("attached")
            .await
            .unwrap()
            .unwrap()
            .source_memory_ids,
        vec!["zotero::paper"]
    );
    let sources = db
        .get_page_sources_scoped("attached", &ReadScope::Space("Research".into()))
        .await
        .unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].memory_source_id, "zotero::paper");
    db.conn
        .lock()
        .await
        .execute(
            "UPDATE memories SET content=?1 WHERE id='chunk-1'",
            libsql::params![format!("{} {CLAIM}", "Unrelated methods ".repeat(150))],
        )
        .await
        .unwrap();
    assert!(check_write_citations(
        &db,
        &format!("{CLAIM} [1]."),
        &["chunk-1".into()],
        &ReadScope::Space("Research".into())
    )
    .await
    .unwrap()
    .is_ok());
}

#[tokio::test]
async fn citation_snapshot_adjust_does_not_inherit_staged_verified_status() {
    for edited in [
        "The person chooses to keep this unrelated observation [1].".to_string(),
        format!("{CLAIM} [99] [1]."),
    ] {
        let (db, _dir) = fixture().await;
        let (_, snapshots) = checked(&db).await;
        create(&db, "adjusted", "Original body", None, &[])
            .await
            .unwrap();
        let page = db.get_page("adjusted").await.unwrap().unwrap();
        let revision = db
            .try_get_page_source_revision("adjusted")
            .await
            .unwrap()
            .unwrap();
        let proposed = format!("{CLAIM} [1].");
        let staged = crate::post_write::stage_page_revision_card_with_citation_snapshot(
            &db,
            &page,
            &proposed,
            &["zotero::paper".into()],
            revision,
            "agent_refresh",
            Some("test"),
            None,
            &[],
            Some(&ReadScope::Space("Research".into())),
            &snapshots,
        )
        .await
        .unwrap();
        crate::post_write::accept_pending_revision_edited_with_knowledge_path(
            &db,
            staged.revision_card_id.as_deref().unwrap(),
            "human",
            None,
            &wenlan_types::requests::RevisionAcceptEditRequest {
                expected_content: proposed,
                content: edited.clone(),
            },
        )
        .await
        .unwrap();
        let page = db.get_page("adjusted").await.unwrap().unwrap();
        assert_eq!(page.content, edited);
        assert!(page.citations.is_empty());
    }
}

#[tokio::test]
async fn citation_snapshot_recheck_preserves_legacy_folder_and_page_sources() {
    for page_source in [false, true] {
        let (db, _dir) = fixture().await;
        let locator = if page_source {
            create(&db, "basis-page", CLAIM, None, &[]).await.unwrap();
            "basis-page"
        } else {
            db.conn.lock().await.execute(
                "UPDATE memories SET source='memory',source_agent='folder' WHERE source_id='zotero::paper'", (),
            ).await.unwrap();
            "zotero::paper"
        };
        let body = format!("{CLAIM} [1].");
        let json = serde_json::json!([{"occurrence":1,"marker":1,"source_kind":"external_file",
            "locator":locator,"score":1.0,"status":"verified","scope":"sentence"}])
        .to_string();
        create(&db, "rechecked", &body, Some(&json), &[])
            .await
            .unwrap();
        let hash = crate::authorship::sentence_units(&body)[0].hash.clone();
        crate::sentences::recheck_sentence(&db, "rechecked", &hash)
            .await
            .unwrap();
        assert_eq!(
            db.get_page("rechecked").await.unwrap().unwrap().citations[0].status,
            "verified"
        );
    }
}

#[tokio::test]
async fn citation_snapshot_recheck_uses_actual_document_mode_and_page_space_fence() {
    let (db, _dir) = fixture().await;
    let dispersed = CLAIM
        .split_whitespace()
        .map(|token| format!("{token} {}", "unrelated ".repeat(120)))
        .collect::<Vec<_>>()
        .join(" ");
    db.conn
        .lock()
        .await
        .execute(
            "UPDATE memories SET content=?1 WHERE source_id='zotero::paper'",
            libsql::params![dispersed],
        )
        .await
        .unwrap();
    let body = format!("{CLAIM} [1].");
    let json = serde_json::json!([{"occurrence":1,"marker":1,"source_kind":"memory",
        "locator":"zotero::paper","score":1.0,"status":"verified","scope":"sentence"}])
    .to_string();
    create(&db, "rechecked", &body, Some(&json), &[])
        .await
        .unwrap();
    let hash = crate::authorship::sentence_units(&body)[0].hash.clone();
    crate::sentences::recheck_sentence(&db, "rechecked", &hash)
        .await
        .unwrap();
    let page = db.get_page("rechecked").await.unwrap().unwrap();
    assert_eq!(page.citations[0].status, "unverified");
    let snapshots = db
        .citation_sources(
            &["zotero::paper".into()],
            &ReadScope::Space("Research".into()),
        )
        .await
        .unwrap()
        .into_iter()
        .map(|source| source.unwrap().snapshot)
        .collect::<Vec<_>>();
    db.conn
        .lock()
        .await
        .execute("UPDATE pages SET space='Other' WHERE id='rechecked'", ())
        .await
        .unwrap();
    assert!(!db
        .set_page_citations_at_page_version("rechecked", &json, page.version, &snapshots)
        .await
        .unwrap());
    assert_eq!(
        db.get_page("rechecked").await.unwrap().unwrap().citations[0].status,
        "unverified"
    );
}
