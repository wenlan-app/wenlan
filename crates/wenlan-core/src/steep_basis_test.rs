// SPDX-License-Identifier: Apache-2.0
//! Steep on demand for a note that relies on another page: when that page
//! changes, the caller's AI tool brings the note in line with it. Only a
//! locked note waits for the person.

use super::*;
use crate::db::code_basis_test::test_db;
use crate::db::BASIS_STATE_OK;

const T0: &str = "2026-01-01T00:00:00Z";
const LEAD_V1: &str = "Wenlan keeps docs, skills and rules your AI tools work from.";
const LEAD_V2: &str = "Wenlan is the living playbook for every AI tool.";
const CITER_V1: &str = "Our pitch: Wenlan keeps docs, skills and rules your AI tools work from.";
const CITER_V2: &str = "Our pitch: Wenlan is the living playbook for every AI tool.";

async fn note(db: &MemoryDB, id: &str, title: &str, content: &str, kind: &str) {
    db.insert_page_with_kind(
        id,
        title,
        None,
        content,
        None,
        Some("sp"),
        &[],
        T0,
        kind,
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
}

/// `lead` is the page others follow; `intro` relies on it at its first
/// version, then `lead` changes.
async fn lead_changed(db: &MemoryDB) {
    note(db, "lead", "Positioning", LEAD_V1, "authored").await;
    note(db, "intro", "Introduction", CITER_V1, "authored").await;
    db.mark_page_basis("intro", "lead", 1, "human")
        .await
        .unwrap();
    db.update_page_content("lead", LEAD_V2, &[], "manual_edit")
        .await
        .unwrap();
}

async fn basis_state(db: &MemoryDB) -> String {
    db.list_page_basis("intro").await.unwrap()[0].state.clone()
}

#[tokio::test]
async fn a_note_whose_base_changed_is_listed_for_the_ai_tool_not_the_person() {
    let (db, _dir) = test_db().await;
    lead_changed(&db).await;

    let list = list_work(&db, &ReadScope::Global, |_| true, 10, None, false)
        .await
        .unwrap();
    let item = list
        .items
        .iter()
        .find(|item| item.page_id == "intro")
        .expect("the note waits for steep");
    assert_eq!(item.stale_reason, STEEP_REASON_BASIS_CHANGED);
    assert!(!item.needs_you, "an AI tool can settle it");
    assert!(!item.human_owned, "only a locked note waits for the person");
    assert!(waiting_page_ids(&db)
        .await
        .unwrap()
        .contains(&"intro".to_string()));
}

#[tokio::test]
async fn the_claim_shows_the_base_before_and_now_and_the_rewrite_confirms_it() {
    let (db, _dir) = test_db().await;
    lead_changed(&db).await;

    let claim = claim(&db, "intro", &ReadScope::Global, None).await.unwrap();
    assert_eq!(claim.plan, STEEP_PLAN_REWRITE);
    assert_eq!(claim.stale_reason, STEEP_REASON_BASIS_CHANGED);
    assert_eq!(claim.bases.len(), 1);
    assert_eq!(claim.bases[0].page_id, "lead");
    assert_eq!(claim.bases[0].content, LEAD_V2);
    assert_eq!(claim.bases[0].before.as_deref(), Some(LEAD_V1));

    // A sentence taken from the base page carries no [n].
    let done = commit(
        &db,
        claim.token.as_deref().unwrap(),
        STEEP_ACTION_REWRITE,
        Some(CITER_V2),
        &[],
        None,
        Some("claude-code"),
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "written");
    let after = db.get_page("intro").await.unwrap().unwrap();
    assert_eq!(after.content, CITER_V2);
    assert_eq!(
        basis_state(&db).await,
        BASIS_STATE_OK,
        "confirmed with the write"
    );

    let list = list_work(&db, &ReadScope::Global, |_| true, 10, None, false)
        .await
        .unwrap();
    assert!(list.items.iter().all(|item| item.page_id != "intro"));
}

#[tokio::test]
async fn still_accurate_confirms_the_base_without_rewriting() {
    let (db, _dir) = test_db().await;
    lead_changed(&db).await;
    let before = db.get_page("intro").await.unwrap().unwrap();

    let claim = claim(&db, "intro", &ReadScope::Global, None).await.unwrap();
    let done = commit(
        &db,
        claim.token.as_deref().unwrap(),
        STEEP_ACTION_STILL_ACCURATE,
        None,
        &[],
        None,
        Some("claude-code"),
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "still_accurate");
    let after = db.get_page("intro").await.unwrap().unwrap();
    assert_eq!(after.version, before.version);
    assert_eq!(basis_state(&db).await, BASIS_STATE_OK);
}

#[tokio::test]
async fn a_base_that_changes_again_after_the_claim_refuses_the_commit() {
    let (db, _dir) = test_db().await;
    lead_changed(&db).await;

    let claim = claim(&db, "intro", &ReadScope::Global, None).await.unwrap();
    db.update_page_content("lead", "Something else again.", &[], "manual_edit")
        .await
        .unwrap();
    let refused = commit(
        &db,
        claim.token.as_deref().unwrap(),
        STEEP_ACTION_REWRITE,
        Some(CITER_V2),
        &[],
        None,
        Some("claude-code"),
    )
    .await;
    assert!(
        matches!(refused, Err(SteepError::Changed(_))),
        "{refused:?}"
    );
    let after = db.get_page("intro").await.unwrap().unwrap();
    assert_eq!(after.content, CITER_V1, "nothing written");
}

#[tokio::test]
async fn a_locked_note_gets_a_revision_card_and_the_page_waits() {
    // Staging a revision card embeds it, so this one needs the embedder.
    let (db, _dir) = crate::db::tests::test_db().await;
    lead_changed(&db).await;
    db.set_page_review_required("intro", true).await.unwrap();

    let claim = claim(&db, "intro", &ReadScope::Global, None).await.unwrap();
    assert_eq!(claim.plan, STEEP_PLAN_REVIEW_CARD);
    let done = commit(
        &db,
        claim.token.as_deref().unwrap(),
        STEEP_ACTION_REWRITE,
        Some(CITER_V2),
        &[],
        None,
        Some("claude-code"),
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "review_card");
    // The card says which page changed, not only that the note is locked.
    let card = db
        .list_pending_revisions(10)
        .await
        .unwrap()
        .into_iter()
        .find(|card| card.target_source_id == "intro")
        .expect("the card is listed");
    assert_eq!(card.review_reason.as_deref(), Some("follows"));
    assert_eq!(card.follows_title.as_deref(), Some("Positioning"));
    let after = db.get_page("intro").await.unwrap().unwrap();
    assert_eq!(
        after.content, CITER_V1,
        "the locked page waits for the person"
    );

    // The card is the work that was due: the note is not offered again.
    let list = list_work(&db, &ReadScope::Global, |_| true, 10, None, false)
        .await
        .unwrap();
    assert!(list.items.iter().all(|item| item.page_id != "intro"));
}

#[tokio::test]
async fn sentences_the_person_wrote_follow_the_page_too() {
    // A note the person wrote themselves: every sentence is theirs, which the
    // sentence gate otherwise keeps from any AI rewrite. Following a page is
    // the person's own ask, so the rewrite lands.
    let (db, dir) = test_db().await;
    let pages = dir.path().join("pages");
    std::fs::create_dir_all(&pages).unwrap();
    note(&db, "lead", "Positioning", LEAD_V1, "authored").await;
    let intro = crate::post_write::create_page(
        &db,
        wenlan_types::requests::CreateConceptRequest {
            title: "Introduction".into(),
            content: CITER_V1.into(),
            summary: None,
            entity_id: None,
            space: wenlan_types::WriteSpaceTarget::Named("sp".into()),
            source_memory_ids: Vec::new(),
            creation_kind: Some("authored".into()),
            workspace: Some("sp".into()),
        },
        "unknown",
        Some(&pages),
    )
    .await
    .unwrap()
    .id;
    db.mark_page_basis(&intro, "lead", 1, "human")
        .await
        .unwrap();
    db.update_page_content("lead", LEAD_V2, &[], "manual_edit")
        .await
        .unwrap();

    let claim = claim(&db, &intro, &ReadScope::Global, None).await.unwrap();
    assert_eq!(claim.plan, STEEP_PLAN_REWRITE);
    let done = commit(
        &db,
        claim.token.as_deref().unwrap(),
        STEEP_ACTION_REWRITE,
        Some(CITER_V2),
        &[],
        Some(&pages),
        Some("claude-code"),
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "written");
    let after = db.get_page(&intro).await.unwrap().unwrap();
    assert_eq!(after.content, CITER_V2);
    assert_eq!(
        db.list_page_basis(&intro).await.unwrap()[0].state,
        BASIS_STATE_OK
    );
}
