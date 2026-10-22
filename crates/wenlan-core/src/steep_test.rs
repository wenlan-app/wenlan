// SPDX-License-Identifier: Apache-2.0
//! Steep on demand: claim, commit, and the holds shared with the automatic run.

use super::*;
use crate::db::code_basis_test::test_db;

const SRC_A: &str = "Tally stores every invoice in one SQLite file next to the app.";
const SRC_B: &str = "Nightly backups copy the SQLite file to iCloud Drive.";
const BODY_A: &str = "Tally stores every invoice in one SQLite file next to the app [1].";
const BODY_AB: &str = "Tally stores every invoice in one SQLite file next to the app [1]. \
                       Nightly backups copy the SQLite file to iCloud Drive [2].";

async fn memory(db: &MemoryDB, id: &str, content: &str) {
    let now = chrono::Utc::now().timestamp();
    let conn = db.test_primary_session().await;
    conn.execute(
        "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
         VALUES (?1, ?1, ?1, ?2, 0, 'text', 'fact', 'test', 'claude-code', ?3, ?3, 1, 'confirmed', 'memory')",
        libsql::params![id, content, now],
    )
    .await
    .unwrap();
}

/// A machine-written note on `sources`, marked stale the way a source edit
/// marks it.
async fn stale_page(db: &MemoryDB, id: &str, sources: &[&str]) {
    let now = chrono::Utc::now().to_rfc3339();
    db.insert_page(
        id,
        "Tally storage",
        None,
        "old body",
        None,
        None,
        sources,
        &now,
    )
    .await
    .unwrap();
    db.set_page_stale(id, "source_updated").await.unwrap();
}

async fn page(db: &MemoryDB, id: &str) -> crate::pages::Page {
    db.get_page(id).await.unwrap().unwrap()
}

async fn two_source_page(db: &MemoryDB) {
    memory(db, "mem_a", SRC_A).await;
    memory(db, "mem_b", SRC_B).await;
    stale_page(db, "page_t", &["mem_a", "mem_b"]).await;
}

#[tokio::test]
async fn claim_then_rewrite_lands_body_citations_and_clears_staleness() {
    let (db, _dir) = test_db().await;
    two_source_page(&db).await;

    let claim = claim(&db, "page_t", &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(claim.plan, STEEP_PLAN_REWRITE);
    assert_eq!(claim.sources.len(), 2);
    assert_eq!(claim.sources[0].n, 1);
    assert_eq!(claim.sources[0].memory_id, "mem_a");
    let token = claim.token.unwrap();

    let done = commit(
        &db,
        &token,
        STEEP_ACTION_REWRITE,
        Some(BODY_AB),
        &[],
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "written");
    assert!(done.citations.unwrap().starts_with("2 verified"));

    let after = page(&db, "page_t").await;
    assert!(after.content.contains("iCloud Drive"));
    assert_eq!(after.stale_reason, None);
    assert_eq!(after.citations.len(), 2);
    assert_eq!(db.steep_leases().holder_of("page_t"), None);

    // The same token again replays the first result instead of writing twice.
    let again = commit(
        &db,
        &token,
        STEEP_ACTION_REWRITE,
        Some(BODY_AB),
        &[],
        None,
        None,
    )
    .await
    .unwrap();
    assert!(again.replayed);
    assert_eq!(again.outcome, "written");
    assert_eq!(page(&db, "page_t").await.version, after.version);
}

#[tokio::test]
async fn a_source_removed_after_the_claim_refuses_the_commit() {
    let (db, _dir) = test_db().await;
    two_source_page(&db).await;
    let token = claim(&db, "page_t", &ReadScope::Global, None)
        .await
        .unwrap()
        .token
        .unwrap();

    db.delete_by_source_id("memory", "mem_b").await.unwrap();

    let error = commit(
        &db,
        &token,
        STEEP_ACTION_REWRITE,
        Some(BODY_AB),
        &[],
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, SteepError::Changed(_)), "{error:?}");
    let after = page(&db, "page_t").await;
    assert_eq!(after.content, "old body");
    assert_eq!(after.stale_reason.as_deref(), Some("source_removed"));
}

#[tokio::test]
async fn a_source_edited_after_the_claim_refuses_the_commit() {
    let (db, _dir) = test_db().await;
    two_source_page(&db).await;
    let token = claim(&db, "page_t", &ReadScope::Global, None)
        .await
        .unwrap()
        .token
        .unwrap();

    // An in-place text change, without any staleness bookkeeping: only the
    // snapshot's own copy of the source text can catch it.
    {
        let conn = db.test_primary_session().await;
        conn.execute(
            "UPDATE memories SET content = 'Backups moved to a NAS.' WHERE id = 'mem_b'",
            (),
        )
        .await
        .unwrap();
    }

    let error = commit(
        &db,
        &token,
        STEEP_ACTION_REWRITE,
        Some(BODY_AB),
        &[],
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, SteepError::Changed(_)), "{error:?}");
    assert_eq!(page(&db, "page_t").await.content, "old body");
}

#[tokio::test]
async fn a_new_stale_mark_after_the_claim_is_not_erased() {
    let (db, _dir) = test_db().await;
    two_source_page(&db).await;
    let token = claim(&db, "page_t", &ReadScope::Global, None)
        .await
        .unwrap()
        .token
        .unwrap();

    db.set_page_stale("page_t", "source_updated").await.unwrap();

    let error = commit(
        &db,
        &token,
        STEEP_ACTION_STILL_ACCURATE,
        None,
        &[],
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, SteepError::Changed(_)), "{error:?}");
    assert_eq!(
        page(&db, "page_t").await.stale_reason.as_deref(),
        Some("source_updated")
    );
}

#[tokio::test]
async fn a_human_edit_after_the_claim_refuses_the_commit() {
    let (db, _dir) = test_db().await;
    two_source_page(&db).await;
    let token = claim(&db, "page_t", &ReadScope::Global, None)
        .await
        .unwrap()
        .token
        .unwrap();

    db.update_page_content("page_t", "my own words", &["mem_a", "mem_b"], "fs_edit")
        .await
        .unwrap();

    let error = commit(
        &db,
        &token,
        STEEP_ACTION_REWRITE,
        Some(BODY_AB),
        &[],
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, SteepError::Changed(_)), "{error:?}");
    assert_eq!(page(&db, "page_t").await.content, "my own words");
}

#[tokio::test]
async fn a_note_the_user_wrote_becomes_a_revision_card() {
    // Staging a revision card embeds it, so this one needs the embedder.
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(&db, "mem_a", SRC_A).await;
    let now = chrono::Utc::now().to_rfc3339();
    db.insert_page(
        "page_h",
        "Tally",
        None,
        "first",
        None,
        None,
        &["mem_a"],
        &now,
    )
    .await
    .unwrap();
    db.update_page_content("page_h", "my own words", &["mem_a"], "fs_edit")
        .await
        .unwrap();
    db.set_page_stale("page_h", "source_updated").await.unwrap();

    let claim = claim(&db, "page_h", &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(claim.plan, STEEP_PLAN_REVIEW_CARD);
    let done = commit(
        &db,
        claim.token.as_deref().unwrap(),
        STEEP_ACTION_REWRITE,
        Some(BODY_A),
        &[],
        None,
        Some("claude-code"),
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "review_card");
    assert!(done.revision_card_id.is_some());

    let after = page(&db, "page_h").await;
    assert_eq!(after.content, "my own words", "prose waits for the user");
    assert_eq!(
        after.stale_reason, None,
        "the card is the work that was due"
    );
    assert_eq!(db.list_pending_revisions(10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_note_with_no_sources_left_needs_the_user() {
    let (db, _dir) = test_db().await;
    memory(&db, "mem_a", SRC_A).await;
    stale_page(&db, "page_t", &["mem_a"]).await;
    db.delete_by_source_id("memory", "mem_a").await.unwrap();

    let claim = claim(&db, "page_t", &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(claim.plan, STEEP_PLAN_NEEDS_YOU);
    assert_eq!(claim.token, None);
    assert_eq!(db.steep_leases().holder_of("page_t"), None);
    let after = page(&db, "page_t").await;
    assert_eq!(after.stale_reason.as_deref(), Some("source_removed"));
    assert_eq!(
        after.refresh_blocked_reason.as_deref(),
        Some(NEEDS_YOU_NO_SOURCES)
    );
}

#[tokio::test]
async fn a_removed_source_is_rewritten_out_and_never_waved_through() {
    let (db, _dir) = test_db().await;
    two_source_page(&db).await;
    db.delete_by_source_id("memory", "mem_b").await.unwrap();

    let claim = claim(&db, "page_t", &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(claim.stale_reason, "source_removed");
    assert_eq!(claim.sources.len(), 1, "only the survivor is numbered");
    let token = claim.token.unwrap();

    let error = commit(
        &db,
        &token,
        STEEP_ACTION_STILL_ACCURATE,
        None,
        &[],
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, SteepError::Invalid(_)), "{error:?}");

    let done = commit(
        &db,
        &token,
        STEEP_ACTION_REWRITE,
        Some(BODY_A),
        &[],
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "written");
    let after = page(&db, "page_t").await;
    assert_eq!(after.stale_reason, None);
    assert_eq!(after.source_memory_ids, vec!["mem_a".to_string()]);
}

#[tokio::test]
async fn still_accurate_settles_without_touching_the_prose() {
    let (db, _dir) = test_db().await;
    two_source_page(&db).await;
    let before = page(&db, "page_t").await;
    let token = claim(&db, "page_t", &ReadScope::Global, None)
        .await
        .unwrap()
        .token
        .unwrap();

    let done = commit(
        &db,
        &token,
        STEEP_ACTION_STILL_ACCURATE,
        None,
        &[],
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "still_accurate");
    let after = page(&db, "page_t").await;
    assert_eq!(after.content, before.content);
    assert_eq!(after.version, before.version);
    assert_eq!(after.stale_reason, None);
}

#[tokio::test]
async fn an_uncited_rewrite_is_refused_and_the_hold_kept() {
    let (db, _dir) = test_db().await;
    two_source_page(&db).await;
    let token = claim(&db, "page_t", &ReadScope::Global, None)
        .await
        .unwrap()
        .token
        .unwrap();

    // A claim no source makes, cited anyway; and a sentence that names a
    // source number the claim never handed out, beside well cited ones.
    let invented = "Tally syncs receipts to Dropbox every hour over a private VPN [1].";
    let out_of_range = format!("{BODY_AB}\n\nTally also tours Norway each summer [9].");
    for bad in [
        "A rewrite that cites nothing.",
        invented,
        out_of_range.as_str(),
    ] {
        let error = commit(
            &db,
            &token,
            STEEP_ACTION_REWRITE,
            Some(bad),
            &[],
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, SteepError::CitationGate(_)), "{error:?}");
    }
    assert_eq!(
        db.steep_leases().holder_of("page_t"),
        Some(SteepHolder::OnDemand)
    );
    assert_eq!(page(&db, "page_t").await.content, "old body");

    let done = commit(
        &db,
        &token,
        STEEP_ACTION_REWRITE,
        Some(BODY_AB),
        &[],
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "written");
}

#[tokio::test]
async fn automatic_and_on_demand_never_hold_the_same_note() {
    let (db, _dir) = test_db().await;
    two_source_page(&db).await;

    {
        let hold = hold_for_automatic(&db, "page_t").expect("free note");
        let error = claim(&db, "page_t", &ReadScope::Global, None)
            .await
            .unwrap_err();
        assert!(
            matches!(error, SteepError::Held(SteepHolder::Automatic)),
            "{error:?}"
        );
        drop(hold);
    }

    let token = claim(&db, "page_t", &ReadScope::Global, None)
        .await
        .unwrap()
        .token
        .unwrap();
    assert!(hold_for_automatic(&db, "page_t").is_none());
    let error = claim(&db, "page_t", &ReadScope::Global, None)
        .await
        .unwrap_err();
    assert!(
        matches!(error, SteepError::Held(SteepHolder::OnDemand)),
        "{error:?}"
    );

    commit(&db, &token, STEEP_ACTION_RELEASE, None, &[], None, None)
        .await
        .unwrap();
    assert!(hold_for_automatic(&db, "page_t").is_some());
}

#[tokio::test]
async fn a_token_taken_over_after_expiry_is_refused() {
    let (db, _dir) = test_db().await;
    two_source_page(&db).await;
    let old = claim(&db, "page_t", &ReadScope::Global, None)
        .await
        .unwrap()
        .token
        .unwrap();
    // Expire the hold, then let another caller take the note.
    db.steep_leases()
        .lock()
        .get_mut("page_t")
        .unwrap()
        .expires_at = Instant::now();
    let new = claim(&db, "page_t", &ReadScope::Global, None)
        .await
        .unwrap()
        .token
        .unwrap();
    assert_ne!(old, new);

    let error = commit(
        &db,
        &old,
        STEEP_ACTION_REWRITE,
        Some(BODY_AB),
        &[],
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, SteepError::TokenGone), "{error:?}");
    let done = commit(
        &db,
        &new,
        STEEP_ACTION_REWRITE,
        Some(BODY_AB),
        &[],
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "written");
}

#[tokio::test]
async fn the_work_list_counts_both_reasons_and_pages_with_a_cursor() {
    let (db, _dir) = test_db().await;
    memory(&db, "mem_a", SRC_A).await;
    memory(&db, "mem_b", SRC_B).await;
    memory(&db, "mem_c", SRC_B).await;
    stale_page(&db, "page_1", &["mem_a"]).await;
    stale_page(&db, "page_2", &["mem_b"]).await;
    stale_page(&db, "page_3", &["mem_c"]).await;
    db.delete_by_source_id("memory", "mem_c").await.unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    db.insert_page(
        "page_fresh",
        "Fresh",
        None,
        "b",
        None,
        None,
        &["mem_a"],
        &now,
    )
    .await
    .unwrap();

    let first = list_work(&db, &ReadScope::Global, |_| true, 2, None, false)
        .await
        .unwrap();
    assert_eq!(first.total, 3);
    assert_eq!(first.items.len(), 2);
    let cursor = first.next_cursor.expect("a second page");
    let second = list_work(&db, &ReadScope::Global, |_| true, 2, Some(&cursor), false)
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.next_cursor, None);

    let all: Vec<_> = first.items.iter().chain(second.items.iter()).collect();
    let removed = all.iter().find(|i| i.page_id == "page_3").unwrap();
    assert_eq!(removed.stale_reason, "source_removed");
    assert!(removed.needs_you);
    assert!(all.iter().all(|i| i.page_id != "page_fresh"));

    let hidden = list_work(
        &db,
        &ReadScope::Global,
        |id| id != "page_2",
        10,
        None,
        false,
    )
    .await
    .unwrap();
    assert_eq!(hidden.total, 2);
}

async fn set_page_space(db: &MemoryDB, id: &str, space: &str) {
    db.test_primary_session()
        .await
        .execute(
            "UPDATE pages SET space = ?2 WHERE id = ?1",
            libsql::params![id, space],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_page_moved_out_of_the_claimers_space_is_not_written() {
    let (db, _dir) = test_db().await;
    two_source_page(&db).await;
    set_page_space(&db, "page_t", "test").await;
    let scope = ReadScope::Space("test".into());
    let token = claim(&db, "page_t", &scope, None)
        .await
        .unwrap()
        .token
        .unwrap();
    // The user moves the page to another Space while the AI tool rewrites it.
    set_page_space(&db, "page_t", "other").await;

    let error = commit(
        &db,
        &token,
        STEEP_ACTION_REWRITE,
        Some(BODY_AB),
        &[],
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, SteepError::NotFound), "{error:?}");
    let after = page(&db, "page_t").await;
    assert_eq!(after.content, "old body");
    assert_eq!(after.stale_reason.as_deref(), Some("source_updated"));
}

#[tokio::test]
async fn the_steep_write_rechecks_the_space_inside_its_transaction() {
    let (db, _dir) = test_db().await;
    two_source_page(&db).await;
    set_page_space(&db, "page_t", "other").await;
    let fence = db.try_get_page_fence("page_t").await.unwrap().unwrap();
    let version = page(&db, "page_t").await.version;

    let error = crate::post_write::steep_update_page(
        &db,
        "page_t",
        UpdatePageRequest {
            content: BODY_A.to_string(),
            source_memory_ids: vec!["mem_a".into(), "mem_b".into()],
            expected_version: Some(version),
            caller_id: None,
            operation_id: None,
        },
        Some("claude-code"),
        Some(ReadScope::Space("test".into())),
        fence.source_revision,
        None,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, WenlanError::NotFound(_)), "{error:?}");
    assert_eq!(page(&db, "page_t").await.content, "old body");
}

#[tokio::test]
async fn a_note_the_user_edited_gets_a_code_card_and_leaves_the_list_until_they_decide() {
    use crate::code_basis::CODE_BASIS_STATE_PENDING;
    use crate::db::steep_code_test::{
        changed_note, judge_with, only, reasons, verdict, Repo, BODY, FIXED,
    };
    // Staging a revision card embeds it, so this one needs the embedder.
    let (db, _dir) = crate::db::tests::test_db().await;
    let repo = Repo::new();
    changed_note(&db, &repo, "page_u", "authored").await;
    db.test_primary_session()
        .await
        .execute("UPDATE pages SET user_edited = 1 WHERE id = 'page_u'", ())
        .await
        .unwrap();
    judge_with(
        &db,
        "page_u",
        &verdict("stale", "The note should say 48 bits."),
    )
    .await;

    let claimed = claim(&db, "page_u", &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(claimed.plan, STEEP_PLAN_REVIEW_CARD);
    let done = commit(
        &db,
        claimed.token.as_deref().unwrap(),
        STEEP_ACTION_REWRITE,
        Some(FIXED),
        &[],
        None,
        Some("claude-code"),
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "review_card");
    assert!(done.revision_card_id.is_some());
    assert_eq!(page(&db, "page_u").await.content, BODY);
    // The card settles nothing; the note just stops being offered.
    assert_eq!(only(&db, "page_u").await.state, CODE_BASIS_STATE_PENDING);
    assert!(reasons(&db, true).await.is_empty());
    assert!(matches!(
        claim(&db, "page_u", &ReadScope::Global, None).await,
        Err(SteepError::NotWaiting(_))
    ));
}
