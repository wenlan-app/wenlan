// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::db::tests::test_db;
async fn seed(db: &MemoryDB, space: &str, id: &str) {
    let conn = db.conn.lock().await;
    conn.execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type) VALUES(?1,'file',?1,?1,'actual complete text',?2,0,1,'text')",libsql::params![id,space]).await.unwrap();
}
fn create(space: &str, ids: &[&str]) -> CreateOrganizeRunRequest {
    CreateOrganizeRunRequest {
        space: space.into(),
        sources: ids
            .iter()
            .map(|id| ResearchSourceRef {
                source: "file".into(),
                source_id: (*id).into(),
                title: "caller title ignored".into(),
            })
            .collect(),
        instructions: None,
        target_page_id: None,
    }
}
async fn complete(db: &MemoryDB, space: &str, mut value: OrganizeRun) -> OrganizeRun {
    for item in value.items.clone() {
        let read = db
            .read_organize_source(
                space,
                &value.id,
                ReadOrganizeSourceRequest {
                    space: "Research".into(),
                    source: item.source_ref.source.clone(),
                    source_id: item.source_ref.source_id.clone(),
                    offset: 0,
                    limit: 16000,
                },
            )
            .await
            .unwrap();
        assert!(read.complete);
        value.revision = read.run_revision;
        value = db
            .update_research_run(
                space,
                &value.id,
                UpdateOrganizeRunRequest {
                    space: space.into(),
                    expected_revision: value.revision,
                    source: Some(item.source_ref.source),
                    source_id: Some(item.source_ref.source_id),
                    coverage: Some(ResearchCoverage::Complete),
                    content_digest: Some(item.content_digest),
                    next_offset: None,
                    error: None,
                    cancel: false,
                    resume: false,
                },
            )
            .await
            .unwrap();
    }
    value
}
fn change(value: &OrganizeRun, index: usize) -> SourceClassificationChange {
    let item = &value.items[index];
    SourceClassificationChange {
        title: "untrusted".into(),
        before_tags: vec!["untrusted".into()],
        source: item.source_ref.source.clone(),
        source_id: item.source_ref.source_id.clone(),
        source_content_digest: item.content_digest.clone(),
        expected_decision_revision: item.decision_revision,
        add_tags: vec!["science".into()],
        remove_tags: vec![],
        reason: "Evidence classification".into(),
    }
}
#[tokio::test]
async fn research_rules_cas_and_run_restart_preserve_exact_scope() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "paper").await;
    let defaults = db.research_rules(&space.id).await.unwrap();
    assert_eq!(defaults.revision, 0);
    let saved = db
        .save_research_rules(
            &space.id,
            SaveOrganizeRulesRequest {
                space: "Research".into(),
                expected_revision: 0,
                name: "Methods".into(),
                instructions: "Compare methods".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert!(db
        .save_research_rules(
            &space.id,
            SaveOrganizeRulesRequest {
                space: "Research".into(),
                expected_revision: 0,
                name: "Stale".into(),
                instructions: "Wrong".into()
            }
        )
        .await
        .is_err());
    let value = db
        .create_research_run(&space.id, create("Research", &["paper"]))
        .await
        .unwrap();
    assert_eq!(value.rules, saved);
    assert_eq!(value.items[0].source_ref.title, "paper");
    assert_eq!(db.research_run(&space.id, &value.id).await.unwrap(), value);
    let other = db.create_space("Other", None, false).await.unwrap();
    assert!(db.research_run(&other.id, &value.id).await.is_err());
    assert!(db
        .create_research_run(&other.id, create("Other", &["paper"]))
        .await
        .is_err());
    assert_eq!(db.research_runs(&space.id).await.unwrap().runs.len(), 1);
    db.save_research_rules(
        &space.id,
        SaveOrganizeRulesRequest {
            space: "Research".into(),
            expected_revision: 1,
            name: "Future only".into(),
            instructions: "Different future rules".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        db.research_run(&space.id, &value.id).await.unwrap().rules,
        saved
    );
    drop(db);
    let reopened = libsql::Builder::new_local(_temp.path().join("origin_memory.db"))
        .build()
        .await
        .unwrap();
    let connection = reopened.connect().unwrap();
    let durable: OrganizeRun = load(&connection, &key("run", &(&space.id, &value.id)).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(durable, value);
    let old_rules: OrganizeRules = load(
        &connection,
        &key("rules-version", &(&space.id, 1u64)).unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(old_rules, saved);
}
#[tokio::test]
async fn research_dismissal_survives_new_run_and_presentation_changes() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "paper").await;
    let first = complete(
        &db,
        &space.id,
        db.create_research_run(&space.id, create("Research", &["paper"]))
            .await
            .unwrap(),
    )
    .await;
    let p = db
        .propose_research_classifications(
            &space.id,
            ProposeSourceClassificationsRequest {
                space: "Research".into(),
                run_id: first.id.clone(),
                changes: vec![change(&first, 0)],
            },
        )
        .await
        .unwrap();
    assert!(p.created);
    db.reject_research_classification(&p.id, &crate::read_scope::ReadScope::Global)
        .await
        .unwrap();
    let next = complete(
        &db,
        &space.id,
        db.create_research_run(&space.id, create("Research", &["paper"]))
            .await
            .unwrap(),
    )
    .await;
    let mut c = change(&next, 0);
    c.reason = "Different explanation".into();
    c.add_tags = vec![" SCIENCE ".into()];
    let again = db
        .propose_research_classifications(
            &space.id,
            ProposeSourceClassificationsRequest {
                space: "Research".into(),
                run_id: next.id,
                changes: vec![c],
            },
        )
        .await
        .unwrap();
    assert_eq!(again.id, p.id);
    assert!(!again.created);
    assert!(db
        .accept_research_classifications(
            &p.id,
            "review",
            None,
            &crate::read_scope::ReadScope::Global
        )
        .await
        .is_err());
}
#[tokio::test]
async fn research_classification_is_atomic_on_stale_human_decisions() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "one").await;
    seed(&db, "Research", "two").await;
    let value = complete(
        &db,
        &space.id,
        db.create_research_run(&space.id, create("Research", &["one", "two"]))
            .await
            .unwrap(),
    )
    .await;
    let proposal = db
        .propose_research_classifications(
            &space.id,
            ProposeSourceClassificationsRequest {
                space: "Research".into(),
                run_id: value.id.clone(),
                changes: vec![change(&value, 0), change(&value, 1)],
            },
        )
        .await
        .unwrap();
    db.patch_document_tag(
        "file",
        "two",
        "science",
        "remove",
        &crate::read_scope::ReadScope::Space("Research".into()),
    )
    .await
    .unwrap();
    let state = db
        .research_classification_decisions(&space.id, "file", "two")
        .await
        .unwrap();
    assert!(state
        .decisions
        .iter()
        .any(|d| d.tag == "science" && !d.present));
    assert!(db
        .accept_research_classifications(
            &proposal.id,
            "review",
            None,
            &crate::read_scope::ReadScope::Global
        )
        .await
        .is_err());
    assert!(db
        .get_document_tags("file", "one")
        .await
        .unwrap()
        .is_empty());
    assert_ne!(
        db.get_refinement_proposal(&proposal.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "resolved"
    );
}
#[tokio::test]
async fn research_review_merge_and_source_change_fences() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "paper").await;
    db.set_document_tags("file", "paper", vec!["old synonym".into()])
        .await
        .unwrap();
    let value = complete(
        &db,
        &space.id,
        db.create_research_run(&space.id, create("Research", &["paper"]))
            .await
            .unwrap(),
    )
    .await;
    let mut c = change(&value, 0);
    c.remove_tags = vec!["old synonym".into()];
    let p = db
        .propose_research_classifications(
            &space.id,
            ProposeSourceClassificationsRequest {
                space: "Research".into(),
                run_id: value.id,
                changes: vec![c],
            },
        )
        .await
        .unwrap();
    db.accept_research_classifications(
        &p.id,
        "review",
        None,
        &crate::read_scope::ReadScope::Global,
    )
    .await
    .unwrap();
    assert_eq!(
        db.get_document_tags("file", "paper").await.unwrap(),
        vec!["science"]
    );
    let state = db
        .research_classification_decisions(&space.id, "file", "paper")
        .await
        .unwrap();
    assert!(state
        .decisions
        .iter()
        .any(|d| d.tag == "old synonym" && !d.present));
    assert!(db
        .accept_research_classifications(
            &p.id,
            "review",
            None,
            &crate::read_scope::ReadScope::Global
        )
        .await
        .is_err());
}

#[tokio::test]
async fn research_manual_merge_rolls_back_foreign_scope_and_preserves_decisions() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    db.create_space("Other", None, false).await.unwrap();
    seed(&db, "Research", "one").await;
    seed(&db, "Other", "foreign").await;
    db.set_document_tags("file", "one", vec!["old".into(), "keep".into()])
        .await
        .unwrap();
    let request = MergeResearchTagsRequest {
        space: "Research".into(),
        sources: vec![
            ResearchSourceRef {
                source: "file".into(),
                source_id: "one".into(),
                title: String::new(),
            },
            ResearchSourceRef {
                source: "file".into(),
                source_id: "foreign".into(),
                title: String::new(),
            },
        ],
        from_tag: "old".into(),
        into_tag: "canonical".into(),
    };
    assert!(db
        .merge_research_tags(&space.id, request.clone())
        .await
        .is_err());
    assert_eq!(
        db.get_document_tags("file", "one").await.unwrap(),
        vec!["keep", "old"]
    );
    let mut request = request;
    request.sources.pop();
    db.merge_research_tags(&space.id, request).await.unwrap();
    assert_eq!(
        db.get_document_tags("file", "one").await.unwrap(),
        vec!["canonical", "keep"]
    );
    let state = db
        .research_classification_decisions(&space.id, "file", "one")
        .await
        .unwrap();
    assert!(state.decisions.iter().any(|d| d.tag == "old" && !d.present));
}
#[tokio::test]
async fn research_cancel_requires_explicit_resume_and_keeps_coverage() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "one").await;
    seed(&db, "Research", "two").await;
    let value = db
        .create_research_run(&space.id, create("Research", &["one", "two"]))
        .await
        .unwrap();
    let read = db
        .read_organize_source(
            &space.id,
            &value.id,
            ReadOrganizeSourceRequest {
                space: "Research".into(),
                source: "file".into(),
                source_id: "one".into(),
                offset: 0,
                limit: 16000,
            },
        )
        .await
        .unwrap();
    let update = UpdateOrganizeRunRequest {
        space: "Research".into(),
        expected_revision: read.run_revision,
        source: Some("file".into()),
        source_id: Some("one".into()),
        coverage: Some(ResearchCoverage::Complete),
        content_digest: Some(value.items[0].content_digest.clone()),
        next_offset: None,
        error: None,
        cancel: true,
        resume: false,
    };
    let cancelled = db
        .update_research_run(&space.id, &value.id, update)
        .await
        .unwrap();
    assert_eq!(cancelled.state, OrganizeRunState::Cancelled);
    let mut resume = UpdateOrganizeRunRequest {
        space: "Research".into(),
        expected_revision: cancelled.revision,
        source: None,
        source_id: None,
        coverage: None,
        content_digest: None,
        next_offset: None,
        error: None,
        cancel: false,
        resume: false,
    };
    assert!(db
        .update_research_run(&space.id, &value.id, resume.clone())
        .await
        .is_err());
    resume.resume = true;
    let resumed = db
        .update_research_run(&space.id, &value.id, resume)
        .await
        .unwrap();
    assert_eq!(resumed.items[0].coverage, ResearchCoverage::Complete);
    assert_eq!(resumed.items[1].coverage, ResearchCoverage::Pending);
    assert_eq!(resumed.rules, value.rules);
}

#[tokio::test]
async fn research_exact_source_kind_and_deleted_tag_decisions_remain_protected() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "paper").await;
    db.set_document_tags("file", "paper", vec!["curated".into()])
        .await
        .unwrap();
    assert_eq!(db.delete_tag("curated").await.unwrap(), 1);
    let decisions = db
        .research_classification_decisions(&space.id, "file", "paper")
        .await
        .unwrap();
    assert!(decisions
        .decisions
        .iter()
        .any(|d| d.tag == "curated" && !d.present));
    let conn = db.conn.lock().await;
    conn.execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type) VALUES('other-row','webpage','paper','Same opaque id','different','Research',0,1,'text')",()).await.unwrap();
    drop(conn);
    assert!(db
        .create_research_run(&space.id, create("Research", &["paper"]))
        .await
        .is_err());
}

#[tokio::test]
async fn research_classification_mutation_scope_is_checked_inside_transaction() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    db.create_space("Other", None, false).await.unwrap();
    seed(&db, "Research", "paper").await;
    let value = complete(
        &db,
        &space.id,
        db.create_research_run(&space.id, create("Research", &["paper"]))
            .await
            .unwrap(),
    )
    .await;
    let c = change(&value, 0);
    let proposal = db
        .propose_research_classifications(
            &space.id,
            ProposeSourceClassificationsRequest {
                space: "Research".into(),
                run_id: value.id,
                changes: vec![c.clone()],
            },
        )
        .await
        .unwrap();
    let wrong = crate::read_scope::ReadScope::Space("Other".into());
    assert!(db
        .accept_research_classifications(&proposal.id, "review", None, &wrong)
        .await
        .is_err());
    assert!(db
        .accept_research_classifications(&proposal.id, "review", Some(vec![c]), &wrong)
        .await
        .is_err());
    assert!(db
        .reject_research_classification(&proposal.id, &wrong)
        .await
        .is_err());
    assert!(db
        .get_document_tags("file", "paper")
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        db.get_refinement_proposal(&proposal.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "awaiting_review"
    );
    assert!(db
        .resolve_refinement_if_open(&proposal.id, "dismissed")
        .await
        .is_err());
    db.accept_research_classifications(
        &proposal.id,
        "review",
        None,
        &crate::read_scope::ReadScope::Space("Research".into()),
    )
    .await
    .unwrap();
    assert_eq!(
        db.get_document_tags("file", "paper").await.unwrap(),
        vec!["science".to_string()]
    );
}

#[tokio::test]
async fn research_complete_without_server_read_receipt_is_refused() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "paper").await;
    let value = db
        .create_research_run(&space.id, create("Research", &["paper"]))
        .await
        .unwrap();
    let result = db
        .update_research_run(
            &space.id,
            &value.id,
            UpdateOrganizeRunRequest {
                space: "Research".into(),
                expected_revision: value.revision,
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
        .await;
    assert!(
        result.is_err(),
        "echoing a published digest cannot prove source delivery"
    );
    let mut legacy = value.clone();
    legacy.items[0].coverage = ResearchCoverage::Complete;
    {
        let conn = db.conn.lock().await;
        save(
            &conn,
            &key("run", &(&space.id, &legacy.id)).unwrap(),
            &legacy,
        )
        .await
        .unwrap();
    }
    assert!(db
        .prepare_research_write(
            &legacy.id,
            &crate::read_scope::ReadScope::Space("Research".into()),
            None,
            &["paper".into()]
        )
        .await
        .is_err());
    assert!(db
        .propose_research_classifications(
            &space.id,
            ProposeSourceClassificationsRequest {
                space: "Research".into(),
                run_id: legacy.id.clone(),
                changes: vec![change(&legacy, 0)]
            }
        )
        .await
        .is_err());
    let payload = wenlan_types::RefinementPayload::SourceClassification {
        space_id: space.id.clone(),
        run_id: legacy.id.clone(),
        rules_digest: legacy.rules.content_digest.clone(),
        changes: vec![change(&legacy, 0)],
    };
    db.insert_refinement_proposal(
        "legacyproposal",
        "source_classification",
        &["paper".into()],
        Some(&serde_json::to_string(&payload).unwrap()),
        1.0,
    )
    .await
    .unwrap();
    assert!(db
        .accept_research_classifications(
            "legacyproposal",
            "review",
            None,
            &crate::read_scope::ReadScope::Space("Research".into())
        )
        .await
        .is_err());
}

fn source_read(offset: usize, limit: usize) -> ReadOrganizeSourceRequest {
    ReadOrganizeSourceRequest {
        space: "Research".into(),
        source: "file".into(),
        source_id: "paper".into(),
        offset,
        limit,
    }
}
fn completion_update(value: &OrganizeRun, revision: u64) -> UpdateOrganizeRunRequest {
    UpdateOrganizeRunRequest {
        space: "Research".into(),
        expected_revision: revision,
        source: Some("file".into()),
        source_id: Some("paper".into()),
        coverage: Some(ResearchCoverage::Complete),
        content_digest: Some(value.items[0].content_digest.clone()),
        next_offset: None,
        error: None,
        cancel: false,
        resume: false,
    }
}
#[tokio::test]
async fn research_read_receipt_unicode_prefix_retry_restart_and_tail_gap() {
    let (db, temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "paper").await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET content='甲🙂abé終' WHERE id='paper'",
            (),
        )
        .await
        .unwrap();
    }
    let value = db
        .create_research_run(&space.id, create("Research", &["paper"]))
        .await
        .unwrap();
    assert!(db
        .read_organize_source(&space.id, &value.id, source_read(0, 16001))
        .await
        .is_err());
    assert!(db
        .read_organize_source(&space.id, &value.id, source_read(4, 1))
        .await
        .is_err());
    let first = db
        .read_organize_source(&space.id, &value.id, source_read(0, 2))
        .await
        .unwrap();
    assert_eq!(
        (
            first.text.as_str(),
            first.total_chars,
            first.served_chars,
            first.next_offset,
            first.complete
        ),
        ("甲🙂", 6, 2, Some(2), false)
    );
    assert!(db
        .update_research_run(
            &space.id,
            &value.id,
            completion_update(&value, first.run_revision)
        )
        .await
        .is_err());
    let retry = db
        .read_organize_source(&space.id, &value.id, source_read(1, 1))
        .await
        .unwrap();
    assert_eq!(retry.text, "🙂");
    assert_eq!(retry.served_chars, 2);
    assert_eq!(retry.run_revision, first.run_revision);
    assert!(db
        .read_organize_source(&space.id, &value.id, source_read(5, 1))
        .await
        .is_err());
    let middle = db
        .read_organize_source(&space.id, &value.id, source_read(2, 2))
        .await
        .unwrap();
    assert_eq!(middle.text, "ab");
    assert_eq!(middle.served_chars, 4);
    let id = value.id.clone();
    drop(db);
    let reopened = MemoryDB::new(temp.path(), std::sync::Arc::new(crate::events::NoopEmitter))
        .await
        .unwrap();
    let resumed = reopened.research_run(&space.id, &id).await.unwrap();
    assert_eq!(resumed.items[0].served_chars, 4);
    let tail = reopened
        .read_organize_source(&space.id, &id, source_read(4, 16000))
        .await
        .unwrap();
    assert_eq!(tail.text, "é終");
    assert!(tail.complete);
    assert_eq!(tail.served_chars, 6);
    let completed = reopened
        .update_research_run(&space.id, &id, completion_update(&value, tail.run_revision))
        .await
        .unwrap();
    assert_eq!(completed.items[0].coverage, ResearchCoverage::Complete);
    assert_eq!(completed.items[0].served_total_chars, Some(6));
}
#[tokio::test]
async fn research_read_receipt_source_generation_scope_and_terminal_fences() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    let other = db.create_space("Other", None, false).await.unwrap();
    seed(&db, "Research", "paper").await;
    let value = db
        .create_research_run(&space.id, create("Research", &["paper"]))
        .await
        .unwrap();
    assert!(db
        .read_organize_source(
            &other.id,
            &value.id,
            ReadOrganizeSourceRequest {
                space: "Other".into(),
                ..source_read(0, 2)
            }
        )
        .await
        .is_err());
    assert!(db
        .read_organize_source(
            &space.id,
            &value.id,
            ReadOrganizeSourceRequest {
                space: "Other".into(),
                ..source_read(0, 2)
            }
        )
        .await
        .is_err());
    assert!(db
        .read_organize_source(
            &space.id,
            &value.id,
            ReadOrganizeSourceRequest {
                source: "webpage".into(),
                ..source_read(0, 2)
            }
        )
        .await
        .is_err());
    let full = db
        .read_organize_source(&space.id, &value.id, source_read(0, 16000))
        .await
        .unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET content='changed generation' WHERE id='paper'",
            (),
        )
        .await
        .unwrap();
    }
    assert!(db
        .read_organize_source(&space.id, &value.id, source_read(0, 2))
        .await
        .is_err());
    assert!(db
        .update_research_run(
            &space.id,
            &value.id,
            completion_update(&value, full.run_revision)
        )
        .await
        .is_err());
    let fresh = db
        .create_research_run(&space.id, create("Research", &["paper"]))
        .await
        .unwrap();
    let mut cancel = completion_update(&fresh, fresh.revision);
    cancel.coverage = None;
    cancel.cancel = true;
    let cancelled = db
        .update_research_run(&space.id, &fresh.id, cancel.clone())
        .await
        .unwrap();
    assert!(db
        .read_organize_source(&space.id, &fresh.id, source_read(0, 2))
        .await
        .is_err());
    cancel.expected_revision = cancelled.revision;
    cancel.cancel = false;
    cancel.resume = true;
    db.update_research_run(&space.id, &fresh.id, cancel)
        .await
        .unwrap();
    let partial = db
        .read_organize_source(&space.id, &fresh.id, source_read(0, 2))
        .await
        .unwrap();
    assert_eq!(partial.served_chars, 2);
    let mut frozen = db.research_run(&space.id, &fresh.id).await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute("UPDATE memories SET space='Other' WHERE id='paper'", ())
            .await
            .unwrap();
    }
    assert!(db
        .read_organize_source(&space.id, &fresh.id, source_read(2, 2))
        .await
        .is_err());
    {
        let conn = db.conn.lock().await;
        conn.execute("UPDATE memories SET space='Research' WHERE id='paper'", ())
            .await
            .unwrap();
        frozen.state = OrganizeRunState::AwaitingReview;
        save(
            &conn,
            &key("run", &(&space.id, &fresh.id)).unwrap(),
            &frozen,
        )
        .await
        .unwrap();
    }
    assert!(db
        .read_organize_source(&space.id, &fresh.id, source_read(2, 2))
        .await
        .is_err());
    assert!(serde_json::from_value::<UpdateOrganizeRunRequest>(
        serde_json::json!({"space":"Research","expected_revision":1,"served_chars":9999})
    )
    .is_err());
}

#[tokio::test]
async fn research_empty_indexed_source_never_gets_successful_coverage_receipt() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "paper").await;
    {
        let conn = db.conn.lock().await;
        conn.execute("UPDATE memories SET content='' WHERE id='paper'", ())
            .await
            .unwrap();
    }
    let value = db
        .create_research_run(&space.id, create("Research", &["paper"]))
        .await
        .unwrap();
    assert!(db
        .read_organize_source(&space.id, &value.id, source_read(0, 16000))
        .await
        .is_err());
    let stored = db.research_run(&space.id, &value.id).await.unwrap();
    assert_eq!(stored.revision, value.revision);
    assert_eq!(stored.items[0].served_total_chars, None);
    assert!(db
        .update_research_run(
            &space.id,
            &value.id,
            completion_update(&value, value.revision)
        )
        .await
        .is_err());
    let mut legacy = value.clone();
    legacy.items[0].served_total_chars = Some(0);
    legacy.items[0].coverage = ResearchCoverage::Complete;
    assert!(!complete_receipt(&legacy.items[0]));
    {
        let conn = db.conn.lock().await;
        save(
            &conn,
            &key("run", &(&space.id, &legacy.id)).unwrap(),
            &legacy,
        )
        .await
        .unwrap();
    }
    assert!(db
        .prepare_research_write(
            &legacy.id,
            &crate::read_scope::ReadScope::Space("Research".into()),
            None,
            &["paper".into()]
        )
        .await
        .is_err());
}

#[tokio::test]
async fn research_classification_snapshots_current_title_without_rewriting_existing_decisions() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "paper").await;
    let value = complete(
        &db,
        &space.id,
        db.create_research_run(&space.id, create("Research", &["paper"]))
            .await
            .unwrap(),
    )
    .await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET title='Current at staging' WHERE id='paper'",
            (),
        )
        .await
        .unwrap();
    }
    let request = ProposeSourceClassificationsRequest {
        space: "Research".into(),
        run_id: value.id.clone(),
        changes: vec![change(&value, 0)],
    };
    let first = db
        .propose_research_classifications(&space.id, request.clone())
        .await
        .unwrap();
    let original = db
        .get_refinement_proposal(&first.id)
        .await
        .unwrap()
        .unwrap();
    let payload: wenlan_types::RefinementPayload =
        serde_json::from_str(original.payload.as_ref().unwrap()).unwrap();
    assert!(
        matches!(payload,wenlan_types::RefinementPayload::SourceClassification {changes,..} if changes[0].title=="Current at staging")
    );
    assert_eq!(value.items[0].source_ref.title, "paper");
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET title='Later metadata' WHERE id='paper'",
            (),
        )
        .await
        .unwrap();
    }
    let again = db
        .propose_research_classifications(&space.id, request.clone())
        .await
        .unwrap();
    assert!(!again.created);
    assert_eq!(again.id, first.id);
    assert_eq!(
        db.get_refinement_proposal(&first.id)
            .await
            .unwrap()
            .unwrap()
            .payload,
        original.payload
    );
    db.reject_research_classification(
        &first.id,
        &crate::read_scope::ReadScope::Space("Research".into()),
    )
    .await
    .unwrap();
    let terminal = db
        .propose_research_classifications(&space.id, request)
        .await
        .unwrap();
    assert!(!terminal.created);
    let saved = db
        .get_refinement_proposal(&first.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.status, "dismissed");
    assert_eq!(saved.payload, original.payload);
}
