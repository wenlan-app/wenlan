// SPDX-License-Identifier: Apache-2.0
//! DB-backed tests for sentence authorship, proposals, the sentence write gate
//! and per-sentence citation checks.

use super::*;
use crate::events::NoopEmitter;
use crate::post_write::{page_write, PageWrite, WriteOutcome, WriteResult};
use std::sync::Arc;
use wenlan_types::authorship::{
    AiEditMode, EvidenceState, SentenceAuthor, SentenceProposalStatus, SentenceProposeRequest,
    WeakReason,
};
use wenlan_types::requests::UpdatePageRequest;

async fn test_db() -> (MemoryDB, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(&dir.path().join("test.db"), Arc::new(NoopEmitter))
        .await
        .unwrap();
    (db, dir)
}

async fn seed_memory(db: &MemoryDB, source_id: &str, content: &str) {
    db.upsert_documents(vec![crate::sources::RawDocument {
        source: "memory".to_string(),
        source_id: source_id.to_string(),
        title: source_id.to_string(),
        content: content.to_string(),
        last_modified: chrono::Utc::now().timestamp(),
        memory_type: Some("fact".to_string()),
        source_agent: Some("test".to_string()),
        confidence: Some(0.9),
        ..Default::default()
    }])
    .await
    .unwrap();
}

const AI_BODY: &str = "Tokio runs async tasks. Workers steal queued tasks.";

/// A machine-owned page (`create` by `distilled`) cited by one memory.
async fn ai_page(db: &MemoryDB, id: &str, citations: Option<&str>) -> Page {
    seed_memory(
        db,
        "mem-tokio",
        "Tokio runs async tasks and workers steal queued tasks",
    )
    .await;
    db.insert_page_with_kind(
        id,
        "Tokio",
        None,
        AI_BODY,
        None,
        None,
        &["mem-tokio"],
        &chrono::Utc::now().to_rfc3339(),
        "distilled",
        "confirmed",
        None,
        citations,
    )
    .await
    .unwrap();
    db.get_page(id).await.unwrap().unwrap()
}

async fn write(
    db: &MemoryDB,
    id: &str,
    content: &str,
    edited_by: &str,
    expected_version: Option<i64>,
) -> WriteResult {
    page_write(
        db,
        PageWrite::Update {
            page_id: id,
            req: UpdatePageRequest {
                content: content.to_string(),
                source_memory_ids: vec!["mem-tokio".to_string()],
                expected_version,
                caller_id: None,
                operation_id: None,
            },
            edited_by,
            require_stale: false,
            expected_source_revision: None,
            knowledge_path: None,
            citations: None,
        },
    )
    .await
    .unwrap()
}

/// The AI page after the person added a sentence of their own.
async fn page_with_my_line(db: &MemoryDB, id: &str) -> Page {
    ai_page(db, id, None).await;
    let mine = format!("{AI_BODY} I prefer the current-thread runtime.");
    let result = write(db, id, &mine, "manual_edit", None).await;
    assert!(result.wrote);
    db.get_page(id).await.unwrap().unwrap()
}

/// The AI page after the person added a cited sentence whose citation was
/// checked and found unsupported: its facts changed.
async fn page_with_my_unsupported_line(db: &MemoryDB, id: &str) -> Page {
    ai_page(db, id, None).await;
    let mine = format!("{AI_BODY} I prefer the current-thread runtime.[1]");
    assert!(write(db, id, &mine, "manual_edit", None).await.wrote);
    let page = db.get_page(id).await.unwrap().unwrap();
    assert!(db
        .set_page_citations_at_page_version(id, &citation("mem-tokio", "unverified"), page.version)
        .await
        .unwrap());
    db.get_page(id).await.unwrap().unwrap()
}

fn authors(view: &PageSentencesResponse) -> Vec<SentenceAuthor> {
    view.sentences.iter().map(|s| s.author).collect()
}

#[tokio::test]
async fn sentences_view_tells_the_persons_sentences_from_the_ais() {
    let (db, _dir) = test_db().await;
    page_with_my_line(&db, "p").await;
    let view = page_sentences(&db, "p").await.unwrap();
    assert_eq!(
        authors(&view),
        [
            SentenceAuthor::Ai,
            SentenceAuthor::Ai,
            SentenceAuthor::Person
        ]
    );
    assert_eq!(view.ai_edits, AiEditMode::Propose, "the default is propose");
    assert!(view.sentences[2].protected && !view.sentences[0].protected);
    assert_eq!(
        view.sentences[2].weak,
        [],
        "the person's own words need no citation"
    );
    assert_eq!(
        view.sentences[0].weak,
        [WeakReason::NoCitation],
        "an AI sentence without a citation is weak"
    );
    assert_eq!(view.sentences[0].evidence, EvidenceState::None);
}

#[tokio::test]
async fn a_rewording_of_the_persons_sentence_is_dropped_not_proposed() {
    // 只在事实变了时: the person's sentence has no citation problem, so the
    // AI's rewording of it is not a proposal; the sentence stays as written.
    let (db, _dir) = test_db().await;
    let before = page_with_my_line(&db, "p").await;
    let rewrite = "Tokio runs async tasks on a pool. Workers steal queued tasks. \
                   The multi-thread runtime is better for everyone.";
    let result = write(&db, "p", rewrite, "agent_refresh", Some(before.version)).await;
    assert!(result.wrote, "the AI's own sentence change lands");
    assert!(result.sentence_proposals.is_empty());
    let after = db.get_page("p").await.unwrap().unwrap();
    assert_eq!(
        after.content,
        "Tokio runs async tasks on a pool. Workers steal queued tasks. \
         I prefer the current-thread runtime."
    );
    assert!(page_sentences(&db, "p").await.unwrap().proposals.is_empty());
}

#[tokio::test]
async fn a_version_checked_agent_write_lands_only_on_the_ais_sentences() {
    let (db, _dir) = test_db().await;
    let before = page_with_my_unsupported_line(&db, "p").await;
    let rewrite = "Tokio runs async tasks on a pool. Workers steal queued tasks. \
                   The multi-thread runtime is better for everyone.";
    let result = write(&db, "p", rewrite, "agent_refresh", Some(before.version)).await;
    assert!(result.wrote, "the AI's own sentence change lands");
    assert_eq!(result.sentence_proposals.len(), 1);
    let after = db.get_page("p").await.unwrap().unwrap();
    assert_eq!(
        after.content,
        "Tokio runs async tasks on a pool. Workers steal queued tasks. \
         I prefer the current-thread runtime.[1]"
    );
    assert!(after.user_edited, "the page is still the person's");

    let view = page_sentences(&db, "p").await.unwrap();
    assert_eq!(view.proposals.len(), 1);
    let proposal = &view.proposals[0];
    assert_eq!(proposal.before, "I prefer the current-thread runtime.[1]");
    assert_eq!(
        proposal.reason,
        Some(WeakReason::Unsupported),
        "the facts changed: its source does not back it"
    );
    assert_eq!(
        proposal.after,
        "The multi-thread runtime is better for everyone."
    );
    assert_eq!(proposal.actor.name, "agent_refresh");
    assert_eq!(
        view.sentences[2].proposals,
        std::slice::from_ref(&proposal.id)
    );

    // Retrying the same write stages nothing twice.
    let again = write(&db, "p", rewrite, "agent_refresh", Some(after.version)).await;
    assert_eq!(again.outcome, WriteOutcome::Unchanged);
    assert!(again.sentence_proposals.is_empty());
    assert_eq!(page_sentences(&db, "p").await.unwrap().proposals.len(), 1);
}

#[tokio::test]
async fn a_machine_rewrite_keeps_the_citations_on_the_persons_sentence() {
    let (db, _dir) = test_db().await;
    ai_page(&db, "p", None).await;
    seed_memory(
        &db,
        "mem-x",
        "The current-thread runtime suits small services",
    )
    .await;
    seed_memory(&db, "mem-ai", "Tokio workers are cheap to spawn").await;
    let mine = format!("{AI_BODY} I prefer the current-thread runtime.[1]");
    assert!(write(&db, "p", &mine, "manual_edit", None).await.wrote);
    let before = db.get_page("p").await.unwrap().unwrap();
    assert!(db
        .set_page_citations_at_page_version("p", &citation("mem-x", "unverified"), before.version)
        .await
        .unwrap());

    // The AI changes the person's cited sentence and adds a cited one of its
    // own; its marker [1] is its own source, mem-ai.
    let rewrite = "Tokio runs async tasks. Workers steal queued tasks. \
                   The multi-thread runtime is better for everyone. \
                   Workers are cheap to spawn.[1]";
    let result = page_write(
        &db,
        PageWrite::Update {
            page_id: "p",
            req: UpdatePageRequest {
                content: rewrite.to_string(),
                source_memory_ids: vec!["mem-ai".to_string()],
                expected_version: Some(before.version),
                caller_id: None,
                operation_id: None,
            },
            edited_by: "agent_refresh",
            require_stale: false,
            expected_source_revision: None,
            knowledge_path: None,
            citations: Some((citation("mem-ai", "verified"), "1 cited".to_string())),
        },
    )
    .await
    .unwrap();
    assert!(result.wrote);
    assert_eq!(
        result.sentence_proposals.len(),
        1,
        "the change is a proposal"
    );

    let after = db.get_page("p").await.unwrap().unwrap();
    assert_eq!(
        after.content,
        "Tokio runs async tasks. Workers steal queued tasks. \
         I prefer the current-thread runtime.[1] Workers are cheap to spawn.[1]",
        "the person's sentence keeps its marker"
    );
    let by_occurrence: Vec<(u32, &str)> = after
        .citations
        .iter()
        .map(|c| (c.occurrence, c.locator.as_str()))
        .collect();
    assert_eq!(by_occurrence, [(1, "mem-x"), (2, "mem-ai")]);

    let view = page_sentences(&db, "p").await.unwrap();
    assert_eq!(view.sentences[2].author, SentenceAuthor::Person);
    assert_eq!(view.sentences[2].citations[0].locator, "mem-x");
    assert_eq!(view.sentences[3].citations[0].locator, "mem-ai");
    assert_eq!(view.proposals.len(), 1);
    assert_eq!(
        view.proposals[0].after,
        "The multi-thread runtime is better for everyone."
    );
}

#[tokio::test]
async fn proposals_are_accepted_or_rejected_one_by_one() {
    let (db, _dir) = test_db().await;
    let before = page_with_my_unsupported_line(&db, "p").await;
    let units = crate::authorship::sentence_units(&before.content);
    let mine = units[2].hash.clone();
    for (reason, refused) in [
        (None, "reason_required"),
        (Some(WeakReason::Stale), "reason_not_current"),
        (Some(WeakReason::NoCitation), "reason_not_current"),
    ] {
        let result = propose_sentence(
            &db,
            "p",
            &SentenceProposeRequest {
                sentence_hash: mine.clone(),
                text: "I prefer the multi-thread runtime now.".into(),
                why: None,
                reason,
                expected_version: None,
            },
            "claude-code",
        )
        .await;
        assert!(
            matches!(&result, Err(WenlanError::Validation(m)) if m.starts_with(refused)),
            "{reason:?}: {result:?}"
        );
    }
    let first = propose_sentence(
        &db,
        "p",
        &SentenceProposeRequest {
            sentence_hash: mine.clone(),
            text: "I prefer the multi-thread runtime now.".into(),
            why: Some("you said so yesterday".into()),
            reason: Some(WeakReason::Unsupported),
            expected_version: Some(before.version),
        },
        "claude-code",
    )
    .await
    .unwrap();
    let second = propose_sentence(
        &db,
        "p",
        &SentenceProposeRequest {
            sentence_hash: mine.clone(),
            text: String::new(),
            why: None,
            reason: Some(WeakReason::Unsupported),
            expected_version: None,
        },
        "claude-code",
    )
    .await
    .unwrap();
    let stale = propose_sentence(
        &db,
        "p",
        &SentenceProposeRequest {
            sentence_hash: mine.clone(),
            text: "x".into(),
            why: None,
            reason: Some(WeakReason::Unsupported),
            expected_version: Some(before.version - 1),
        },
        "claude-code",
    )
    .await;
    assert!(matches!(stale, Err(WenlanError::Conflict(_))));

    let rejected = reject_sentence_proposal(&db, "p", &second.id, None)
        .await
        .unwrap();
    assert_eq!(rejected.status, SentenceProposalStatus::Rejected);
    assert!(matches!(
        reject_sentence_proposal(&db, "p", &second.id, None).await,
        Err(WenlanError::Conflict(_))
    ));

    let accepted = accept_sentence_proposal(&db, "p", &first.id, None, None)
        .await
        .unwrap();
    assert_eq!(accepted.proposal.status, SentenceProposalStatus::Accepted);
    let page = db.get_page("p").await.unwrap().unwrap();
    assert_eq!(accepted.version, page.version);
    assert!(page
        .content
        .ends_with("I prefer the multi-thread runtime now."));
    let view = page_sentences(&db, "p").await.unwrap();
    let taken = &view.sentences[2];
    assert_eq!(
        taken.author,
        SentenceAuthor::Person,
        "「你點頭的」 counts as yours"
    );
    assert!(taken.accepted);
    assert_eq!(
        taken.origin,
        SentenceAuthor::Ai,
        "and keeps its citation requirement"
    );
    assert!(view.proposals.is_empty());

    let entries = db.list_sentence_history("page", "p").await.unwrap();
    let ops: Vec<&str> = entries.iter().map(|e| e.op.as_str()).collect();
    assert_eq!(
        ops,
        [
            "sentence.propose",
            "sentence.propose",
            "sentence.reject",
            "sentence.accept"
        ]
    );
}

#[tokio::test]
async fn a_flag_marks_the_sentence_until_it_changes() {
    let (db, _dir) = test_db().await;
    let page = ai_page(&db, "p", None).await;
    let hash = crate::authorship::sentence_units(&page.content)[1]
        .hash
        .clone();
    let flagged = flag_sentence(&db, "p", &hash, Some("不對")).await.unwrap();
    assert_eq!(flagged.version, page.version);
    let view = page_sentences(&db, "p").await.unwrap();
    assert!(view.sentences[1].flagged && !view.sentences[0].flagged);
    assert!(matches!(
        flag_sentence(&db, "p", "nope", None).await,
        Err(WenlanError::Conflict(_))
    ));
}

#[tokio::test]
async fn an_accepted_sentence_waits_for_the_person_and_the_lock_makes_everything_wait() {
    // One rule on every page, no "edit directly" mode: the AI updates the
    // sentences it wrote that nobody touched, and a sentence the person
    // accepted from a proposal is theirs from then on.
    let (db, _dir) = test_db().await;
    let page = ai_page(&db, "p", None).await;
    assert_eq!(ai_edit_mode(&page), AiEditMode::Propose);
    let first = crate::authorship::sentence_units(&page.content)[0]
        .hash
        .clone();
    let proposal = propose_sentence(
        &db,
        "p",
        &SentenceProposeRequest {
            sentence_hash: first,
            text: "Tokio runs async tasks on a work-stealing pool.".into(),
            why: None,
            reason: None,
            expected_version: Some(page.version),
        },
        "claude-code",
    )
    .await
    .unwrap();
    accept_sentence_proposal(&db, "p", &proposal.id, None, None)
        .await
        .unwrap();
    let accepted = db.get_page("p").await.unwrap().unwrap();

    let rewrite = "Tokio runs async tasks. Workers steal tasks from each other.";
    let result = write(&db, "p", rewrite, "agent_refresh", Some(accepted.version)).await;
    assert!(result.wrote, "the AI's untouched sentence updates");
    assert!(
        result.sentence_proposals.is_empty(),
        "the accepted one is protected, and a rewording of it is not proposed"
    );
    let after = db.get_page("p").await.unwrap().unwrap();
    assert_eq!(
        after.content,
        "Tokio runs async tasks on a work-stealing pool. Workers steal tasks from each other."
    );

    // The lock makes every AI change wait; unlocking returns to the rule.
    assert!(db.set_page_review_required("p", true).await.unwrap());
    let locked = db.get_page("p").await.unwrap().unwrap();
    assert_eq!(ai_edit_mode(&locked), AiEditMode::Locked);
    // A change to a sentence the page has: no whole-page card, nothing
    // lands, and with no facts-changed reason nothing is proposed either.
    let reworded = "Tokio runs async tasks on a work-stealing pool. Workers trade tasks.";
    let result = write(&db, "p", reworded, "agent_refresh", Some(locked.version)).await;
    assert_eq!(result.outcome, WriteOutcome::Unchanged);
    assert!(result.revision_card_id.is_none() && result.sentence_proposals.is_empty());
    assert_eq!(
        db.get_page("p").await.unwrap().unwrap().content,
        after.content
    );
    // A write that adds a sentence still waits as a whole-page card.
    let grown = format!("{} Tokio has timers.", after.content);
    let result = write(&db, "p", &grown, "agent_refresh", Some(locked.version)).await;
    assert!(result.revision_card_id.is_some(), "{result:?}");
    assert_eq!(
        db.get_page("p").await.unwrap().unwrap().content,
        after.content
    );
    assert_eq!(
        page_sentences(&db, "p").await.unwrap().ai_edits,
        AiEditMode::Locked
    );
    assert!(db.set_page_review_required("p", false).await.unwrap());
    assert_eq!(
        page_sentences(&db, "p").await.unwrap().ai_edits,
        AiEditMode::Propose
    );
}

#[tokio::test]
async fn unknown_origin_is_protected_only_on_a_human_owned_page() {
    let (db, _dir) = test_db().await;
    ai_page(&db, "p", None).await;
    db.execute_batch_for_test(
        "UPDATE page_history SET edited_by = 'migration_87' WHERE page_id = 'p'",
    )
    .await;
    let rewrite = "Tokio schedules everything differently. Workers steal queued tasks.";
    let page = db.get_page("p").await.unwrap().unwrap();
    let result = write(&db, "p", rewrite, "agent_refresh", Some(page.version)).await;
    assert!(result.wrote && result.sentence_proposals.is_empty());
    assert_eq!(db.get_page("p").await.unwrap().unwrap().content, rewrite);

    ai_page(&db, "q", None).await;
    db.execute_batch_for_test(
        "UPDATE page_history SET edited_by = 'migration_87' WHERE page_id = 'q';
         UPDATE pages SET user_edited = 1 WHERE id = 'q';",
    )
    .await;
    let page = db.get_page("q").await.unwrap().unwrap();
    let view = page_sentences(&db, "q").await.unwrap();
    assert!(view
        .sentences
        .iter()
        .all(|s| s.author == SentenceAuthor::Unknown && s.protected));
    let result = write(&db, "q", rewrite, "agent_refresh", Some(page.version)).await;
    assert_eq!(result.outcome, WriteOutcome::Unchanged);
    assert!(
        result.sentence_proposals.is_empty(),
        "protected, and a rewording is not proposed"
    );
    assert_eq!(db.get_page("q").await.unwrap().unwrap().content, AI_BODY);
}

#[tokio::test]
async fn a_forced_rebuild_regenerates_the_ais_sentences_and_keeps_the_persons() {
    let (db, _dir) = test_db().await;
    page_with_my_line(&db, "p").await;
    let fence = db.try_get_page_fence("p").await.unwrap().unwrap();
    let result = page_write(
        &db,
        PageWrite::UserForcedUpdate {
            page_id: "p",
            req: UpdatePageRequest {
                content: "Tokio is an async runtime. Workers steal queued tasks.".into(),
                source_memory_ids: vec!["mem-tokio".to_string()],
                expected_version: None,
                caller_id: None,
                operation_id: None,
            },
            edited_by: "distill",
            expected_fence: &fence,
            knowledge_path: None,
            citations: None,
        },
    )
    .await
    .unwrap();
    assert!(result.wrote);
    assert!(
        result.sentence_proposals.is_empty(),
        "my line is kept, and with no facts-changed reason dropping it is not proposed"
    );
    let after = db.get_page("p").await.unwrap().unwrap();
    assert_eq!(
        after.content,
        "Tokio is an async runtime. Workers steal queued tasks. \
         I prefer the current-thread runtime."
    );
    assert!(
        after.user_edited,
        "the rebuild kept the person's text, so it stays theirs"
    );
}

fn citation(locator: &str, status: &str) -> String {
    serde_json::json!([{
        "occurrence": 1, "marker": 1, "source_kind": "memory", "locator": locator,
        "score": 0.9, "status": status
    }])
    .to_string()
}

async fn set_memory(db: &MemoryDB, source_id: &str, content: &str, version: i64) {
    db.execute_batch_for_test(&format!(
        "UPDATE memories SET content = '{content}', version = {version} \
         WHERE source_id = '{source_id}'"
    ))
    .await;
}

#[tokio::test]
async fn citations_record_the_cited_version_and_go_stale_only_when_the_text_changes() {
    let (db, _dir) = test_db().await;
    seed_memory(&db, "mem-port", "The daemon binds port 7878 by default").await;
    db.insert_page_with_kind(
        "c",
        "Ports",
        None,
        "The daemon binds port 7878 by default.[1] Another uncited claim here.",
        None,
        None,
        &["mem-port"],
        &chrono::Utc::now().to_rfc3339(),
        "distilled",
        "confirmed",
        None,
        Some(&citation("mem-port", "verified")),
    )
    .await
    .unwrap();
    let page = db.get_page("c").await.unwrap().unwrap();
    assert_eq!(
        page.citations[0].cited_version,
        Some(1),
        "stored as mem-port@1"
    );
    assert!(page.citations[0].cited_digest.is_some());

    let view = page_sentences(&db, "c").await.unwrap();
    assert_eq!(view.sentences[0].evidence, EvidenceState::Verified);
    assert_eq!(view.sentences[0].weak, []);
    assert_eq!(view.sentences[1].weak, [WeakReason::NoCitation]);

    // A newer version with the same text still supports the citation.
    set_memory(&db, "mem-port", "The daemon binds port 7878 by default", 2).await;
    let view = page_sentences(&db, "c").await.unwrap();
    let cited = &view.sentences[0].citations[0];
    assert_eq!(
        (cited.cited_version, cited.current_version),
        (Some(1), Some(2))
    );
    assert!(!cited.stale, "unchanged text across versions is not stale");

    // A changed text makes it stale: not disproven, and no longer verified.
    set_memory(&db, "mem-port", "Wenlan listens on a unix socket only", 3).await;
    let view = page_sentences(&db, "c").await.unwrap();
    assert!(view.sentences[0].citations[0].stale);
    assert_eq!(view.sentences[0].weak, [WeakReason::Stale]);
    assert_eq!(view.sentences[0].evidence, EvidenceState::Unevaluated);

    // Rechecking that one sentence against the new text disproves it.
    let hash = view.sentences[0].hash.clone();
    let other_before = page.citations.clone();
    let rechecked = recheck_sentence(&db, "c", &hash).await.unwrap();
    assert_eq!(rechecked.sentence.weak, [WeakReason::Disproven]);
    assert_eq!(rechecked.sentence.evidence, EvidenceState::Unverified);
    let stored = db.get_page("c").await.unwrap().unwrap();
    assert_eq!(
        stored.citations[0].cited_version,
        Some(3),
        "rechecked against @3"
    );
    assert!(stored.citations[0].disproven);
    assert_eq!(
        stored.version, page.version,
        "a recheck is not a page version"
    );
    assert_eq!(other_before.len(), stored.citations.len());

    // Once the source says it again, a recheck restores support.
    set_memory(&db, "mem-port", "The daemon binds port 7878 by default", 4).await;
    let restored = recheck_sentence(&db, "c", &hash).await.unwrap();
    assert_eq!(restored.sentence.evidence, EvidenceState::Verified);
    assert_eq!(restored.sentence.weak, []);
}

#[tokio::test]
async fn an_unverified_citation_is_weak_and_a_marker_without_a_check_is_unevaluated() {
    let (db, _dir) = test_db().await;
    seed_memory(
        &db,
        "mem-x",
        "Completely unrelated source text about gardening",
    )
    .await;
    db.insert_page_with_kind(
        "u",
        "U",
        None,
        "The daemon binds port 7878 by default.[1] Second claim cites nothing stored.[2]",
        None,
        None,
        &["mem-x"],
        &chrono::Utc::now().to_rfc3339(),
        "distilled",
        "confirmed",
        None,
        Some(&citation("mem-x", "unverified")),
    )
    .await
    .unwrap();
    let view = page_sentences(&db, "u").await.unwrap();
    assert_eq!(view.sentences[0].weak, [WeakReason::Unsupported]);
    assert_eq!(view.sentences[0].evidence, EvidenceState::Unverified);
    assert_eq!(view.sentences[1].citations[0].status, "unevaluated");
    assert_eq!(view.sentences[1].evidence, EvidenceState::Unevaluated);
}

#[tokio::test]
async fn migration_145_is_idempotent_on_an_upgraded_database() {
    let (db, dir) = crate::db::tests::test_db_at(144).await;
    drop(db);
    for _ in 0..2 {
        let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
            .await
            .unwrap();
        let found = db
            .scalar_for_test(
                "SELECT
                    (SELECT COUNT(*) FROM sqlite_master WHERE name = 'sentence_history')
                  + (SELECT user_version FROM pragma_user_version)",
            )
            .await;
        assert_eq!(found, 1 + i64::from(crate::db::SCHEMA_VERSION));
    }
}

#[tokio::test]
async fn an_authored_note_no_person_touched_is_the_ais() {
    // A note an agent writes with no sources lands as `authored`; until a
    // person edits it, its sentences are the agent's, so a version-checked
    // agent write lands on them.
    let (db, _dir) = test_db().await;
    seed_memory(&db, "mem-tokio", "Tokio runs async tasks").await;
    db.insert_page_with_kind(
        "agent_note",
        "Agent note",
        None,
        AI_BODY,
        None,
        None,
        &[],
        &chrono::Utc::now().to_rfc3339(),
        "authored",
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
    let page = db.get_page("agent_note").await.unwrap().unwrap();
    assert!(!page.user_edited);
    let view = page_sentences(&db, "agent_note").await.unwrap();
    assert!(view
        .sentences
        .iter()
        .all(|s| s.author == SentenceAuthor::Ai && !s.protected));

    // This older `create` row does not say who made the page, and an edit
    // says only that the person edited it: their own line is theirs, the rest
    // stays unattributed (and protected, the page now being theirs).
    let mine = format!("{AI_BODY} I keep this line.");
    assert!(
        write(&db, "agent_note", &mine, "manual_edit", None)
            .await
            .wrote
    );
    let view = page_sentences(&db, "agent_note").await.unwrap();
    assert_eq!(
        authors(&view),
        [
            SentenceAuthor::Unknown,
            SentenceAuthor::Unknown,
            SentenceAuthor::Person
        ]
    );
    assert!(view.sentences.iter().all(|s| s.protected));
}

/// An authored page created by `actor` (`unknown` is the person in the app).
async fn authored_page(db: &MemoryDB, id: &str, actor: &str) -> Page {
    db.insert_page_with_basis(
        id,
        "Authored",
        None,
        AI_BODY,
        None,
        None,
        &[],
        &chrono::Utc::now().to_rfc3339(),
        "authored",
        "confirmed",
        None,
        None,
        &[],
        actor,
        None,
    )
    .await
    .unwrap();
    db.get_page(id).await.unwrap().unwrap()
}

#[tokio::test]
async fn an_ai_tools_page_stays_the_ais_after_the_person_edits_one_line() {
    // Seeing a page does not make it yours: the person's first edit makes
    // only what they changed theirs.
    let (db, _dir) = test_db().await;
    seed_memory(&db, "mem-tokio", "Tokio runs async tasks").await;
    authored_page(&db, "codex_note", "codex").await;
    let rows = db.list_page_history("codex_note", 10).await.unwrap();
    assert_eq!(rows[0].edited_by, "agent_create");
    let edited = "Tokio runs many async tasks. Workers steal queued tasks.";
    assert!(
        write(&db, "codex_note", edited, "manual_edit", None)
            .await
            .wrote
    );
    let view = page_sentences(&db, "codex_note").await.unwrap();
    assert_eq!(
        authors(&view),
        [SentenceAuthor::Person, SentenceAuthor::Ai],
        "the untouched sentence is still the AI tool's"
    );
    assert!(
        !view.sentences[1].protected,
        "so the AI can still update it"
    );
}

#[tokio::test]
async fn a_page_the_person_creates_in_the_app_is_theirs_from_birth() {
    let (db, _dir) = test_db().await;
    authored_page(&db, "my_note", "unknown").await;
    let rows = db.list_page_history("my_note", 10).await.unwrap();
    assert_eq!(rows[0].edited_by, "person_create");
    assert_eq!(
        crate::page_history::kind_of_edit(&rows[0].edited_by),
        wenlan_types::responses::PageHistoryKind::PersonEdit
    );
    let view = page_sentences(&db, "my_note").await.unwrap();
    assert!(view
        .sentences
        .iter()
        .all(|s| s.author == SentenceAuthor::Person && s.protected));
}

#[tokio::test]
async fn undoing_an_ai_write_puts_back_only_its_untouched_sentences_as_the_ais() {
    let (db, _dir) = test_db().await;
    let mine = page_with_my_line(&db, "p").await;
    assert!(
        mine.human_edited_at.is_some(),
        "the person's save counts as their edit"
    );
    // The AI rewrites its first sentence, drops its second, and adds one.
    let rewrite = "Tokio runs async tasks on a pool. I prefer the current-thread runtime. \
                   Tokio has timers.";
    let result = write(&db, "p", rewrite, "agent_refresh", Some(mine.version)).await;
    assert!(result.wrote);
    let ai = db.get_page("p").await.unwrap().unwrap();
    assert_eq!(ai.content, rewrite);
    assert_eq!(
        ai.human_edited_at, mine.human_edited_at,
        "an AI write is not the person's edit"
    );
    let view = page_sentences(&db, "p").await.unwrap();
    let last = view.last_ai_update.expect("the AI wrote last");
    assert_eq!(last.version, ai.version);
    assert_eq!(
        last.changed_sentences, 3,
        "changed one, removed one, added one"
    );
    assert_eq!(last.changes.len(), 3, "one entry per counted sentence");
    let changed: Vec<_> = last
        .changes
        .iter()
        .filter(|c| c.kind == wenlan_types::authorship::SentenceChangeKind::Changed)
        .collect();
    assert!(!changed.is_empty(), "{:?}", last.changes);
    for change in &changed {
        assert!(
            view.sentences
                .iter()
                .any(|s| Some(&s.hash) == change.hash.as_ref()),
            "a changed sentence is found in the sentences list: {change:?}"
        );
        assert!(change.before.is_some());
    }
    let rows = db.list_page_history("p", 10).await.unwrap();
    assert_eq!(
        crate::page_history::changed_sentences_at(&rows, ai.version),
        Some(3)
    );

    // The person touches the AI's new sentence; the undo leaves it alone.
    let touched = "Tokio runs async tasks on a pool. I prefer the current-thread runtime. \
                   Tokio has timers and intervals.";
    assert!(write(&db, "p", touched, "manual_edit", None).await.wrote);
    let person_version = db.get_page("p").await.unwrap().unwrap().version;

    let undone = revert_ai_write(&db, "p", ai.version, None).await.unwrap();
    assert_eq!((undone.reverted, undone.skipped), (2, 1));
    let after = db.get_page("p").await.unwrap().unwrap();
    assert_eq!(undone.version, after.version);
    assert_eq!(
        after.content,
        "Tokio runs async tasks. Workers steal queued tasks. I prefer the current-thread runtime. \
         Tokio has timers and intervals."
    );
    let view = page_sentences(&db, "p").await.unwrap();
    assert_eq!(
        authors(&view),
        [
            SentenceAuthor::Ai,
            SentenceAuthor::Ai,
            SentenceAuthor::Person,
            SentenceAuthor::Person
        ],
        "what the undo put back stays the AI's; undoing never makes text yours"
    );
    let ops: Vec<String> = db
        .list_sentence_history("page", "p")
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.op)
        .collect();
    assert!(ops.iter().any(|op| op == "sentence.revert"));

    assert!(matches!(
        revert_ai_write(&db, "p", person_version, None).await,
        Err(WenlanError::Conflict(m)) if m.starts_with("not_an_ai_write")
    ));
    assert!(matches!(
        revert_ai_write(&db, "p", 9_999, None).await,
        Err(WenlanError::NotFound(_))
    ));
    assert!(matches!(
        revert_ai_write(&db, "nope", 1, None).await,
        Err(WenlanError::NotFound(_))
    ));
}

#[tokio::test]
async fn a_page_only_the_ai_wrote_has_no_person_edit_time() {
    let (db, _dir) = test_db().await;
    let page = ai_page(&db, "p", None).await;
    assert_eq!(page.human_edited_at, None);
    let view = page_sentences(&db, "p").await.unwrap();
    let last = view.last_ai_update.expect("the AI wrote the page");
    assert_eq!(last.version, page.version);
    assert_eq!(last.changed_sentences, 2, "both sentences are new");
}

#[tokio::test]
async fn migration_148_keeps_every_sentence_history_row_and_admits_revert() {
    let (db, dir) = crate::db::tests::test_db_at(147).await;
    let page = ai_page(&db, "p", None).await;
    let hash = crate::authorship::sentence_units(&page.content)[0]
        .hash
        .clone();
    flag_sentence(&db, "p", &hash, Some("不對")).await.unwrap();
    drop(db);
    for _ in 0..2 {
        let db = MemoryDB::new(dir.path(), Arc::new(NoopEmitter))
            .await
            .unwrap();
        let entries = db.list_sentence_history("page", "p").await.unwrap();
        assert_eq!(entries.len(), 1, "the flag survives the rebuild");
        assert_eq!(entries[0].op, "sentence.flag");
        db.append_sentence_history(NewHistoryEntry {
            ts: 1,
            tx: "tx_m148",
            actor: &person(),
            op: OP_SENTENCE_REVERT,
            target: &page_target("q"),
            version: Some(1),
            why: None,
            text: None,
            digest_before: None,
            digest_after: None,
            data: &serde_json::json!({}),
        })
        .await
        .unwrap();
    }
}
