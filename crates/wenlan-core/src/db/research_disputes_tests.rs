// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::db::tests::test_db;
fn evidence(id: &str) -> ResearchDisputeEvidence {
    ResearchDisputeEvidence {
        source: "file".into(),
        source_id: id.into(),
        source_digest: "a".repeat(64),
        quote: "actual complete text".into(),
        pdf_target: None,
        source_lifecycle: None,
    }
}
fn proposal() -> ResearchDispute {
    ResearchDispute {
        space_id: "space-id".into(),
        kind: ResearchDisputeKind::OpposingClaims,
        conditions: "same population".into(),
        reason: "Compare the original claims".into(),
        evidence: vec![evidence("paper-a"), evidence("paper-b")],
    }
}
#[test]
fn canonical_identity_tracks_evidence_and_conditions_not_presentation() {
    let original = proposal();
    let (_, id) = canonicalize(original.clone()).unwrap();
    let mut rerun = original.clone();
    rerun.reason = "New presentation".into();
    rerun.evidence.reverse();
    rerun.conditions = " same\n population ".into();
    rerun.evidence[0].quote = "actual\ncomplete text".into();
    assert_eq!(canonicalize(rerun).unwrap().1, id);
    let mut changed = original.clone();
    changed.conditions = "another population".into();
    assert_ne!(canonicalize(changed).unwrap().1, id);
    let mut changed = original.clone();
    changed.evidence[0].source_digest = "b".repeat(64);
    assert_ne!(canonicalize(changed).unwrap().1, id);
    let mut pdf = original.clone();
    pdf.evidence[0].pdf_target = Some(wenlan_types::pages::PdfCitationTarget {
        attachment_key: "ATTACH01".into(),
        pdf_digest: "c".repeat(64),
        page: 1,
        quote: pdf.evidence[0].quote.clone(),
    });
    let pdfid = canonicalize(pdf.clone()).unwrap().1;
    assert_ne!(pdfid, id);
    pdf.evidence[0].pdf_target.as_mut().unwrap().pdf_digest = "d".repeat(64);
    assert_ne!(canonicalize(pdf.clone()).unwrap().1, pdfid);
    pdf.evidence[0].pdf_target.as_mut().unwrap().pdf_digest = "c".repeat(64);
    pdf.evidence[0].pdf_target.as_mut().unwrap().page = 2;
    assert_ne!(canonicalize(pdf).unwrap().1, pdfid);
    let mut duplicate = original;
    duplicate.evidence[1] = duplicate.evidence[0].clone();
    assert!(canonicalize(duplicate).is_err());
}
async fn seed(db: &MemoryDB, space: &str, id: &str) {
    db.conn.lock().await.execute("INSERT INTO memories(id,source,source_id,title,content,space,chunk_index,last_modified,chunk_type) VALUES(?1,'file',?1,?1,'actual complete text',?2,0,1,'text')",libsql::params![id,space]).await.unwrap();
}
async fn request(db: &MemoryDB, space_id: &str) -> ProposeResearchDisputeRequest {
    let conn = db.conn.lock().await;
    let mut evidence = vec![evidence("paper-a"), evidence("paper-b")];
    for e in &mut evidence {
        e.source_digest = research_runs::source_snapshot(&conn, space_id, &e.source, &e.source_id)
            .await
            .unwrap()
            .1;
    }
    ProposeResearchDisputeRequest {
        space: "Research".into(),
        kind: ResearchDisputeKind::OpposingClaims,
        conditions: "same population".into(),
        reason: "Compare the original claims".into(),
        evidence,
    }
}
fn decision(digest: &str, decision: ResearchDisputeDecision) -> DecideResearchDisputeRequest {
    DecideResearchDisputeRequest {
        space: "Research".into(),
        expected_payload_digest: digest.into(),
        decision,
        reason: "I checked the conditions".into(),
    }
}
async fn timestamp(db: &MemoryDB, id: &str) -> String {
    let conn = db.conn.lock().await;
    let mut rows = conn
        .query(
            "SELECT CAST(created_at AS TEXT) FROM refinement_queue WHERE id=?1",
            [id],
        )
        .await
        .unwrap();
    rows.next().await.unwrap().unwrap().get(0).unwrap()
}
#[tokio::test]
async fn durable_decisions_cas_rerun_and_defer_leave_original_queue_intact() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "paper-a").await;
    seed(&db, "Research", "paper-b").await;
    let req = request(&db, &space.id).await;
    let staged = db
        .propose_research_dispute(&space.id, req.clone())
        .await
        .unwrap();
    let created = timestamp(&db, &staged.id).await;
    let deferred = db
        .decide_research_dispute(
            &space.id,
            &staged.id,
            decision(&staged.payload_digest, ResearchDisputeDecision::Defer),
            "user",
        )
        .await
        .unwrap();
    assert_eq!(deferred.status, "awaiting_review");
    assert_eq!(deferred.decisions.len(), 1);
    let again = db
        .decide_research_dispute(
            &space.id,
            &staged.id,
            decision(&staged.payload_digest, ResearchDisputeDecision::Defer),
            "user",
        )
        .await
        .unwrap();
    assert_eq!(again.decisions, deferred.decisions);
    assert_eq!(timestamp(&db, &staged.id).await, created);
    assert!(matches!(
        db.decide_research_dispute(
            &space.id,
            &staged.id,
            decision("wrong", ResearchDisputeDecision::Close),
            "user"
        )
        .await,
        Err(WenlanError::Conflict(_))
    ));
    let closed = db
        .decide_research_dispute(
            &space.id,
            &staged.id,
            decision(&staged.payload_digest, ResearchDisputeDecision::Close),
            "user",
        )
        .await
        .unwrap();
    assert_eq!(closed.status, "resolved");
    let mut rerun = req.clone();
    rerun.reason = "Other wording".into();
    let duplicate = db.propose_research_dispute(&space.id, rerun).await.unwrap();
    assert!(!duplicate.created);
    assert_eq!(duplicate.payload_digest, staged.payload_digest);
    assert_eq!(
        db.research_dispute(&space.id, &staged.id)
            .await
            .unwrap()
            .dispute
            .reason,
        req.reason
    );
    let mut changed = req;
    changed.conditions = "different experimental temperature".into();
    assert!(
        db.propose_research_dispute(&space.id, changed)
            .await
            .unwrap()
            .created
    );
    let renamed = db.update_space("Research", "Renamed", None).await.unwrap();
    assert_eq!(renamed.id, space.id);
    assert_eq!(
        db.research_dispute(&space.id, &staged.id)
            .await
            .unwrap()
            .space,
        "Renamed"
    );
}
#[tokio::test]
async fn stale_confirmation_fails_but_close_records_changed_and_missing_sources() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "paper-a").await;
    seed(&db, "Research", "paper-b").await;
    let req = request(&db, &space.id).await;
    let staged = db.propose_research_dispute(&space.id, req).await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE memories SET content='changed findings' WHERE source_id='paper-a'",
            (),
        )
        .await
        .unwrap();
        conn.execute("DELETE FROM memories WHERE source_id='paper-b'", ())
            .await
            .unwrap();
    }
    assert!(matches!(
        db.decide_research_dispute(
            &space.id,
            &staged.id,
            decision(
                &staged.payload_digest,
                ResearchDisputeDecision::ConfirmDifference
            ),
            "user"
        )
        .await,
        Err(WenlanError::Conflict(_))
    ));
    assert!(db
        .research_dispute(&space.id, &staged.id)
        .await
        .unwrap()
        .decisions
        .is_empty());
    let closed = db
        .decide_research_dispute(
            &space.id,
            &staged.id,
            decision(&staged.payload_digest, ResearchDisputeDecision::Close),
            "user",
        )
        .await
        .unwrap();
    assert_eq!(closed.decisions.len(), 1);
    assert!(closed
        .source_states
        .iter()
        .any(|s| s.state == ResearchEvidenceState::Changed));
    assert!(closed
        .source_states
        .iter()
        .any(|s| s.state == ResearchEvidenceState::Unavailable));
    assert_eq!(closed.decisions[0].source_states, closed.source_states);
}
#[tokio::test]
async fn exact_quote_space_and_resolution_history_are_atomic() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    let other = db.create_space("Other", None, false).await.unwrap();
    seed(&db, "Research", "paper-a").await;
    seed(&db, "Research", "paper-b").await;
    let mut req = request(&db, &space.id).await;
    req.evidence[0].quote = "invented semantic summary".into();
    assert!(db.propose_research_dispute(&space.id, req).await.is_err());
    seed(&db, "Other", "foreign-paper").await;
    let mut foreign = request(&db, &space.id).await;
    foreign.evidence[0].source_id = "foreign-paper".into();
    assert!(db
        .propose_research_dispute(&space.id, foreign)
        .await
        .is_err());
    let req = request(&db, &space.id).await;
    assert!(db
        .propose_research_dispute(&other.id, req.clone())
        .await
        .is_err());
    let staged = db.propose_research_dispute(&space.id, req).await.unwrap();
    assert!(db.research_dispute(&other.id, &staged.id).await.is_err());
    db.conn.lock().await.execute("CREATE TRIGGER fail_dispute_resolution BEFORE UPDATE OF status ON refinement_queue WHEN NEW.action='research_dispute' BEGIN SELECT RAISE(ABORT,'test resolution failure'); END",()).await.unwrap();
    assert!(db
        .decide_research_dispute(
            &space.id,
            &staged.id,
            decision(
                &staged.payload_digest,
                ResearchDisputeDecision::ConfirmDifference
            ),
            "user"
        )
        .await
        .is_err());
    let intact = db.research_dispute(&space.id, &staged.id).await.unwrap();
    assert_eq!(intact.status, "awaiting_review");
    assert!(intact.decisions.is_empty());
}

#[tokio::test]
async fn lifecycle_change_without_byte_change_refuses_fresh_dispute_decisions() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "paper-a").await;
    seed(&db, "Research", "paper-b").await;
    let staged = db
        .propose_research_dispute(&space.id, request(&db, &space.id).await)
        .await
        .unwrap();
    db.set_source_lifecycle(
        &space.id,
        wenlan_types::source_lifecycle::SetSourceLifecycleRequest {
            space: "Research".into(),
            source: "file".into(),
            source_id: "paper-a".into(),
            expected_revision: 0,
            state: wenlan_types::source_lifecycle::SourceLifecycleState::Retracted,
            reason: "Publisher retraction".into(),
        },
        "human",
    )
    .await
    .unwrap();
    for choice in [
        ResearchDisputeDecision::ConfirmDifference,
        ResearchDisputeDecision::NotComparable,
        ResearchDisputeDecision::CorrectInterpretation,
    ] {
        assert!(
            matches!(
                db.decide_research_dispute(
                    &space.id,
                    &staged.id,
                    decision(&staged.payload_digest, choice),
                    "human"
                )
                .await,
                Err(WenlanError::Conflict(_))
            ),
            "same bytes must not hide a changed/inactive lifecycle"
        );
    }
    let view = db.research_dispute(&space.id, &staged.id).await.unwrap();
    assert!(view
        .source_states
        .iter()
        .any(|state| state.state != ResearchEvidenceState::Unchanged));
    let deferred = db
        .decide_research_dispute(
            &space.id,
            &staged.id,
            decision(&staged.payload_digest, ResearchDisputeDecision::Defer),
            "human",
        )
        .await
        .unwrap();
    assert_eq!(deferred.status, "awaiting_review");
    let closed = db
        .decide_research_dispute(
            &space.id,
            &staged.id,
            decision(&staged.payload_digest, ResearchDisputeDecision::Close),
            "human",
        )
        .await
        .unwrap();
    assert_eq!(closed.status, "resolved");
}

#[tokio::test]
async fn lifecycle_capture_is_authoritative_and_reactivation_requires_new_proposal() {
    use wenlan_types::source_lifecycle::{
        SetSourceLifecycleRequest, SourceLifecycleState, SourceLifecycleSummary,
    };
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "paper-a").await;
    seed(&db, "Research", "paper-b").await;
    let old = db
        .propose_research_dispute(&space.id, request(&db, &space.id).await)
        .await
        .unwrap();
    let change = |state, expected_revision| SetSourceLifecycleRequest {
        space: "Research".into(),
        source: "file".into(),
        source_id: "paper-a".into(),
        state,
        expected_revision,
        reason: "Explicit human decision".into(),
    };
    db.set_source_lifecycle(
        &space.id,
        change(SourceLifecycleState::Excluded, 0),
        "human",
    )
    .await
    .unwrap();
    let historical = db
        .propose_research_dispute(&space.id, request(&db, &space.id).await)
        .await
        .unwrap();
    assert_ne!(
        old.id, historical.id,
        "lifecycle generation is part of immutable proposal identity"
    );
    let view = db
        .research_dispute(&space.id, &historical.id)
        .await
        .unwrap();
    assert!(view
        .source_states
        .iter()
        .any(|s| s.state == ResearchEvidenceState::Historical));
    assert!(matches!(
        db.decide_research_dispute(
            &space.id,
            &historical.id,
            decision(
                &historical.payload_digest,
                ResearchDisputeDecision::ConfirmDifference
            ),
            "human"
        )
        .await,
        Err(WenlanError::Conflict(_))
    ));
    db.set_source_lifecycle(&space.id, change(SourceLifecycleState::Active, 1), "human")
        .await
        .unwrap();
    let old_view = db.research_dispute(&space.id, &old.id).await.unwrap();
    assert!(old_view
        .source_states
        .iter()
        .any(|s| s.state == ResearchEvidenceState::Changed));
    let mut refreshed = request(&db, &space.id).await;
    for e in &mut refreshed.evidence {
        e.source_lifecycle = Some(SourceLifecycleSummary {
            revision: 999,
            state: SourceLifecycleState::Retracted,
            reason: "Untrusted caller".into(),
        });
    }
    let fresh = db
        .propose_research_dispute(&space.id, refreshed)
        .await
        .unwrap();
    assert_ne!(fresh.id, old.id);
    let view = db.research_dispute(&space.id, &fresh.id).await.unwrap();
    let a = view
        .dispute
        .evidence
        .iter()
        .find(|e| e.source_id == "paper-a")
        .unwrap()
        .source_lifecycle
        .as_ref()
        .unwrap();
    assert_eq!(a.revision, 2);
    assert_eq!(a.state, SourceLifecycleState::Active);
    assert!(view
        .source_states
        .iter()
        .all(|s| s.state == ResearchEvidenceState::Unchanged));
    db.decide_research_dispute(
        &space.id,
        &fresh.id,
        decision(
            &fresh.payload_digest,
            ResearchDisputeDecision::ConfirmDifference,
        ),
        "human",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn legacy_dispute_without_lifecycle_generation_is_readable_but_cannot_confirm() {
    let (db, _temp) = test_db().await;
    let space = db.create_space("Research", None, false).await.unwrap();
    seed(&db, "Research", "paper-a").await;
    seed(&db, "Research", "paper-b").await;
    let staged = db
        .propose_research_dispute(&space.id, request(&db, &space.id).await)
        .await
        .unwrap();
    let legacy_digest = {
        let conn = db.conn.lock().await;
        let (mut stored, _, _) = load_on(&conn, &space.id, &staged.id).await.unwrap();
        for e in &mut stored.dispute.evidence {
            e.source_lifecycle = None;
        }
        stored.payload_digest = digest(&stored.dispute).unwrap();
        let raw = serde_json::to_string(&stored).unwrap();
        assert!(
            !raw.contains("source_lifecycle"),
            "legacy JSON is preserved without inventing revision zero"
        );
        conn.execute(
            "UPDATE refinement_queue SET payload=?1 WHERE id=?2",
            libsql::params![raw, staged.id.as_str()],
        )
        .await
        .unwrap();
        stored.payload_digest
    };
    let view = db.research_dispute(&space.id, &staged.id).await.unwrap();
    assert!(view
        .source_states
        .iter()
        .all(|s| s.expected_lifecycle.is_none() && s.state == ResearchEvidenceState::Changed));
    assert!(matches!(
        db.decide_research_dispute(
            &space.id,
            &staged.id,
            decision(
                &legacy_digest,
                ResearchDisputeDecision::CorrectInterpretation
            ),
            "human"
        )
        .await,
        Err(WenlanError::Conflict(_))
    ));
    db.decide_research_dispute(
        &space.id,
        &staged.id,
        decision(&legacy_digest, ResearchDisputeDecision::Close),
        "human",
    )
    .await
    .unwrap();
}
