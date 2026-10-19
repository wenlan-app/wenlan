// SPDX-License-Identifier: Apache-2.0
//! DB-level tests for the page basis (a note relying on another note).
//!
//! Numbers in the test names refer to the acceptance list in the B1 plan
//! (`2026-10-17-page-cites-page-and-lock-toggle.plan.md`).

use super::page_basis::*;
use super::tests::{test_db, test_db_at};
use super::MemoryDB;
use crate::error::WenlanError;
use std::sync::Arc;

const T0: &str = "2026-01-01T00:00:00Z";

async fn page(db: &MemoryDB, id: &str, title: &str, content: &str, space: &str) {
    db.insert_page_with_kind(
        id,
        title,
        None,
        content,
        None,
        Some(space),
        &[],
        T0,
        "authored",
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
}

/// A history-bearing content write, with the `minor` flag the public wrappers
/// do not yet expose.
async fn edit(db: &MemoryDB, id: &str, content: &str, minor: bool) {
    let updated = db
        .try_update_page_content(
            id,
            content,
            &[],
            "manual_edit",
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            false,
            minor,
        )
        .await
        .unwrap();
    assert!(updated, "the fixture edit of {id} must land");
}

async fn entry(db: &MemoryDB, citer: &str, basis: &str) -> PageBasisEntry {
    db.list_page_basis(citer)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.basis_page_id == basis)
        .unwrap_or_else(|| panic!("{citer} has no basis row for {basis}"))
}

async fn scalar(db: &MemoryDB, sql: &str, id: &str) -> i64 {
    let conn = db.conn.lock().await;
    let mut rows = conn.query(sql, libsql::params![id]).await.unwrap();
    rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
}

async fn version_of(db: &MemoryDB, id: &str) -> i64 {
    scalar(db, "SELECT version FROM pages WHERE id = ?1", id).await
}

async fn history_rows(db: &MemoryDB, id: &str) -> i64 {
    scalar(
        db,
        "SELECT COUNT(*) FROM page_history WHERE page_id = ?1",
        id,
    )
    .await
}

async fn basis_revision(db: &MemoryDB, id: &str) -> i64 {
    scalar(db, "SELECT basis_revision FROM pages WHERE id = ?1", id).await
}

async fn set_status(db: &MemoryDB, id: &str, status: &str) {
    let conn = db.conn.lock().await;
    conn.execute(
        "UPDATE pages SET status = ?2 WHERE id = ?1",
        libsql::params![id, status],
    )
    .await
    .unwrap();
}

async fn set_kind(db: &MemoryDB, id: &str, kind: &str) {
    let conn = db.conn.lock().await;
    conn.execute(
        "UPDATE pages SET kind = ?2 WHERE id = ?1",
        libsql::params![id, kind],
    )
    .await
    .unwrap();
}

/// Two notes in one Space: `a` (the basis) and `b` (the citer), with `b`
/// already relying on `a` at its first version.
async fn pair() -> (MemoryDB, tempfile::TempDir) {
    let (db, dir) = test_db().await;
    page(&db, "a", "Alpha", "The launch is on 2026-03-01.", "sp").await;
    page(&db, "b", "Beta", "Plans built on Alpha.", "sp").await;
    db.mark_page_basis("b", "a", 1, "human").await.unwrap();
    (db, dir)
}

// ---------------------------------------------------------------------------
// Migration 138
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_138_adds_the_table_the_index_and_both_columns_idempotently() {
    let (db, dir) = test_db_at(137).await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "INSERT INTO pages (id,title,content,created_at,last_compiled,last_modified)
             VALUES ('legacy','Legacy','body','2026-01-01T00:00:00Z',
                     '2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO page_history (page_id,version,title,content,edited_by,created_at)
             VALUES ('legacy',1,'Legacy','body','create',1)",
            (),
        )
        .await
        .unwrap();
    }
    drop(db);

    // Opening the database runs 138; opening it again must be a no-op.
    for _ in 0..2 {
        let db = MemoryDB::new(dir.path(), Arc::new(crate::events::NoopEmitter))
            .await
            .unwrap();
        assert_eq!(
            scalar(
                &db,
                "SELECT minor FROM page_history WHERE page_id = ?1",
                "legacy"
            )
            .await,
            0,
            "existing history rows default to not minor"
        );
        assert_eq!(basis_revision(&db, "legacy").await, 0);
        let conn = db.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE name IN ('page_basis','page_basis_by_basis')",
                (),
            )
            .await
            .unwrap();
        let mut names = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            names.push(row.get::<String>(0).unwrap());
        }
        names.sort();
        assert_eq!(names, vec!["page_basis", "page_basis_by_basis"]);
        let mut version = conn.query("PRAGMA user_version", ()).await.unwrap();
        let user_version: i64 = version.next().await.unwrap().unwrap().get(0).unwrap();
        assert!(user_version >= 138, "user_version was {user_version}");
    }
}

#[tokio::test]
async fn page_basis_state_check_allows_unavailable_and_nothing_else() {
    let (db, _dir) = test_db().await;
    let conn = db.conn.lock().await;
    let insert = |state: &'static str| {
        let conn = &conn;
        async move {
            conn.execute(
                "INSERT OR REPLACE INTO page_basis
                   (page_id,basis_page_id,basis_title,confirmed_version,confirmed_digest,
                    confirmed_by,confirmed_at,state,added_by,added_at,basis_incarnation)
                 VALUES ('x','y','Y',1,'d','human',1,?1,'human',1,'inc')",
                libsql::params![state],
            )
            .await
        }
    };
    for state in ["ok", "pending", "archived", "deleted", "unavailable"] {
        insert(state)
            .await
            .unwrap_or_else(|e| panic!("{state}: {e}"));
    }
    assert!(insert("bogus").await.is_err());
}

// ---------------------------------------------------------------------------
// Acceptance 1: mark
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptance_1_mark_lists_the_basis_as_ok_with_who_and_when() {
    let (db, _dir) = pair().await;
    let basis = entry(&db, "b", "a").await;
    assert_eq!(basis.state, BASIS_STATE_OK);
    assert_eq!(basis.title, "Alpha");
    assert_eq!(basis.confirmed_version, 1);
    assert_eq!(basis.current_version, Some(1));
    assert_eq!(basis.confirmed_by, "human");
    assert_eq!(basis.added_by, "human");
    assert!(basis.changed_version.is_none());
}

#[tokio::test]
async fn acceptance_1_mark_refuses_each_bad_target_with_its_own_reason() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "alpha text", "sp").await;
    page(&db, "b", "Beta", "beta text", "sp").await;
    page(&db, "other", "Elsewhere", "text", "sp2").await;
    page(&db, "gone", "Archived", "text", "sp").await;
    page(&db, "shadow", "Shadow", "text", "sp").await;
    page(&db, "draft", "Draft", "text", "sp").await;
    set_status(&db, "gone", "archived").await;
    set_kind(&db, "shadow", "entity").await;
    set_status(&db, "draft", "draft").await;

    let refused = |result: Result<PageBasisEntry, BasisError>| result.unwrap_err();
    assert!(matches!(
        refused(db.mark_page_basis("b", "b", 1, "human").await),
        BasisError::SelfReference
    ));
    assert!(matches!(
        refused(db.mark_page_basis("b", "missing", 1, "human").await),
        BasisError::BasisNotFound
    ));
    assert!(matches!(
        refused(db.mark_page_basis("b", "gone", 1, "human").await),
        BasisError::BasisArchived
    ));
    assert!(matches!(
        refused(db.mark_page_basis("b", "other", 1, "human").await),
        BasisError::DifferentSpace
    ));
    assert!(matches!(
        refused(db.mark_page_basis("b", "shadow", 1, "human").await),
        BasisError::BasisIsEntityPage
    ));
    assert!(matches!(
        refused(db.mark_page_basis("b", "draft", 1, "human").await),
        BasisError::BasisIsDraft
    ));
    assert!(matches!(
        refused(db.mark_page_basis("b", "a", 9, "human").await),
        BasisError::InvalidVersion {
            requested: 9,
            current: 1
        }
    ));
    assert!(matches!(
        refused(db.mark_page_basis("b", "a", 0, "human").await),
        BasisError::InvalidVersion { .. }
    ));
    // The citer must be an active, non-draft, non-entity note too.
    assert!(matches!(
        refused(db.mark_page_basis("missing", "a", 1, "human").await),
        BasisError::CiterNotFound
    ));
    assert!(matches!(
        refused(db.mark_page_basis("gone", "a", 1, "human").await),
        BasisError::CiterNotActive
    ));
    assert!(matches!(
        refused(db.mark_page_basis("draft", "a", 1, "human").await),
        BasisError::CiterIsDraft
    ));
    assert!(matches!(
        refused(db.mark_page_basis("shadow", "a", 1, "human").await),
        BasisError::CiterIsEntityPage
    ));
    assert!(db.list_page_basis("b").await.unwrap().is_empty());
}

#[tokio::test]
async fn unfiled_notes_share_a_space_and_the_unfiled_note_is_not_in_a_named_one() {
    let (db, _dir) = test_db().await;
    db.insert_page_with_kind(
        "u1",
        "Unfiled one",
        None,
        "one",
        None,
        None,
        &[],
        T0,
        "authored",
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
    db.insert_page_with_kind(
        "u2",
        "Unfiled two",
        None,
        "two",
        None,
        None,
        &[],
        T0,
        "authored",
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
    page(&db, "n1", "Named", "three", "sp").await;
    db.mark_page_basis("u2", "u1", 1, "human").await.unwrap();
    assert!(matches!(
        db.mark_page_basis("n1", "u1", 1, "human")
            .await
            .unwrap_err(),
        BasisError::DifferentSpace
    ));
}

#[tokio::test]
async fn basis_errors_map_to_validation_conflict_and_not_found() {
    let validation = [
        BasisError::SelfReference,
        BasisError::DifferentSpace,
        BasisError::BasisArchived,
        BasisError::CiterIsDraft,
        BasisError::InvalidVersion {
            requested: 3,
            current: 2,
        },
    ];
    for error in validation {
        assert!(matches!(
            WenlanError::from(error),
            WenlanError::Validation(_)
        ));
    }
    assert!(matches!(
        WenlanError::from(BasisError::MovedOn { current: 4 }),
        WenlanError::Conflict(_)
    ));
    assert!(matches!(
        WenlanError::from(BasisError::BasisUnavailable(
            BasisUnavailableReason::OtherSpace
        )),
        WenlanError::Conflict(_)
    ));
    for error in [
        BasisError::BasisNotFound,
        BasisError::CiterNotFound,
        BasisError::NotABasis,
        BasisError::BasisDeleted,
    ] {
        assert!(matches!(WenlanError::from(error), WenlanError::NotFound(_)));
    }
}

// ---------------------------------------------------------------------------
// Acceptance 2 and 14: what counts as a change
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptance_2_reformatting_the_basis_keeps_dependents_ok() {
    let (db, _dir) = pair().await;
    // Bold, a heading level, blank lines and a list marker around the same words.
    edit(&db, "a", "The launch is on **2026-03-01**.\n\n\n", false).await;
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_OK);
    edit(&db, "a", "# The launch is on 2026-03-01.", false).await;
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_OK);
    edit(&db, "a", "### The launch is on 2026-03-01.", false).await;
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_OK);
    assert_eq!(
        version_of(&db, "a").await,
        4,
        "each reformat is a real version"
    );
}

#[tokio::test]
async fn acceptance_14_a_character_level_change_is_pending_and_an_undo_clears_it() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "Use foo_bar here.", "sp").await;
    page(&db, "b", "Beta", "Built on Alpha.", "sp").await;
    db.mark_page_basis("b", "a", 1, "human").await.unwrap();

    edit(&db, "a", "Use foobar here.", false).await;
    let pending = entry(&db, "b", "a").await;
    assert_eq!(pending.state, BASIS_STATE_PENDING);
    assert_eq!(pending.changed_version, Some(2));

    // Undoing the edit clears the notice by itself (two-way re-evaluation).
    edit(&db, "a", "Use foo_bar here.", false).await;
    let cleared = entry(&db, "b", "a").await;
    assert_eq!(cleared.state, BASIS_STATE_OK);
    assert!(cleared.changed_version.is_none());
    assert_eq!(
        cleared.confirmed_version, 1,
        "an undo is not a new confirmation"
    );
}

// ---------------------------------------------------------------------------
// Acceptance 3: pending and the diff
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptance_3_a_changed_date_is_pending_and_the_diff_has_both_texts() {
    let (db, _dir) = pair().await;
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;

    let pending = entry(&db, "b", "a").await;
    assert_eq!(pending.state, BASIS_STATE_PENDING);
    assert_eq!(pending.changed_version, Some(2));
    assert_eq!(pending.current_version, Some(2));
    assert_eq!(pending.confirmed_version, 1);
    assert!(!pending.minor_only);

    let BasisDiffOutcome::Available(diff) = db.basis_diff("b", "a").await.unwrap() else {
        panic!("the diff must be available");
    };
    assert_eq!((diff.from_version, diff.to_version), (1, 2));
    assert_eq!(diff.from_content, "The launch is on 2026-03-01.");
    assert_eq!(diff.to_content, "The launch is on 2026-04-01.");
    assert_eq!(diff.title, "Alpha");
    assert_eq!(diff.edits.len(), 1);
    assert_eq!(diff.edits[0].version, 2);
    assert!(!diff.edits[0].minor);
}

#[tokio::test]
async fn minor_only_holds_only_while_every_version_since_the_confirmed_one_is_minor() {
    let (db, _dir) = pair().await;
    edit(&db, "a", "The launch is on 2026-03-01!", true).await;
    edit(&db, "a", "The launch is on 2026-03-01!!", true).await;
    let minor = entry(&db, "b", "a").await;
    assert_eq!(minor.state, BASIS_STATE_PENDING);
    assert!(minor.minor_only, "two minor edits are minor only");

    edit(&db, "a", "The launch is on 2026-05-01!!", false).await;
    let major = entry(&db, "b", "a").await;
    assert_eq!(major.state, BASIS_STATE_PENDING);
    assert!(!major.minor_only, "one real edit ends minor-only");
    assert_eq!(major.changed_version, Some(4));

    // A minor edit does not suppress pending: the state is pending either way.
    let BasisDiffOutcome::Available(diff) = db.basis_diff("b", "a").await.unwrap() else {
        panic!("diff");
    };
    let flags: Vec<bool> = diff.edits.iter().map(|e| e.minor).collect();
    assert_eq!(flags, vec![true, true, false]);
}

// ---------------------------------------------------------------------------
// Acceptance 4: confirm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptance_4_confirm_with_a_stale_version_is_refused_and_the_current_one_records_who() {
    let (db, _dir) = pair().await;
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    edit(&db, "a", "The launch is on 2026-05-01.", false).await;

    let stale = db
        .confirm_page_basis("b", "a", 2, "human")
        .await
        .unwrap_err();
    assert!(
        matches!(stale, BasisError::MovedOn { current: 3 }),
        "{stale:?}"
    );
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_PENDING);

    let confirmed = db
        .confirm_page_basis("b", "a", 3, "claude-code")
        .await
        .unwrap();
    assert_eq!(confirmed.state, BASIS_STATE_OK);
    assert_eq!(confirmed.confirmed_version, 3);
    assert_eq!(confirmed.confirmed_by, "claude-code");
    assert!(confirmed.changed_version.is_none());
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_OK);

    // The confirmation holds: the same text stays ok, a new change is pending.
    edit(
        &db,
        "a",
        "The launch is on 2026-05-01. Doors at six.",
        false,
    )
    .await;
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_PENDING);
}

#[tokio::test]
async fn confirm_on_a_note_that_is_not_a_basis_is_refused() {
    let (db, _dir) = pair().await;
    page(&db, "c", "Gamma", "text", "sp").await;
    let error = db
        .confirm_page_basis("b", "c", 1, "human")
        .await
        .unwrap_err();
    assert!(matches!(error, BasisError::NotABasis));
}

#[tokio::test]
async fn confirm_joins_the_callers_transaction_and_rolls_back_with_it() {
    let (db, _dir) = pair().await;
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    {
        let conn = db.conn.lock().await;
        let tx = conn.transaction().await.unwrap();
        let confirmed = confirm_page_basis_on_conn(&tx, "b", "a", 2, "human")
            .await
            .unwrap();
        assert_eq!(confirmed.state, BASIS_STATE_OK);
        // The caller's save fails after the confirm: nothing may persist.
        tx.rollback().await.unwrap();
    }
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_PENDING);

    {
        let conn = db.conn.lock().await;
        let tx = conn.transaction().await.unwrap();
        // The check runs inside the caller's transaction too.
        let stale = confirm_page_basis_on_conn(&tx, "b", "a", 1, "human")
            .await
            .unwrap_err();
        assert!(matches!(stale, BasisError::MovedOn { current: 2 }));
        tx.rollback().await.unwrap();
    }
}

#[tokio::test]
async fn mark_unmark_and_confirm_are_metadata_only() {
    let (db, _dir) = pair().await;
    let version = version_of(&db, "b").await;
    let history = history_rows(&db, "b").await;
    let revision = basis_revision(&db, "b").await;
    assert_eq!(revision, 1, "the fixture's mark bumped the revision once");

    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    db.confirm_page_basis("b", "a", 2, "human").await.unwrap();
    assert_eq!(basis_revision(&db, "b").await, 2);
    assert!(db.unmark_page_basis("b", "a").await.unwrap());
    assert_eq!(basis_revision(&db, "b").await, 3);
    assert!(
        !db.unmark_page_basis("b", "a").await.unwrap(),
        "nothing left to remove"
    );
    assert_eq!(
        basis_revision(&db, "b").await,
        3,
        "a no-op unmark changes nothing"
    );

    assert_eq!(
        version_of(&db, "b").await,
        version,
        "the citer's version never moves"
    );
    assert_eq!(
        history_rows(&db, "b").await,
        history,
        "no history row is written"
    );
    assert!(db.list_page_basis("b").await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Acceptance 5: grouping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptance_5_twenty_citers_of_one_basis_are_one_group() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "The launch is on 2026-03-01.", "sp").await;
    page(&db, "elsewhere", "Beyond", "text", "sp2").await;
    page(&db, "elsewhere-basis", "Beyond basis", "text", "sp2").await;
    for n in 0..20 {
        let id = format!("citer-{n:02}");
        page(&db, &id, &format!("Citer {n:02}"), "text", "sp").await;
        db.mark_page_basis(&id, "a", 1, "human").await.unwrap();
    }
    db.mark_page_basis("elsewhere", "elsewhere-basis", 1, "human")
        .await
        .unwrap();
    assert!(db.list_basis_pending(None).await.unwrap().is_empty());

    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    edit(&db, "elsewhere-basis", "changed", false).await;

    let groups = db.list_basis_pending(Some("sp")).await.unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].basis_page_id, "a");
    assert_eq!(groups[0].title, "Alpha");
    assert_eq!(groups[0].state, BASIS_STATE_PENDING);
    assert_eq!(groups[0].changed_version, Some(2));
    assert_eq!(groups[0].citers.len(), 20);
    assert_eq!(groups[0].citers[0].title, "Citer 00");

    let everywhere = db.list_basis_pending(None).await.unwrap();
    assert_eq!(everywhere.len(), 2, "one group per basis note");
}

// ---------------------------------------------------------------------------
// Acceptance 6: archive and delete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptance_6_archive_reads_archived_and_leaving_archive_re_evaluates() {
    let (db, _dir) = pair().await;
    db.archive_page("a").await.unwrap();
    let archived = entry(&db, "b", "a").await;
    assert_eq!(archived.state, BASIS_STATE_ARCHIVED);
    assert_eq!(archived.title, "Alpha");
    let groups = db.list_basis_pending(None).await.unwrap();
    assert_eq!(groups[0].state, BASIS_STATE_ARCHIVED);

    // A confirm against an archived basis is refused, not silently accepted.
    let refused = db
        .confirm_page_basis("b", "a", 2, "human")
        .await
        .unwrap_err();
    assert!(matches!(
        refused,
        BasisError::BasisUnavailable(BasisUnavailableReason::Archived)
    ));

    // Restore: the same machinery every restore path uses appends the history
    // row, and the row is evaluated against the text again.
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE pages SET status = 'active', version = version + 1 WHERE id = 'a'",
            (),
        )
        .await
        .unwrap();
        MemoryDB::append_page_history(&conn, "a", "restore", 5)
            .await
            .unwrap();
    }
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_OK);
}

#[tokio::test]
async fn acceptance_6_delete_reads_deleted_keeps_the_title_and_leaves_the_citer_readable() {
    let (db, _dir) = pair().await;
    db.delete_page("a").await.unwrap();

    let deleted = entry(&db, "b", "a").await;
    assert_eq!(deleted.state, BASIS_STATE_DELETED);
    assert_eq!(deleted.title, "Alpha", "the title survives the delete");
    assert!(deleted.current_version.is_none());
    assert!(
        db.get_page("b").await.unwrap().is_some(),
        "the citer is untouched"
    );
    assert!(matches!(
        db.basis_diff("b", "a").await.unwrap(),
        BasisDiffOutcome::Unavailable(BasisUnavailableReason::Deleted)
    ));
    assert!(matches!(
        db.confirm_page_basis("b", "a", 1, "human")
            .await
            .unwrap_err(),
        BasisError::BasisUnavailable(BasisUnavailableReason::Deleted)
    ));
    // Marking the deleted note again says it was deleted, not "not found".
    assert!(matches!(
        db.mark_page_basis("b", "a", 1, "human").await.unwrap_err(),
        BasisError::BasisDeleted
    ));
    assert_eq!(
        db.list_basis_pending(None).await.unwrap()[0].state,
        BASIS_STATE_DELETED
    );
}

#[tokio::test]
async fn deleting_the_citer_removes_the_rows_it_held() {
    let (db, _dir) = pair().await;
    db.delete_page("b").await.unwrap();
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM page_basis WHERE page_id = ?1",
            "b"
        )
        .await,
        0
    );
    assert!(db.list_page_basis("b").await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Acceptance 9: a stale read never confirms
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptance_9_marking_with_a_version_that_moved_on_lands_pending_for_good() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "The launch is on 2026-03-01.", "sp").await;
    page(&db, "b", "Beta", "Plans.", "sp").await;
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;

    // The agent read version 1; the note is at version 2 when it writes.
    let marked = db
        .mark_page_basis("b", "a", 1, "claude-code")
        .await
        .unwrap();
    assert_eq!(marked.state, BASIS_STATE_PENDING);
    assert_eq!(marked.changed_version, Some(2));
    assert_eq!(marked.confirmed_version, 1);

    // Even if the text later reads like what the agent saw, nobody confirmed it.
    edit(&db, "a", "The launch is on 2026-03-01.", false).await;
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_PENDING);

    // Only an explicit confirm at the current version clears it.
    let confirmed = db
        .confirm_page_basis("b", "a", 3, "claude-code")
        .await
        .unwrap();
    assert_eq!(confirmed.state, BASIS_STATE_OK);
}

// ---------------------------------------------------------------------------
// Acceptance 11: Space moves
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptance_11_moving_the_basis_to_another_space_is_unavailable_and_moving_back_re_evaluates(
) {
    let (db, _dir) = pair().await;
    db.set_page_workspace("a", Some("sp2")).await.unwrap();

    let moved = entry(&db, "b", "a").await;
    assert_eq!(moved.state, BASIS_STATE_UNAVAILABLE);
    assert!(matches!(
        db.basis_diff("b", "a").await.unwrap(),
        BasisDiffOutcome::Unavailable(BasisUnavailableReason::OtherSpace)
    ));
    assert!(matches!(
        db.confirm_page_basis("b", "a", 1, "human")
            .await
            .unwrap_err(),
        BasisError::BasisUnavailable(BasisUnavailableReason::OtherSpace)
    ));

    // While away it is edited, then it comes back: the row is evaluated
    // against the text now, so it is pending, not ok.
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_UNAVAILABLE);
    db.set_page_workspace("a", Some("sp")).await.unwrap();
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_PENDING);
}

#[tokio::test]
async fn moving_the_citer_or_both_notes_follows_the_same_rule() {
    let (db, _dir) = pair().await;
    db.set_page_workspace("b", Some("sp2")).await.unwrap();
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_UNAVAILABLE);
    db.set_page_workspace("a", Some("sp2")).await.unwrap();
    assert_eq!(
        entry(&db, "b", "a").await.state,
        BASIS_STATE_OK,
        "moving both notes together keeps the row"
    );
    // Unfiled is a Space like any other.
    db.set_page_workspace("a", None).await.unwrap();
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_UNAVAILABLE);
    db.set_page_workspace("b", None).await.unwrap();
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_OK);
}

async fn deleted_space_brings_a_note_home(action: &str) {
    let (db, _dir) = test_db().await;
    // Both notes unfiled; the basis visits a registered Space.
    for (id, title) in [("a", "Alpha"), ("b", "Beta")] {
        db.insert_page_with_kind(
            id,
            title,
            None,
            "text",
            None,
            None,
            &[],
            T0,
            "authored",
            "confirmed",
            None,
            None,
        )
        .await
        .unwrap();
    }
    db.create_space("away", None, false).await.unwrap();
    db.mark_page_basis("b", "a", 1, "human").await.unwrap();
    db.set_page_workspace("a", Some("away")).await.unwrap();
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_UNAVAILABLE);

    db.delete_space("away", action).await.unwrap();
    assert_eq!(
        entry(&db, "b", "a").await.state,
        BASIS_STATE_OK,
        "delete_space({action}) put the basis back with its citer"
    );
}

#[tokio::test]
async fn delete_space_unassign_re_evaluates_basis_rows() {
    deleted_space_brings_a_note_home("unassign").await;
}

#[tokio::test]
async fn delete_space_delete_re_evaluates_basis_rows() {
    deleted_space_brings_a_note_home("delete").await;
}

#[tokio::test]
async fn delete_space_move_and_reassign_re_evaluate_basis_rows() {
    for reassign in [false, true] {
        let (db, _dir) = test_db().await;
        page(&db, "a", "Alpha", "text", "home").await;
        page(&db, "b", "Beta", "text", "home").await;
        db.create_space("home", None, false).await.unwrap();
        db.create_space("away", None, false).await.unwrap();
        db.mark_page_basis("b", "a", 1, "human").await.unwrap();
        db.set_page_workspace("a", Some("away")).await.unwrap();
        assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_UNAVAILABLE);

        if reassign {
            db.reassign_memories_space("away", "home").await.unwrap();
        } else {
            db.delete_space("away", "move:home").await.unwrap();
        }
        assert_eq!(
            entry(&db, "b", "a").await.state,
            BASIS_STATE_OK,
            "reassign={reassign}"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance 12: merge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptance_12_merge_leaves_citers_of_the_absorbed_note_stale_and_copies_its_bases_as_pending(
) {
    let (db, _dir) = test_db().await;
    for (id, title) in [
        ("s", "Survivor"),
        ("x", "Absorbed"),
        ("c", "Citer of X"),
        ("y", "Basis both share"),
        ("z", "Basis only X had"),
    ] {
        page(&db, id, title, &format!("{title} text"), "sp").await;
    }
    db.mark_page_basis("c", "x", 1, "human").await.unwrap();
    db.mark_page_basis("x", "y", 1, "human").await.unwrap();
    db.mark_page_basis("x", "z", 1, "human").await.unwrap();
    db.mark_page_basis("x", "s", 1, "human").await.unwrap();
    db.mark_page_basis("s", "y", 1, "human").await.unwrap();

    db.insert_refinement_proposal(
        "proposal",
        "page_merge",
        &["s".to_string(), "x".to_string()],
        None,
        1.0,
    )
    .await
    .unwrap();
    db.accept_page_merge("proposal", "s", "x").await.unwrap();

    // Citers of X now see an archived basis.
    assert_eq!(entry(&db, "c", "x").await.state, BASIS_STATE_ARCHIVED);

    // X's bases are on S, none of them ok unless S already had them.
    let bases = db.list_page_basis("s").await.unwrap();
    let by_id = |id: &str| bases.iter().find(|e| e.basis_page_id == id).cloned();
    assert_eq!(
        by_id("y").unwrap().state,
        BASIS_STATE_OK,
        "S's own confirmed row is kept, not downgraded"
    );
    assert_eq!(by_id("y").unwrap().confirmed_by, "human");
    let copied = by_id("z").expect("X's basis z is copied to the survivor");
    assert_eq!(copied.state, BASIS_STATE_PENDING);
    assert_eq!(copied.confirmed_by, "page_merge");
    assert!(
        by_id("s").is_none() && by_id("x").is_none(),
        "the survivor never becomes its own basis"
    );
    assert_eq!(bases.len(), 2);
    assert!(
        basis_revision(&db, "s").await >= 2,
        "the copy bumps the revision"
    );

    // No later re-evaluation can turn the copy ok, even if z is edited back.
    edit(&db, "z", "something else", false).await;
    edit(&db, "z", "Basis only X had text", false).await;
    assert_eq!(entry(&db, "s", "z").await.state, BASIS_STATE_PENDING);

    // Only an explicit confirm does.
    let confirmed = db.confirm_page_basis("s", "z", 3, "human").await.unwrap();
    assert_eq!(confirmed.state, BASIS_STATE_OK);
}

// ---------------------------------------------------------------------------
// Acceptance 13: id reuse
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptance_13_a_recreated_id_never_reads_as_the_same_basis() {
    let (db, _dir) = pair().await;
    db.delete_page("a").await.unwrap();
    // A new note takes the same id, with the same words even.
    page(&db, "a", "Alpha", "The launch is on 2026-03-01.", "sp").await;

    let row = entry(&db, "b", "a").await;
    assert_eq!(row.state, BASIS_STATE_DELETED);
    assert!(
        row.current_version.is_none(),
        "the recreated note is not the one B relied on"
    );
    assert!(matches!(
        db.basis_diff("b", "a").await.unwrap(),
        BasisDiffOutcome::Unavailable(_)
    ));
    assert!(db.confirm_page_basis("b", "a", 1, "human").await.is_err());

    // Edits to the new note never resurrect the old row.
    edit(&db, "a", "A different note entirely.", false).await;
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_DELETED);

    // Marking the new note is a fresh, explicit decision with its own incarnation.
    let remarked = db.mark_page_basis("b", "a", 2, "human").await.unwrap();
    assert_eq!(remarked.state, BASIS_STATE_OK);
    assert_eq!(remarked.current_version, Some(2));
}

#[tokio::test]
async fn a_row_whose_incarnation_no_longer_matches_reads_deleted_without_the_delete_hook() {
    let (db, _dir) = pair().await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE pages SET incarnation = 'a-different-generation' WHERE id = 'a'",
            (),
        )
        .await
        .unwrap();
    }
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    // The stored state itself, not just the joined read, must say deleted: the
    // re-evaluation may not treat a different generation as a text change.
    let stored = {
        let conn = db.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT state FROM page_basis WHERE page_id = 'b' AND basis_page_id = 'a'",
                (),
            )
            .await
            .unwrap();
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(0)
            .unwrap()
    };
    assert_eq!(stored, BASIS_STATE_DELETED);
    let row = entry(&db, "b", "a").await;
    assert_eq!(row.state, BASIS_STATE_DELETED);
    assert!(matches!(
        db.basis_diff("b", "a").await.unwrap(),
        BasisDiffOutcome::Unavailable(BasisUnavailableReason::IncarnationChanged)
    ));
    assert!(matches!(
        db.confirm_page_basis("b", "a", 2, "human")
            .await
            .unwrap_err(),
        BasisError::BasisUnavailable(BasisUnavailableReason::IncarnationChanged)
    ));
}

// ---------------------------------------------------------------------------
// Diff edge cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_diff_says_unavailable_when_the_confirmed_version_is_no_longer_stored() {
    let (db, _dir) = pair().await;
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "DELETE FROM page_history WHERE page_id = 'a' AND version = 1",
            (),
        )
        .await
        .unwrap();
    }
    assert!(matches!(
        db.basis_diff("b", "a").await.unwrap(),
        BasisDiffOutcome::Unavailable(BasisUnavailableReason::HistoryGone)
    ));
    assert!(matches!(
        db.basis_diff("a", "b").await.unwrap_err(),
        BasisError::NotABasis
    ));
}
