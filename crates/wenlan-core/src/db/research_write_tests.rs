// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::db::{tests::test_db, BasisWrite};
use wenlan_types::research::*;
async fn seed(db: &MemoryDB) -> String {
    let space = db.create_space("Research", None, false).await.unwrap();
    let conn = db.conn.lock().await;
    conn.execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type) VALUES('paper','file','paper','Paper','actual complete text','Research',0,1,'text')",()).await.unwrap();
    space.id
}
async fn prepared(
    db: &MemoryDB,
    space: &str,
    target: Option<&str>,
) -> (OrganizeRun, ResearchWriteFence) {
    let value = db
        .create_research_run(
            space,
            CreateOrganizeRunRequest {
                space: "Research".into(),
                sources: vec![ResearchSourceRef {
                    source: "file".into(),
                    source_id: "paper".into(),
                    title: String::new(),
                }],
                instructions: None,
                target_page_id: target.map(str::to_string),
            },
        )
        .await
        .unwrap();
    let read = db
        .read_organize_source(
            space,
            &value.id,
            ReadOrganizeSourceRequest {
                space: "Research".into(),
                source: "file".into(),
                source_id: "paper".into(),
                offset: 0,
                limit: 16000,
            },
        )
        .await
        .unwrap();
    let value = db
        .update_research_run(
            space,
            &value.id,
            UpdateOrganizeRunRequest {
                space: "Research".into(),
                expected_revision: read.run_revision,
                source: Some("file".into()),
                source_id: Some("paper".into()),
                coverage: Some(ResearchCoverage::Complete),
                content_digest: Some(value.items[0].content_digest.clone()),
                next_offset: None,
                error: None,
                cancel: false,
                resume: false,
            },
        )
        .await
        .unwrap();
    let fence = db
        .prepare_research_write(
            &value.id,
            &ReadScope::Space("Research".into()),
            target,
            &["paper".into()],
        )
        .await
        .unwrap();
    (value, fence)
}
async fn page(
    db: &MemoryDB,
    id: &str,
    fence: Option<&ResearchWriteFence>,
) -> Result<(), WenlanError> {
    db.insert_page_with_basis(
        id,
        "Topic",
        None,
        "actual complete text",
        None,
        Some("Research"),
        &["paper"],
        "2026-11-06T00:00:00Z",
        "research",
        "confirmed",
        Some("Research"),
        None,
        &[],
        "agent",
        None,
        &[],
        fence,
    )
    .await
}
#[tokio::test]
async fn research_create_binds_actual_page_and_rejects_stale_source_without_orphan() {
    let (db, _temp) = test_db().await;
    let space = seed(&db).await;
    let (value, fence) = prepared(&db, &space, None).await;
    {
        let conn = db.conn.lock().await;
        conn.execute("UPDATE memories SET content='changed' WHERE id='paper'", ())
            .await
            .unwrap();
    }
    assert!(page(&db, "orphan", Some(&fence)).await.is_err());
    assert!(db.get_page("orphan").await.unwrap().is_none());
    assert_eq!(
        db.research_run(&space, &value.id).await.unwrap().state,
        OrganizeRunState::InProgress
    );
    let (value, fence) = prepared(&db, &space, None).await;
    page(&db, "bound", Some(&fence)).await.unwrap();
    let saved = db.research_run(&space, &value.id).await.unwrap();
    assert_eq!(saved.target_page_id.as_deref(), Some("bound"));
    assert_eq!(
        saved.target_page_incarnation.as_deref(),
        Some(
            db.try_get_page_fence("bound")
                .await
                .unwrap()
                .unwrap()
                .incarnation
                .as_str()
        )
    );
    assert_eq!(saved.result_page_version, Some(1));
    assert_eq!(saved.state, OrganizeRunState::Completed);
}
#[tokio::test]
async fn research_stage_and_adopt_bind_in_same_transactions() {
    let (db, _temp) = test_db().await;
    let space = seed(&db).await;
    page(&db, "topic", None).await.unwrap();
    let (value, fence) = prepared(&db, &space, Some("topic")).await;
    let current = db.get_page("topic").await.unwrap().unwrap();
    let source_revision = db.get_page_source_revision("topic").await.unwrap();
    let result = crate::post_write::stage_page_revision_card_with_citations(
        &db,
        &current,
        "actual complete text, reviewed update",
        &["paper".into()],
        source_revision,
        "agent_refresh",
        Some("agent"),
        None,
        &[],
        Some(&ReadScope::Space("Research".into())),
        &[],
        None,
        Some(&fence),
    )
    .await
    .unwrap();
    let pending = result.revision_card_id.unwrap();
    let staged = db.research_run(&space, &value.id).await.unwrap();
    assert_eq!(staged.state, OrganizeRunState::AwaitingReview);
    assert_eq!(
        staged.pending_revision_id.as_deref(),
        Some(pending.as_str())
    );
    assert_eq!(staged.result_page_version, None);
    crate::post_write::accept_pending_revision(&db, &pending, "review")
        .await
        .unwrap();
    let adopted = db.research_run(&space, &value.id).await.unwrap();
    assert_eq!(adopted.state, OrganizeRunState::Completed);
    assert_eq!(adopted.result_page_version, Some(2));
    assert_eq!(adopted.pending_revision_id, None);
}
#[tokio::test]
async fn research_target_scope_and_rename_are_checked_live() {
    let (db, _temp) = test_db().await;
    let space = seed(&db).await;
    page(&db, "topic", None).await.unwrap();
    let (value, _) = prepared(&db, &space, Some("topic")).await;
    assert!(db
        .prepare_research_write(
            &value.id,
            &ReadScope::Space("Research".into()),
            Some("other"),
            &["paper".into()]
        )
        .await
        .is_err());
    db.update_space("Research", "Renamed", None).await.unwrap();
    assert!(db
        .prepare_research_write(
            &value.id,
            &ReadScope::Space("Research".into()),
            Some("topic"),
            &["paper".into()]
        )
        .await
        .is_err());
    let fence = db
        .prepare_research_write(
            &value.id,
            &ReadScope::Space("Renamed".into()),
            Some("topic"),
            &["paper".into()],
        )
        .await
        .unwrap();
    assert_eq!(fence.space_id, space);
    let conn = db.conn.lock().await;
    conn.execute(
        "UPDATE pages SET space='Other',workspace='Other' WHERE id='topic'",
        (),
    )
    .await
    .unwrap();
    drop(conn);
    assert!(db
        .bind_research_unchanged(&fence, "topic", &["paper".into()], 1)
        .await
        .is_err());
    let _ = BasisWrite::default();
}

#[tokio::test]
async fn research_update_and_adoption_source_conflicts_roll_back_all_state() {
    let (db, _temp) = test_db().await;
    let space = seed(&db).await;
    page(&db, "topic", None).await.unwrap();
    let (value, fence) = prepared(&db, &space, Some("topic")).await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET content='different source' WHERE id='paper'",
            (),
        )
        .await
        .unwrap();
    }
    let outcome = db
        .try_update_page_content_with_changelog_at_version_and_basis(
            "topic",
            "candidate body",
            &["paper"],
            "agent_refresh",
            "[]",
            None,
            1,
            None,
            &BasisWrite {
                research_write: Some(fence),
                ..BasisWrite::default()
            },
        )
        .await;
    assert!(outcome.is_err());
    let unchanged = db.get_page("topic").await.unwrap().unwrap();
    assert_eq!(unchanged.version, 1);
    assert_eq!(unchanged.content, "actual complete text");
    assert_eq!(
        db.research_run(&space, &value.id)
            .await
            .unwrap()
            .target_page_version,
        Some(1)
    );
    let (value, fence) = prepared(&db, &space, Some("topic")).await;
    let source_revision = db.get_page_source_revision("topic").await.unwrap();
    let result = crate::post_write::stage_page_revision_card_with_citations(
        &db,
        &unchanged,
        "review candidate",
        &["paper".into()],
        source_revision,
        "agent_refresh",
        Some("agent"),
        None,
        &[],
        Some(&ReadScope::Space("Research".into())),
        &[],
        None,
        Some(&fence),
    )
    .await
    .unwrap();
    let pending = result.revision_card_id.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET content='changed again' WHERE id='paper'",
            (),
        )
        .await
        .unwrap();
    }
    assert!(
        crate::post_write::accept_pending_revision(&db, &pending, "review")
            .await
            .is_err()
    );
    assert_eq!(db.get_page("topic").await.unwrap().unwrap().version, 1);
    let waiting = db.research_run(&space, &value.id).await.unwrap();
    assert_eq!(waiting.state, OrganizeRunState::AwaitingReview);
    assert_eq!(
        waiting.pending_revision_id.as_deref(),
        Some(pending.as_str())
    );
    assert!(db
        .pending_memory_revision_payload(&pending)
        .await
        .unwrap()
        .is_some());
    crate::post_write::dismiss_pending_revision(&db, &pending, "review")
        .await
        .unwrap();
    let dismissed = db.research_run(&space, &value.id).await.unwrap();
    assert_eq!(dismissed.state, OrganizeRunState::Partial);
    assert_eq!(dismissed.pending_revision_id, None);
}
#[tokio::test]
async fn research_staging_failure_leaves_no_card_or_waiting_progress() {
    let (db, _temp) = test_db().await;
    let space = seed(&db).await;
    page(&db, "topic", None).await.unwrap();
    let (value, fence) = prepared(&db, &space, Some("topic")).await;
    let current = db.get_page("topic").await.unwrap().unwrap();
    let source_revision = db.get_page_source_revision("topic").await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute("UPDATE memories SET content='changed' WHERE id='paper'", ())
            .await
            .unwrap();
    }
    assert!(crate::post_write::stage_page_revision_card_with_citations(
        &db,
        &current,
        "review candidate",
        &["paper".into()],
        source_revision,
        "agent_refresh",
        Some("agent"),
        None,
        &[],
        Some(&ReadScope::Space("Research".into())),
        &[],
        None,
        Some(&fence)
    )
    .await
    .is_err());
    let run = db.research_run(&space, &value.id).await.unwrap();
    assert_eq!(run.revision, value.revision);
    assert_eq!(run.pending_revision_id, None);
    assert!(db.list_pending_revisions(50).await.unwrap().is_empty());
}

#[tokio::test]
async fn research_missing_source_keeps_honest_partial_result_and_can_record_failure() {
    let (db, _temp) = test_db().await;
    let space = seed(&db).await;
    {
        let conn = db.conn.lock().await;
        conn.execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type) VALUES('missing','file','missing','Missing','indexed before removal','Research',0,1,'text')",()).await.unwrap();
    }
    let value = db
        .create_research_run(
            &space,
            CreateOrganizeRunRequest {
                space: "Research".into(),
                sources: vec![
                    ResearchSourceRef {
                        source: "file".into(),
                        source_id: "paper".into(),
                        title: String::new(),
                    },
                    ResearchSourceRef {
                        source: "file".into(),
                        source_id: "missing".into(),
                        title: String::new(),
                    },
                ],
                instructions: None,
                target_page_id: None,
            },
        )
        .await
        .unwrap();
    let missing_digest = value.items[1].content_digest.clone();
    let read = db
        .read_organize_source(
            &space,
            &value.id,
            ReadOrganizeSourceRequest {
                space: "Research".into(),
                source: "file".into(),
                source_id: "paper".into(),
                offset: 0,
                limit: 16000,
            },
        )
        .await
        .unwrap();
    let value = db
        .update_research_run(
            &space,
            &value.id,
            UpdateOrganizeRunRequest {
                space: "Research".into(),
                expected_revision: read.run_revision,
                source: Some("file".into()),
                source_id: Some("paper".into()),
                coverage: Some(ResearchCoverage::Complete),
                content_digest: Some(value.items[0].content_digest.clone()),
                next_offset: None,
                error: None,
                cancel: false,
                resume: false,
            },
        )
        .await
        .unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute("DELETE FROM memories WHERE id='missing'", ())
            .await
            .unwrap();
    }
    let value = db
        .update_research_run(
            &space,
            &value.id,
            UpdateOrganizeRunRequest {
                space: "Research".into(),
                expected_revision: value.revision,
                source: Some("file".into()),
                source_id: Some("missing".into()),
                coverage: Some(ResearchCoverage::Unavailable),
                content_digest: Some(missing_digest),
                next_offset: None,
                error: Some("Source was removed before reading".into()),
                cancel: false,
                resume: false,
            },
        )
        .await
        .unwrap();
    let fence = db
        .prepare_research_write(
            &value.id,
            &ReadScope::Space("Research".into()),
            None,
            &["paper".into()],
        )
        .await
        .unwrap();
    page(&db, "partial", Some(&fence)).await.unwrap();
    let stored = db.research_run(&space, &value.id).await.unwrap();
    assert_eq!(stored.state, OrganizeRunState::Partial);
    assert_eq!(stored.result_page_version, Some(1));
    assert_eq!(stored.items[1].coverage, ResearchCoverage::Unavailable);
}

#[tokio::test]
async fn research_old_run_rejects_recreated_target_even_at_same_version() {
    let (db, _temp) = test_db().await;
    let space = seed(&db).await;
    page(&db, "topic", None).await.unwrap();
    let (_, fence) = prepared(&db, &space, Some("topic")).await;
    let old = db.try_get_page_fence("topic").await.unwrap().unwrap();
    db.delete_page("topic").await.unwrap();
    page(&db, "topic", None).await.unwrap();
    let new = db.try_get_page_fence("topic").await.unwrap().unwrap();
    assert_ne!(old.incarnation, new.incarnation);
    assert!(
        db.bind_research_unchanged(&fence, "topic", &["paper".into()], 1)
            .await
            .is_err(),
        "old run must not bind replacement page"
    );
    let updated = db
        .try_update_page_content_with_changelog_at_version_and_basis(
            "topic",
            "must not write",
            &["paper"],
            "agent_refresh",
            "[]",
            None,
            1,
            None,
            &BasisWrite {
                research_write: Some(fence.clone()),
                ..BasisWrite::default()
            },
        )
        .await;
    assert!(updated.is_err());
    let replacement = db.get_page("topic").await.unwrap().unwrap();
    assert_eq!(replacement.content, "actual complete text");
    let result = crate::post_write::stage_page_revision_card_with_citations(
        &db,
        &replacement,
        "must not stage",
        &["paper".into()],
        db.get_page_source_revision("topic").await.unwrap(),
        "agent_refresh",
        Some("agent"),
        None,
        &[],
        Some(&ReadScope::Space("Research".into())),
        &[],
        None,
        Some(&fence),
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn research_pending_old_target_cannot_adopt_into_recreated_page() {
    let (db, _temp) = test_db().await;
    let space = seed(&db).await;
    page(&db, "topic", None).await.unwrap();
    let (_, fence) = prepared(&db, &space, Some("topic")).await;
    let current = db.get_page("topic").await.unwrap().unwrap();
    let result = crate::post_write::stage_page_revision_card_with_citations(
        &db,
        &current,
        "old candidate",
        &["paper".into()],
        db.get_page_source_revision("topic").await.unwrap(),
        "agent_refresh",
        Some("agent"),
        None,
        &[],
        Some(&ReadScope::Space("Research".into())),
        &[],
        None,
        Some(&fence),
    )
    .await
    .unwrap();
    let pending = result.revision_card_id.unwrap();
    db.delete_page("topic").await.unwrap();
    page(&db, "topic", None).await.unwrap();
    assert!(
        crate::post_write::accept_pending_revision(&db, &pending, "review")
            .await
            .is_err()
    );
    assert_eq!(
        db.get_page("topic").await.unwrap().unwrap().content,
        "actual complete text"
    );
}
#[tokio::test]
async fn research_legacy_target_without_incarnation_requires_new_run() {
    let (db, _temp) = test_db().await;
    let space = seed(&db).await;
    page(&db, "topic", None).await.unwrap();
    let (mut value, fence) = prepared(&db, &space, Some("topic")).await;
    value.target_page_incarnation = None;
    {
        let conn = db.conn.lock().await;
        save(&conn, &key("run", &(&space, &value.id)).unwrap(), &value)
            .await
            .unwrap();
    }
    assert!(db
        .bind_research_unchanged(&fence, "topic", &["paper".into()], 1)
        .await
        .is_err());
}

async fn refresh_fixture(db: &MemoryDB, prior: usize, total: usize) -> (String, Vec<String>) {
    let space = seed(db).await;
    page(db, "topic", None).await.unwrap();
    let ids: Vec<String> = (0..total)
        .map(|i| format!("refresh-paper-{i:02}"))
        .collect();
    let conn = db.conn.lock().await;
    for id in &ids {
        conn.execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type) VALUES(?1,'file',?1,?1,'reviewable complete source text','Research',0,1,'text')",[id.as_str()]).await.unwrap();
    }
    drop(conn);
    db.replace_page_sources(
        "topic",
        &ids[..prior].iter().map(String::as_str).collect::<Vec<_>>(),
        "fixture",
    )
    .await
    .unwrap();
    let conn = db.conn.lock().await;
    conn.execute("UPDATE pages SET stale_reason='source_updated',sources_updated_count=?1,source_revision=40,review_required=1 WHERE id='topic'",[prior as i64]).await.unwrap();
    (space, ids)
}

async fn complete_refresh_run(
    db: &MemoryDB,
    space: &str,
    ids: &[String],
    read_count: usize,
) -> (OrganizeRun, ResearchWriteFence) {
    let mut value = db
        .create_research_run(
            space,
            CreateOrganizeRunRequest {
                space: "Research".into(),
                sources: ids
                    .iter()
                    .map(|id| ResearchSourceRef {
                        source: "file".into(),
                        source_id: id.clone(),
                        title: String::new(),
                    })
                    .collect(),
                instructions: None,
                target_page_id: Some("topic".into()),
            },
        )
        .await
        .unwrap();
    for id in ids.iter().take(read_count) {
        let read = db
            .read_organize_source(
                space,
                &value.id,
                ReadOrganizeSourceRequest {
                    space: "Research".into(),
                    source: "file".into(),
                    source_id: id.clone(),
                    offset: 0,
                    limit: 16000,
                },
            )
            .await
            .unwrap();
        let digest = value
            .items
            .iter()
            .find(|item| item.source_ref.source_id == *id)
            .unwrap()
            .content_digest
            .clone();
        value = db
            .update_research_run(
                space,
                &value.id,
                UpdateOrganizeRunRequest {
                    space: "Research".into(),
                    expected_revision: read.run_revision,
                    source: Some("file".into()),
                    source_id: Some(id.clone()),
                    coverage: Some(ResearchCoverage::Complete),
                    content_digest: Some(digest),
                    next_offset: None,
                    error: None,
                    cancel: false,
                    resume: false,
                },
            )
            .await
            .unwrap();
    }
    let adopted: Vec<String> = ids.iter().take(read_count).cloned().collect();
    let fence = db
        .prepare_research_write(
            &value.id,
            &ReadScope::Space("Research".into()),
            Some("topic"),
            &adopted,
        )
        .await
        .unwrap();
    (value, fence)
}

async fn stage_refresh(
    db: &MemoryDB,
    ids: &[String],
    fence: Option<&ResearchWriteFence>,
    with_snapshots: bool,
) -> String {
    let scope = ReadScope::Space("Research".into());
    let snapshots = if with_snapshots {
        db.citation_sources(ids, &scope)
            .await
            .unwrap()
            .into_iter()
            .map(|source| source.unwrap().snapshot)
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    crate::post_write::stage_page_revision_card_with_citations(
        db,
        &db.get_page("topic").await.unwrap().unwrap(),
        "human approved refreshed research",
        ids,
        db.get_page_source_revision("topic").await.unwrap(),
        "agent_refresh",
        Some("agent"),
        None,
        &[],
        Some(&scope),
        &snapshots,
        None,
        fence,
    )
    .await
    .unwrap()
    .revision_card_id
    .unwrap()
}

#[tokio::test]
async fn research_reviewed_twenty_to_twenty_one_consumes_only_ordinary_source_staleness() {
    let (db, _temp) = test_db().await;
    let (space, ids) = refresh_fixture(&db, 20, 21).await;
    let (run, fence) = complete_refresh_run(&db, &space, &ids, ids.len()).await;
    let pending = stage_refresh(&db, &ids, Some(&fence), true).await;
    assert_eq!(
        db.get_page("topic")
            .await
            .unwrap()
            .unwrap()
            .sources_updated_count,
        20
    );
    crate::post_write::accept_pending_revision(&db, &pending, "human-reviewer")
        .await
        .unwrap();
    let page = db.get_page("topic").await.unwrap().unwrap();
    assert_eq!(
        page.stale_reason, None,
        "reviewing the full current run must consume ordinary source_updated"
    );
    assert_eq!(page.sources_updated_count, 0);
    assert!(
        page.review_required,
        "the continuous human review lock survives adoption"
    );
    assert_eq!(page.source_memory_ids, ids);
    assert_eq!(
        db.research_run(&space, &run.id).await.unwrap().state,
        OrganizeRunState::Completed
    );
    assert!(db
        .pending_memory_revision_payload(&pending)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn research_reviewed_refresh_keeps_unproven_or_protected_staleness() {
    for case in [
        "omitted_prior",
        "partial_run",
        "generic",
        "legacy_snapshots",
        "partial_snapshots",
        "lifecycle",
        "selection",
        "other_reason",
        "extra_live_provenance",
    ] {
        let (db, _temp) = test_db().await;
        let (space, ids) = refresh_fixture(&db, 1, 2).await;
        if case == "lifecycle" {
            db.set_source_lifecycle(
                &space,
                wenlan_types::source_lifecycle::SetSourceLifecycleRequest {
                    space: "Research".into(),
                    source: "file".into(),
                    source_id: ids[0].clone(),
                    expected_revision: 0,
                    state: wenlan_types::source_lifecycle::SourceLifecycleState::Active,
                    reason: "reviewed lifecycle history".into(),
                },
                "human",
            )
            .await
            .unwrap();
        }
        if case == "selection" {
            let conn = db.conn.lock().await;
            super::super::zotero_scope_impacts::zotero_scope_impact_on(
                &conn,
                &space,
                &ids[..1],
                "collection selection changed",
            )
            .await
            .unwrap();
        }
        if case == "other_reason" || case == "extra_live_provenance" {
            let conn = db.conn.lock().await;
            if case == "other_reason" {
                conn.execute(
                    "UPDATE pages SET stale_reason='basis_changed' WHERE id='topic'",
                    (),
                )
                .await
                .unwrap();
            } else {
                conn.execute("INSERT INTO edges(edge_id,src_kind,src_id,dst_kind,dst_id,edge_type,lineage,grounded,space,weight,created_at) VALUES('extra-prior-evidence','page','topic','memory','paper','cites','evidence',1,'Research',1,1)", ()).await.unwrap();
            }
        }
        let selected = if case == "omitted_prior" {
            &ids[1..]
        } else {
            &ids[..]
        };
        let read_count = if case == "partial_run" {
            1
        } else {
            selected.len()
        };
        let (_run, fence) = complete_refresh_run(&db, &space, selected, read_count).await;
        let adopted = &selected[..read_count];
        let pending = stage_refresh(
            &db,
            adopted,
            (case != "generic").then_some(&fence),
            case != "legacy_snapshots",
        )
        .await;
        if case == "partial_snapshots" {
            let conn = db.conn.lock().await;
            conn.execute("UPDATE memories SET structured_fields=json_remove(structured_fields,'$.citation_source_snapshots[1]') WHERE source_id=?1", [pending.as_str()]).await.unwrap();
        }
        let before = db.get_page("topic").await.unwrap().unwrap();
        crate::post_write::accept_pending_revision(&db, &pending, "human-reviewer")
            .await
            .unwrap();
        let after = db.get_page("topic").await.unwrap().unwrap();
        assert_eq!(after.stale_reason, before.stale_reason, "{case}");
        assert_eq!(
            after.sources_updated_count, before.sources_updated_count,
            "{case}"
        );
        assert!(after.review_required, "{case}");
        assert_eq!(
            after.content, "human approved refreshed research",
            "{case}: adoption still works"
        );
    }
}

#[tokio::test]
async fn research_reviewed_refresh_generation_race_and_late_failure_roll_back_acknowledgement() {
    for case in ["same_bytes_new_generation", "late_card_delete_failure"] {
        let (db, _temp) = test_db().await;
        let (space, ids) = refresh_fixture(&db, 1, 2).await;
        let (run, fence) = complete_refresh_run(&db, &space, &ids, ids.len()).await;
        let pending = stage_refresh(&db, &ids, Some(&fence), true).await;
        let before = db.get_page("topic").await.unwrap().unwrap();
        let source_revision = db.get_page_source_revision("topic").await.unwrap();
        {
            let conn = db.conn.lock().await;
            if case == "same_bytes_new_generation" {
                conn.execute(
                    "UPDATE memories SET version=COALESCE(version,1)+1 WHERE source_id=?1",
                    [ids[0].as_str()],
                )
                .await
                .unwrap();
            } else {
                conn.execute("CREATE TEMP TRIGGER fail_review_card_delete BEFORE DELETE ON memories WHEN OLD.pending_revision=1 BEGIN SELECT RAISE(ABORT,'injected late review failure'); END", ()).await.unwrap();
            }
        }
        assert!(
            crate::post_write::accept_pending_revision(&db, &pending, "human-reviewer")
                .await
                .is_err(),
            "{case}"
        );
        let after = db.get_page("topic").await.unwrap().unwrap();
        assert_eq!(after.content, before.content, "{case}");
        assert_eq!(after.version, before.version, "{case}");
        assert_eq!(after.stale_reason, before.stale_reason, "{case}");
        assert_eq!(
            after.sources_updated_count, before.sources_updated_count,
            "{case}"
        );
        assert_eq!(
            db.get_page_source_revision("topic").await.unwrap(),
            source_revision,
            "{case}"
        );
        let waiting = db.research_run(&space, &run.id).await.unwrap();
        assert_eq!(waiting.state, OrganizeRunState::AwaitingReview, "{case}");
        assert_eq!(
            waiting.pending_revision_id.as_deref(),
            Some(pending.as_str()),
            "{case}"
        );
        assert!(
            db.pending_memory_revision_payload(&pending)
                .await
                .unwrap()
                .is_some(),
            "{case}"
        );
    }
}

#[tokio::test]
async fn research_survivor_review_keeps_source_removal_after_orphan_cleanup() {
    let (db, _temp) = test_db().await;
    let (space, ids) = refresh_fixture(&db, 2, 2).await;
    db.clear_page_staleness("topic").await.unwrap();
    let revision_before = db.get_page_source_revision("topic").await.unwrap();
    // Document-store removal can precede the housekeeping/refinery orphan
    // sweep. Exercise that real cleanup path, not a hand-edited page source set.
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "DELETE FROM memories WHERE source='file' AND source_id=?1",
            [ids[1].as_str()],
        )
        .await
        .unwrap();
    }
    assert_eq!(db.cleanup_orphaned_page_sources().await.unwrap(), 1);
    let pruned = db.get_page("topic").await.unwrap().unwrap();
    assert_eq!(pruned.source_memory_ids, ids[..1]);
    assert_eq!(db.get_page_sources("topic").await.unwrap().len(), 1);
    assert_eq!(pruned.sources_updated_count, 1);
    assert_eq!(
        db.get_page_source_revision("topic").await.unwrap(),
        revision_before + 1
    );
    let (run, fence) = complete_refresh_run(&db, &space, &ids[..1], 1).await;
    let pending = stage_refresh(&db, &ids[..1], Some(&fence), true).await;
    crate::post_write::accept_pending_revision(&db, &pending, "human-reviewer")
        .await
        .unwrap();
    let adopted = db.get_page("topic").await.unwrap().unwrap();
    assert_eq!(adopted.content, "human approved refreshed research");
    assert_eq!(
        db.research_run(&space, &run.id).await.unwrap().state,
        OrganizeRunState::Completed
    );
    assert_eq!(
        adopted.stale_reason.as_deref(),
        Some("source_removed"),
        "reviewing survivor A cannot acknowledge missing B"
    );
    assert_eq!(adopted.sources_updated_count, 1);
    assert!(adopted.review_required);
}

#[tokio::test]
async fn research_orphan_cleanup_preserves_existing_nonordinary_notice() {
    for reason in ["basis_changed", "source_removed"] {
        let (db, _temp) = test_db().await;
        let (_space, ids) = refresh_fixture(&db, 2, 2).await;
        {
            let conn = db.conn.lock().await;
            conn.execute("UPDATE pages SET stale_reason=?1,refresh_blocked_reason='already_blocked' WHERE id='topic'", [reason]).await.unwrap();
            conn.execute(
                "DELETE FROM memories WHERE source='file' AND source_id=?1",
                [ids[1].as_str()],
            )
            .await
            .unwrap();
        }
        assert_eq!(db.cleanup_orphaned_page_sources().await.unwrap(), 1);
        let page = db.get_page("topic").await.unwrap().unwrap();
        assert_eq!(page.stale_reason.as_deref(), Some(reason));
        assert_eq!(
            page.refresh_blocked_reason.as_deref(),
            Some("already_blocked")
        );
        assert_eq!(page.sources_updated_count, 3);
        assert_eq!(page.source_memory_ids, ids[..1]);
    }
}
