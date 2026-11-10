// SPDX-License-Identifier: Apache-2.0
//! UX-WHO-2: an AI tool's write the person explicitly asked for in the
//! conversation (`person_asked`) lands over their sentences, those sentences
//! stay theirs, and history keeps their words. Every other guard still holds.

use super::*;
use crate::events::NoopEmitter;
use crate::post_write::{page_write, PageWrite, WriteResult, PERSON_ASKED};
use std::sync::Arc;
use wenlan_types::authorship::SentenceAuthor;
use wenlan_types::requests::UpdatePageRequest;
use wenlan_types::responses::{PageChangelogEntry, PageHistoryKind};

const AI_BODY: &str = "Tokio runs async tasks. Workers steal queued tasks.";
const MINE: &str = "I prefer the current-thread runtime.";
const QUOTE: &str = "請把我那句改成 multi-thread runtime，我改主意了";

async fn test_db() -> (MemoryDB, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = MemoryDB::new(&dir.path().join("test.db"), Arc::new(NoopEmitter))
        .await
        .unwrap();
    (db, dir)
}

/// An AI page the person then added one sentence of their own to, whose
/// citation checked unsupported, so a plain agent rewrite of it is staged as
/// a sentence proposal rather than dropped as a rewording.
async fn page_with_my_line(db: &MemoryDB, id: &str) -> Page {
    db.upsert_documents(vec![crate::sources::RawDocument {
        source: "memory".to_string(),
        source_id: "mem-tokio".to_string(),
        title: "mem-tokio".to_string(),
        content: "Tokio runs async tasks and workers steal queued tasks".to_string(),
        last_modified: chrono::Utc::now().timestamp(),
        memory_type: Some("fact".to_string()),
        source_agent: Some("test".to_string()),
        confidence: Some(0.9),
        ..Default::default()
    }])
    .await
    .unwrap();
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
        None,
    )
    .await
    .unwrap();
    let mine = format!("{AI_BODY} {MINE}[1]");
    let saved = page_write(
        db,
        PageWrite::Update {
            page_id: id,
            req: UpdatePageRequest {
                content: mine,
                source_memory_ids: vec!["mem-tokio".to_string()],
                expected_version: None,
                caller_id: None,
                operation_id: None,
            },
            edited_by: "manual_edit",
            require_stale: false,
            expected_source_revision: None,
            knowledge_path: None,
            citations: None,
        },
    )
    .await
    .unwrap();
    assert!(saved.wrote);
    let page = db.get_page(id).await.unwrap().unwrap();
    let citations = serde_json::json!([{
        "occurrence": 1, "marker": 1, "source_kind": "memory", "locator": "mem-tokio",
        "score": 0.9, "status": "unverified"
    }])
    .to_string();
    assert!(db
        .set_page_citations_at_page_version(id, &citations, page.version, &[])
        .await
        .unwrap());
    db.get_page(id).await.unwrap().unwrap()
}

/// An agent's version-checked write, as the refresh route sends it.
async fn agent_write(
    db: &MemoryDB,
    id: &str,
    content: &str,
    expected_version: Option<i64>,
    person_asked: Option<&str>,
) -> Result<WriteResult, WenlanError> {
    let revision = db.get_page_source_revision(id).await.unwrap();
    page_write(
        db,
        PageWrite::AgentUpdate {
            page_id: id,
            req: UpdatePageRequest {
                content: content.to_string(),
                source_memory_ids: vec!["mem-tokio".to_string()],
                expected_version,
                caller_id: None,
                operation_id: None,
            },
            agent: Some("claude-code"),
            expected_source_revision: revision,
            knowledge_path: None,
            basis: crate::db::BasisWrite {
                actor: "claude-code".to_string(),
                person_asked: person_asked.map(str::to_string),
                ..crate::db::BasisWrite::default()
            },
            citations: None,
        },
    )
    .await
}

async fn changelog(db: &MemoryDB, id: &str) -> Vec<PageChangelogEntry> {
    serde_json::from_str(&db.get_page_changelog(id).await.unwrap()).unwrap_or_default()
}

#[tokio::test]
async fn ux_who_2_a_person_asked_write_lands_on_their_sentence_and_history_keeps_their_words() {
    let (db, _dir) = test_db().await;
    let before = page_with_my_line(&db, "p").await;
    let asked = format!("{AI_BODY} I prefer the multi-thread runtime now.");

    let result = agent_write(&db, "p", &asked, Some(before.version), Some(QUOTE))
        .await
        .unwrap();
    assert!(result.wrote, "{result:?}");
    assert!(result.sentence_proposals.is_empty() && result.edits_not_applied.is_empty());
    let after = db.get_page("p").await.unwrap().unwrap();
    assert_eq!(after.content, asked, "the write lands as sent");

    // The changed sentence is the person's, approved by asking.
    let view = page_sentences(&db, "p").await.unwrap();
    let changed = view
        .sentences
        .iter()
        .find(|s| s.text.contains("multi-thread"))
        .expect("the asked-for sentence is on the page");
    assert_eq!(changed.author, SentenceAuthor::Person);
    assert!(changed.accepted);
    assert_eq!(view.sentences[0].author, SentenceAuthor::Ai);

    // History: changed by the agent at the person's request, with their words.
    let entry = changelog(&db, "p").await.pop().unwrap();
    assert_eq!(entry.edited_by, PERSON_ASKED);
    assert_eq!(entry.agent.as_deref(), Some("claude-code"));
    assert_eq!(entry.person_asked.as_deref(), Some(QUOTE));
    let history = crate::page_history::build_page_history(
        &db,
        "p",
        &changelog(&db, "p").await,
        &Default::default(),
        false,
    )
    .await
    .unwrap();
    let item = history
        .iter()
        .find(|item| item.version == Some(after.version))
        .unwrap();
    assert_eq!(item.kind, PageHistoryKind::AiEdit);
    assert_eq!(item.person_asked.as_deref(), Some(QUOTE));

    // A person-asked write is not an AI edit to undo one by one; restoring
    // an earlier version is how it is undone.
    assert_eq!(
        authorship::version_author(PERSON_ASKED, ""),
        authorship::VersionAuthor::PersonAsked
    );
}

#[tokio::test]
async fn ux_who_2_the_same_write_without_person_asked_still_stages_a_proposal() {
    let (db, _dir) = test_db().await;
    let before = page_with_my_line(&db, "p").await;
    let asked = format!("{AI_BODY} I prefer the multi-thread runtime now.");

    let result = agent_write(&db, "p", &asked, Some(before.version), None)
        .await
        .unwrap();
    assert_eq!(result.sentence_proposals.len(), 1, "{result:?}");
    let after = db.get_page("p").await.unwrap().unwrap();
    assert!(
        after.content.contains(MINE),
        "the person's sentence stays: {}",
        after.content
    );
    assert_eq!(
        changelog(&db, "p").await.pop().unwrap().person_asked,
        None,
        "no request is recorded for an ordinary write"
    );
}

#[tokio::test]
async fn ux_who_2_person_asked_needs_their_words_and_the_version_read() {
    let (db, _dir) = test_db().await;
    let before = page_with_my_line(&db, "p").await;
    let asked = format!("{AI_BODY} I prefer the multi-thread runtime now.");

    for blank in ["", "   \n\t"] {
        let error = agent_write(&db, "p", &asked, Some(before.version), Some(blank))
            .await
            .unwrap_err();
        assert!(matches!(error, WenlanError::Validation(_)), "{error:?}");
    }
    let too_long = "字".repeat(crate::post_write::PERSON_ASKED_MAX_CHARS + 1);
    let error = agent_write(&db, "p", &asked, Some(before.version), Some(&too_long))
        .await
        .unwrap_err();
    assert!(matches!(error, WenlanError::Validation(_)), "{error:?}");
    let error = agent_write(&db, "p", &asked, None, Some(QUOTE))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, WenlanError::Validation(m) if m.contains("expected_version")),
        "{error:?}"
    );
    assert_eq!(
        db.get_page("p").await.unwrap().unwrap().content,
        before.content,
        "nothing was written"
    );
}

#[tokio::test]
async fn ux_who_2_a_person_asked_write_to_a_locked_page_still_waits_as_a_card() {
    let (db, _dir) = test_db().await;
    page_with_my_line(&db, "p").await;
    assert!(db.set_page_review_required("p", true).await.unwrap());
    let locked = db.get_page("p").await.unwrap().unwrap();
    let asked = format!("{AI_BODY} I prefer the multi-thread runtime now.");

    let result = agent_write(&db, "p", &asked, Some(locked.version), Some(QUOTE))
        .await
        .unwrap();
    assert!(
        result.gated && result.revision_card_id.is_some(),
        "{result:?}"
    );
    assert_eq!(
        db.get_page("p").await.unwrap().unwrap().content,
        locked.content,
        "the Lock holds even when the person asked"
    );
}

#[tokio::test]
async fn ux_who_2_the_damage_guard_still_refuses_a_person_asked_write() {
    let (db, _dir) = test_db().await;
    let before = page_with_my_line(&db, "p").await;
    let broken = format!("{AI_BODY} I prefer the multi\u{FFFD}thread runtime.");

    let error = agent_write(&db, "p", &broken, Some(before.version), Some(QUOTE))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, WenlanError::Validation(m) if m.contains("damage-guard")),
        "{error:?}"
    );
    assert_eq!(
        db.get_page("p").await.unwrap().unwrap().content,
        before.content
    );
}

#[tokio::test]
async fn ux_who_2_a_stale_person_asked_write_changes_nothing() {
    let (db, _dir) = test_db().await;
    let before = page_with_my_line(&db, "p").await;
    let asked = format!("{AI_BODY} I prefer the multi-thread runtime now.");

    let result = agent_write(&db, "p", &asked, Some(before.version - 1), Some(QUOTE)).await;
    assert!(
        !matches!(&result, Ok(r) if r.wrote),
        "a stale version never lands: {result:?}"
    );
    assert_eq!(
        db.get_page("p").await.unwrap().unwrap().content,
        before.content
    );
}

#[tokio::test]
async fn ux_who_2_a_restore_that_touches_the_persons_sentence_is_detected() {
    let (db, _dir) = test_db().await;
    let page = page_with_my_line(&db, "p").await;
    // Putting the AI-only first version back would drop the person's line.
    assert!(write_touches_person_sentences(&db, &page, AI_BODY)
        .await
        .unwrap());
    // Changing only the AI's sentences does not touch theirs.
    let ai_only = page.content.replace("Workers steal", "Workers take");
    assert!(!write_touches_person_sentences(&db, &page, &ai_only)
        .await
        .unwrap());
}
