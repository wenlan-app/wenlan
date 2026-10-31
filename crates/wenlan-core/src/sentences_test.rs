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
async fn a_version_checked_agent_write_lands_only_on_the_ais_sentences() {
    let (db, _dir) = test_db().await;
    let before = page_with_my_line(&db, "p").await;
    let rewrite = "Tokio runs async tasks on a pool. Workers steal queued tasks. \
                   The multi-thread runtime is better for everyone.";
    let result = write(&db, "p", rewrite, "agent_refresh", Some(before.version)).await;
    assert!(result.wrote, "the AI's own sentence change lands");
    assert_eq!(result.sentence_proposals.len(), 1);
    let after = db.get_page("p").await.unwrap().unwrap();
    assert_eq!(
        after.content,
        "Tokio runs async tasks on a pool. Workers steal queued tasks. \
         I prefer the current-thread runtime."
    );
    assert!(after.user_edited, "the page is still the person's");

    let view = page_sentences(&db, "p").await.unwrap();
    assert_eq!(view.proposals.len(), 1);
    let proposal = &view.proposals[0];
    assert_eq!(proposal.before, "I prefer the current-thread runtime.");
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
        .set_page_citations_at_page_version("p", &citation("mem-x", "verified"), before.version)
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
    let before = page_with_my_line(&db, "p").await;
    let units = crate::authorship::sentence_units(&before.content);
    let mine = units[2].hash.clone();
    let first = propose_sentence(
        &db,
        "p",
        &SentenceProposeRequest {
            sentence_hash: mine.clone(),
            text: "I prefer the multi-thread runtime now.".into(),
            why: Some("you said so yesterday".into()),
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
    assert_eq!(
        result.sentence_proposals.len(),
        1,
        "the accepted one becomes a proposal"
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
    assert_eq!(result.sentence_proposals.len(), 1);
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
    assert_eq!(
        result.sentence_proposals.len(),
        1,
        "deleting my line is a proposal"
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
    let view = page_sentences(&db, "p").await.unwrap();
    assert_eq!(view.proposals[0].after, "", "the AI proposed removing it");
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
        assert_eq!(found, 1 + 145);
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

    // Once a person has edited it, the page's first version counts as theirs.
    let mine = format!("{AI_BODY} I keep this line.");
    assert!(
        write(&db, "agent_note", &mine, "manual_edit", None)
            .await
            .wrote
    );
    let view = page_sentences(&db, "agent_note").await.unwrap();
    assert!(view
        .sentences
        .iter()
        .all(|s| s.author == SentenceAuthor::Person && s.protected));
}
