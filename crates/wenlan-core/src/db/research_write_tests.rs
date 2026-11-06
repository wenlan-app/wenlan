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
