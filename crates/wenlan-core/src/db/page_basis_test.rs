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
            &BasisWrite {
                minor,
                ..BasisWrite::default()
            },
        )
        .await
        .unwrap();
    assert!(updated.is_some(), "the fixture edit of {id} must land");
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
async fn a_note_may_rely_on_a_note_in_any_space() {
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
    page(&db, "n2", "Elsewhere", "four", "sp2").await;
    db.mark_page_basis("u2", "u1", 1, "human").await.unwrap();
    // Named relies on unfiled, unfiled on named, and across two named Spaces.
    for (citer, basis) in [("n1", "u1"), ("u1", "n1"), ("n1", "n2")] {
        let marked = db.mark_page_basis(citer, basis, 1, "human").await.unwrap();
        assert_eq!(marked.state, BASIS_STATE_OK, "{citer} -> {basis}");
    }
    edit(&db, "n2", "four, changed", false).await;
    assert_eq!(entry(&db, "n1", "n2").await.state, BASIS_STATE_PENDING);
    db.confirm_page_basis("n1", "n2", 2, "human").await.unwrap();
    assert_eq!(entry(&db, "n1", "n2").await.state, BASIS_STATE_OK);
}

#[tokio::test]
async fn basis_errors_map_to_validation_conflict_and_not_found() {
    let validation = [
        BasisError::SelfReference,
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
            BasisUnavailableReason::Archived
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
async fn acceptance_11_a_basis_moved_to_another_space_is_still_relied_on() {
    use crate::read_scope::ReadScope;
    let (db, _dir) = pair().await;
    db.set_page_workspace("a", Some("sp2")).await.unwrap();
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_OK);

    // An edit made over there still reaches the note relying on it, and can
    // be read and confirmed from here.
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_PENDING);
    assert!(matches!(
        db.basis_diff("b", "a").await.unwrap(),
        BasisDiffOutcome::Available(_)
    ));
    // An agent held to the citer's Space still cannot read the other note.
    assert!(matches!(
        db.basis_diff_in_scope("b", "a", &ReadScope::Space("sp".into()))
            .await
            .unwrap(),
        BasisDiffOutcome::OutOfScope(id) if id == "a"
    ));
    db.confirm_page_basis("b", "a", 2, "human").await.unwrap();
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_OK);
}

/// The title a citer keeps for a deleted basis is the last one it could read.
/// A note renamed while it sat in another Space does not reveal the new name
/// once it is deleted.
#[tokio::test]
async fn a_rename_in_another_space_does_not_reach_the_title_kept_for_a_deleted_basis() {
    let (db, _dir) = pair().await;
    db.set_page_workspace("a", Some("sp2")).await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute("UPDATE pages SET title = 'Secret plan' WHERE id = 'a'", ())
            .await
            .unwrap();
    }
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_PENDING);

    db.delete_page("a").await.unwrap();

    let deleted = entry(&db, "b", "a").await;
    assert_eq!(deleted.state, BASIS_STATE_DELETED);
    assert_eq!(
        deleted.title, "Alpha",
        "the title from when the citer could last read the note"
    );
}

/// The same holds once the note is back: a rename made while it was away
/// shows up only after it can be read again.
#[tokio::test]
async fn the_kept_title_follows_a_rename_again_once_the_basis_is_back_in_the_citers_space() {
    let (db, _dir) = pair().await;
    db.set_page_workspace("a", Some("sp2")).await.unwrap();
    {
        let conn = db.conn.lock().await;
        conn.execute("UPDATE pages SET title = 'Renamed' WHERE id = 'a'", ())
            .await
            .unwrap();
    }
    db.set_page_workspace("a", Some("sp")).await.unwrap();
    db.delete_page("a").await.unwrap();
    assert_eq!(entry(&db, "b", "a").await.title, "Renamed");
}

#[tokio::test]
async fn moving_the_citer_or_both_notes_keeps_the_row() {
    let (db, _dir) = pair().await;
    for (id, space) in [
        ("b", Some("sp2")),
        ("a", Some("sp2")),
        ("a", None),
        ("b", None),
    ] {
        db.set_page_workspace(id, space).await.unwrap();
        assert_eq!(
            entry(&db, "b", "a").await.state,
            BASIS_STATE_OK,
            "{id} -> {space:?}"
        );
    }
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
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_OK);

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
        assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_OK);

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

#[tokio::test]
async fn the_diff_says_unavailable_when_the_basis_is_archived() {
    let (db, _dir) = pair().await;
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    db.archive_page("a").await.unwrap();
    assert!(matches!(
        db.basis_diff("b", "a").await.unwrap(),
        BasisDiffOutcome::Unavailable(BasisUnavailableReason::Archived)
    ));
}

// ---------------------------------------------------------------------------
// Identity moves
// ---------------------------------------------------------------------------

/// A SOURCE note keeps its content but changes its id when its document is
/// renamed. The rows that name it (as the basis of another note, and as the
/// citer of its own bases) follow it in the same transaction.
#[tokio::test]
async fn rebinding_a_source_note_identity_retargets_both_basis_columns() {
    let (db, _dir) = test_db().await;
    db.upsert_documents(vec![crate::sources::RawDocument {
        source: "memory".to_string(),
        source_id: "old-doc".to_string(),
        title: "memory-old-doc".to_string(),
        content: "A document that will be renamed.".to_string(),
        last_modified: 1,
        memory_type: Some("fact".to_string()),
        space: Some("sp".to_string()),
        source_agent: Some("folder".to_string()),
        confidence: Some(0.9),
        confirmed: Some(false),
        supersede_mode: "hide".to_string(),
        ..Default::default()
    }])
    .await
    .unwrap();
    page(&db, "a", "Alpha", "The launch is on 2026-03-01.", "sp").await;
    page(&db, "b", "Beta", "Plans built on the source note.", "sp").await;
    db.insert_page_with_kind(
        "src_old",
        "Renamed Source Note",
        None,
        "Source body.",
        None,
        Some("sp"),
        &[],
        T0,
        "source",
        "unconfirmed",
        None,
        None,
    )
    .await
    .unwrap();
    // Both directions: `b` relies on the source note, the source note relies on `a`.
    db.mark_page_basis("b", "src_old", 1, "human")
        .await
        .unwrap();
    db.mark_page_basis("src_old", "a", 1, "human")
        .await
        .unwrap();
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    assert_eq!(
        entry(&db, "src_old", "a").await.state,
        BASIS_STATE_PENDING,
        "fixture: the source note's own basis is pending before the rebind"
    );
    let revision_before = basis_revision(&db, "src_old").await;

    db.rebind_source_id_with_source_page("memory", "old-doc", "new-doc", "src_old", "src_new")
        .await
        .unwrap();

    for (column, id) in [("page_id", "src_old"), ("basis_page_id", "src_old")] {
        let n = scalar(
            &db,
            &format!("SELECT COUNT(*) FROM page_basis WHERE {column} = ?1"),
            id,
        )
        .await;
        assert_eq!(n, 0, "no row may still name the old id in {column}");
    }

    // The note that relied on the source note now relies on the new id, still ok.
    let held = entry(&db, "b", "src_new").await;
    assert_eq!(held.state, BASIS_STATE_OK);
    assert_eq!(held.current_version, Some(1));
    // The source note's own pending basis moved with it and stayed pending.
    assert_eq!(entry(&db, "src_new", "a").await.state, BASIS_STATE_PENDING);
    assert!(
        basis_revision(&db, "src_new").await > revision_before,
        "the basis revision carries over and moves on"
    );

    // The incarnation followed too: confirming still works, and a real edit of
    // the renamed note is seen by its citer.
    db.confirm_page_basis("src_new", "a", 2, "human")
        .await
        .unwrap();
    assert_eq!(entry(&db, "src_new", "a").await.state, BASIS_STATE_OK);
    edit(&db, "src_new", "Source body, changed.", false).await;
    assert_eq!(entry(&db, "b", "src_new").await.state, BASIS_STATE_PENDING);
}

// ---------------------------------------------------------------------------
// Write paths: R7 (marks carried by an agent write) and R11 (a human's confirm)
// ---------------------------------------------------------------------------

/// A history-bearing write of `id` carrying `basis`, returning what the write
/// reported (`None` = it did not land).
async fn write_with(
    db: &MemoryDB,
    id: &str,
    content: &str,
    basis: &BasisWrite,
) -> Result<Option<BasisWriteOutcome>, WenlanError> {
    db.try_update_page_content(
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
        basis,
    )
    .await
}

#[tokio::test]
async fn r7_a_write_confirms_a_basis_still_at_the_read_version_and_holds_a_moved_one_pending() {
    let (db, _dir) = test_db().await;
    page(&db, "moved", "Moved", "The launch is on 2026-03-01.", "sp").await;
    page(&db, "steady", "Steady", "Nothing changes here.", "sp").await;
    page(&db, "citer", "Citer", "Plans.", "sp").await;
    edit(&db, "moved", "The launch is on 2026-04-01.", false).await;

    let written = write_with(
        &db,
        "citer",
        "Plans, revised.",
        &BasisWrite {
            actor: "claude-code".into(),
            // The agent read both notes at version 1; "moved" is at 2 now.
            marks: vec![("moved".into(), 1), ("steady".into(), 1)],
            ..BasisWrite::default()
        },
    )
    .await
    .unwrap();
    assert!(written.is_some(), "the write itself lands");

    let moved = entry(&db, "citer", "moved").await;
    assert_eq!(moved.state, BASIS_STATE_PENDING);
    assert_eq!(moved.changed_version, Some(2));
    assert_eq!(moved.confirmed_version, 1);
    let steady = entry(&db, "citer", "steady").await;
    assert_eq!(steady.state, BASIS_STATE_OK);
    assert_eq!(steady.confirmed_by, "claude-code");
    assert_eq!(version_of(&db, "citer").await, 2);
}

#[tokio::test]
async fn r7_a_refused_mark_rolls_the_whole_write_back() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "text", "sp").await;
    page(&db, "citer", "Citer", "Original body.", "sp").await;
    let history = history_rows(&db, "citer").await;

    let error = write_with(
        &db,
        "citer",
        "Changed body.",
        &BasisWrite {
            actor: "claude-code".into(),
            // The first mark is fine; the second names a note that is not there.
            marks: vec![("a".into(), 1), ("ghost".into(), 1)],
            ..BasisWrite::default()
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, WenlanError::NotFound(_)), "{error:?}");

    let stored = db.get_page("citer").await.unwrap().unwrap();
    assert_eq!(stored.content, "Original body.", "the body is untouched");
    assert_eq!(version_of(&db, "citer").await, 1);
    assert_eq!(history_rows(&db, "citer").await, history);
    assert!(
        db.list_page_basis("citer").await.unwrap().is_empty(),
        "the mark before the refused one rolled back too"
    );
}

#[tokio::test]
async fn r11_a_save_confirms_only_a_basis_still_at_the_version_the_human_looked_at() {
    let (db, _dir) = pair().await;
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_PENDING);

    // The human looked at version 2 and saved: confirmed in the save.
    let landed = write_with(
        &db,
        "b",
        "Plans, rechecked.",
        &BasisWrite {
            actor: "human".into(),
            confirm: Some(("a".into(), 2)),
            ..BasisWrite::default()
        },
    )
    .await
    .unwrap()
    .expect("the save lands");
    assert_eq!(
        landed.confirm,
        Some(wenlan_types::BasisConfirmOutcome::Confirmed)
    );
    let confirmed = entry(&db, "b", "a").await;
    assert_eq!(confirmed.state, BASIS_STATE_OK);
    assert_eq!(confirmed.confirmed_version, 2);
    assert_eq!(confirmed.confirmed_by, "human");

    // The basis moves on while the human is still looking at version 2.
    edit(&db, "a", "The launch is on 2026-05-01.", false).await;
    let version_before = version_of(&db, "b").await;
    let landed = write_with(
        &db,
        "b",
        "Plans, rechecked again.",
        &BasisWrite {
            actor: "human".into(),
            confirm: Some(("a".into(), 2)),
            ..BasisWrite::default()
        },
    )
    .await
    .unwrap()
    .expect("the save still lands");
    assert_eq!(
        landed.confirm,
        Some(wenlan_types::BasisConfirmOutcome::MovedOn)
    );
    assert_eq!(
        version_of(&db, "b").await,
        version_before + 1,
        "the human's edit is kept"
    );
    let held = entry(&db, "b", "a").await;
    assert_eq!(
        held.state, BASIS_STATE_PENDING,
        "the stale look confirms nothing"
    );
    assert_eq!(held.confirmed_version, 2);
    assert_eq!(held.changed_version, Some(3));
}

/// A human save carrying a confirm and a retry id: the receipt stores what the
/// confirm really did, so a replay reports the same outcome and writes nothing.
async fn save_with_confirm(
    db: &MemoryDB,
    content: &str,
    operation_id: &str,
    version: i64,
) -> crate::post_write::WriteResult {
    crate::post_write::page_write(
        db,
        crate::post_write::PageWrite::UpdatePreservingSources {
            page_id: "b",
            req: wenlan_types::requests::UpdatePageRequest {
                content: content.to_string(),
                source_memory_ids: Vec::new(),
                expected_version: None,
                caller_id: Some("app".to_string()),
                operation_id: Some(operation_id.to_string()),
            },
            edited_by: "manual_edit",
            knowledge_path: None,
            basis: BasisWrite {
                actor: "human".into(),
                confirm: Some(("a".into(), version)),
                ..BasisWrite::default()
            },
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn replaying_a_save_that_confirmed_a_basis_reports_the_same_outcome() {
    let (db, _dir) = pair().await;
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;

    let first = save_with_confirm(&db, "Plans, rechecked.", "op-confirmed", 2).await;
    assert!(first.wrote);
    assert_eq!(
        first.basis_confirm,
        Some(wenlan_types::BasisConfirmOutcome::Confirmed)
    );
    let version_after_first = version_of(&db, "b").await;

    let replay = save_with_confirm(&db, "Plans, rechecked.", "op-confirmed", 2).await;
    assert_eq!(
        replay.basis_confirm, first.basis_confirm,
        "the replay carries the first call's confirm outcome"
    );
    assert_eq!(
        version_of(&db, "b").await,
        version_after_first,
        "a replay writes nothing"
    );

    // The basis moves on; the human's confirm of version 2 now misses.
    edit(&db, "a", "The launch is on 2026-05-01.", false).await;
    let missed = save_with_confirm(&db, "Plans, again.", "op-moved", 2).await;
    assert_eq!(
        missed.basis_confirm,
        Some(wenlan_types::BasisConfirmOutcome::MovedOn)
    );
    let replay = save_with_confirm(&db, "Plans, again.", "op-moved", 2).await;
    assert_eq!(replay.basis_confirm, missed.basis_confirm);
}

#[tokio::test]
async fn r11_a_confirm_for_a_note_that_is_not_a_basis_fails_the_save() {
    let (db, _dir) = pair().await;
    page(&db, "c", "Gamma", "text", "sp").await;
    let error = write_with(
        &db,
        "b",
        "Plans, changed.",
        &BasisWrite {
            actor: "human".into(),
            confirm: Some(("c".into(), 1)),
            ..BasisWrite::default()
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, WenlanError::NotFound(_)), "{error:?}");
    assert_eq!(version_of(&db, "b").await, 1, "nothing was saved");
}

#[tokio::test]
async fn r11_the_confirm_rolls_back_with_a_save_that_fails_before_commit() {
    let (db, _dir) = test_db().await;
    page(
        &db,
        "r11-basis",
        "Basis",
        "The launch is on 2026-03-01.",
        "sp",
    )
    .await;
    page(&db, "r11-citer", "Citer", "Plans.", "sp").await;
    db.mark_page_basis("r11-citer", "r11-basis", 1, "human")
        .await
        .unwrap();
    edit(&db, "r11-basis", "The launch is on 2026-04-01.", false).await;
    assert_eq!(
        entry(&db, "r11-citer", "r11-basis").await.state,
        BASIS_STATE_PENDING
    );

    // Everything is staged (row, history, confirm) when the save fails.
    *crate::db::FAIL_BEFORE_COMMIT.lock().unwrap() = Some("r11-citer".to_string());
    let error = write_with(
        &db,
        "r11-citer",
        "Plans, rechecked.",
        &BasisWrite {
            actor: "human".into(),
            confirm: Some(("r11-basis".into(), 2)),
            ..BasisWrite::default()
        },
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("injected failure"),
        "the fault fired: {error:?}"
    );

    assert_eq!(
        entry(&db, "r11-citer", "r11-basis").await.state,
        BASIS_STATE_PENDING,
        "a confirm never outlives the save it rode on"
    );
    assert_eq!(version_of(&db, "r11-citer").await, 1);
}

#[tokio::test]
async fn the_minor_flag_lands_on_the_history_row_of_the_write() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "Teh launch.", "sp").await;
    edit(&db, "a", "The launch.", true).await;
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    let flag = |version: i64| {
        let db = &db;
        async move {
            let conn = db.conn.lock().await;
            let mut rows = conn
                .query(
                    "SELECT minor FROM page_history WHERE page_id = 'a' AND version = ?1",
                    libsql::params![version],
                )
                .await
                .unwrap();
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
        }
    };
    assert_eq!(flag(2).await, 1, "the typo fix is minor");
    assert_eq!(flag(3).await, 0, "the real edit is not");
}

// ---------------------------------------------------------------------------
// Diff details
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_diff_lists_each_edit_since_the_confirmed_version_with_its_summary_and_agent() {
    let (db, _dir) = pair().await;
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    edit(&db, "a", "The launch is on 2026-04-01 .", true).await;
    {
        // The changelog on the note carries the edit summaries; version 3 has none.
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE pages SET changelog = ?1 WHERE id = 'a'",
            libsql::params![
                r#"[{"version":2,"delta_summary":"Moved the launch to April.","agent":"claude-code"},{"version":9,"delta_summary":"unrelated"}]"#
            ],
        )
        .await
        .unwrap();
    }
    let BasisDiffOutcome::Available(diff) = db.basis_diff("b", "a").await.unwrap() else {
        panic!("the diff must be available");
    };
    assert_eq!((diff.from_version, diff.to_version), (1, 3));
    assert_eq!(diff.from_content, "The launch is on 2026-03-01.");
    assert_eq!(diff.to_content, "The launch is on 2026-04-01 .");
    assert_eq!(diff.edits.len(), 2, "one per version in between");
    assert_eq!(diff.edits[0].version, 2);
    assert_eq!(diff.edits[0].delta_summary, "Moved the launch to April.");
    assert!(!diff.edits[0].minor);
    assert_eq!(diff.edits[1].version, 3);
    assert_eq!(diff.edits[1].delta_summary, "", "no summary recorded");
    assert!(diff.edits[1].minor);
    assert_eq!(diff.edits[0].agent.as_deref(), Some("claude-code"));
    assert_eq!(diff.edits[1].agent, None, "no agent recorded");
}

// ---------------------------------------------------------------------------
// Read helpers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_non_ok_basis_returns_only_notes_with_something_to_look_at() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "The launch is on 2026-03-01.", "sp").await;
    page(&db, "y", "Yotta", "Healthy.", "sp").await;
    page(&db, "z", "Zeta", "Another basis.", "sp").await;
    for citer in ["quiet", "waiting", "orphaned"] {
        page(&db, citer, citer, "text", "sp").await;
    }
    db.mark_page_basis("quiet", "y", 1, "human").await.unwrap();
    db.mark_page_basis("waiting", "a", 1, "human")
        .await
        .unwrap();
    db.mark_page_basis("orphaned", "z", 1, "human")
        .await
        .unwrap();
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    db.delete_page("z").await.unwrap();

    let ids: Vec<String> = ["quiet", "waiting", "orphaned", "no-such-note"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let found = db.list_non_ok_basis(&ids).await.unwrap();
    assert!(!found.contains_key("quiet"), "an ok basis is not listed");
    assert!(!found.contains_key("no-such-note"));
    assert_eq!(found["waiting"].len(), 1);
    assert_eq!(found["waiting"][0].state, BASIS_STATE_PENDING);
    assert_eq!(found["orphaned"].len(), 1);
    assert_eq!(found["orphaned"][0].state, BASIS_STATE_DELETED);
    assert_eq!(found["orphaned"][0].title, "Zeta", "the title survives");
    assert!(db.list_non_ok_basis(&[]).await.unwrap().is_empty());
}

#[tokio::test]
async fn page_ids_present_finds_notes_in_any_status_and_across_chunks() {
    let (db, _dir) = test_db().await;
    page(&db, "first", "First", "text", "sp").await;
    page(&db, "last", "Last", "text", "sp").await;
    set_status(&db, "last", "archived").await;

    let mut ids: Vec<String> = vec!["first".into()];
    ids.extend((0..450).map(|n| format!("memory-{n}")));
    ids.push("last".into());
    let mut found = db.page_ids_present(&ids).await.unwrap();
    found.sort();
    assert_eq!(found, vec!["first".to_string(), "last".to_string()]);
    assert!(db.page_ids_present(&[]).await.unwrap().is_empty());
}

#[tokio::test]
async fn check_basis_candidates_applies_the_mark_rules_before_a_note_exists() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "text", "sp").await;
    page(&db, "gone", "Gone", "text", "sp").await;
    page(&db, "ent", "Entity", "text", "sp").await;
    // Only a note someone relied on is remembered as deleted; any other
    // missing id is simply not found.
    page(&db, "reader", "Reader", "text", "sp").await;
    db.mark_page_basis("reader", "gone", 1, "human")
        .await
        .unwrap();
    db.delete_page("gone").await.unwrap();
    set_kind(&db, "ent", "entity").await;

    let marks = |marks: &[(&str, i64)]| {
        marks
            .iter()
            .map(|(id, v)| (id.to_string(), *v))
            .collect::<Vec<_>>()
    };
    db.check_basis_candidates(&marks(&[("a", 1)]))
        .await
        .unwrap();
    db.check_basis_candidates(&[]).await.unwrap();
    assert!(matches!(
        db.check_basis_candidates(&marks(&[("a", 4)]))
            .await
            .unwrap_err(),
        BasisError::InvalidVersion {
            requested: 4,
            current: 1
        }
    ));
    assert!(matches!(
        db.check_basis_candidates(&marks(&[("ghost", 1)]))
            .await
            .unwrap_err(),
        BasisError::BasisNotFound
    ));
    assert!(matches!(
        db.check_basis_candidates(&marks(&[("gone", 1)]))
            .await
            .unwrap_err(),
        BasisError::BasisDeleted
    ));
    assert!(matches!(
        db.check_basis_candidates(&marks(&[("ent", 1)]))
            .await
            .unwrap_err(),
        BasisError::BasisIsEntityPage
    ));
    // One bad mark refuses the lot.
    assert!(db
        .check_basis_candidates(&marks(&[("a", 1), ("ghost", 1)]))
        .await
        .is_err());
}

#[tokio::test]
async fn list_basis_pending_in_scope_follows_the_read_scope() {
    use crate::read_scope::ReadScope;
    let (db, _dir) = test_db().await;
    for (basis, citer, space) in [("a1", "c1", "sp"), ("a2", "c2", "sp2")] {
        page(&db, basis, basis, "The date is 2026-03-01.", space).await;
        page(&db, citer, citer, "text", space).await;
        db.mark_page_basis(citer, basis, 1, "human").await.unwrap();
        edit(&db, basis, "The date is 2026-04-01.", false).await;
    }
    for id in ["u1", "u2"] {
        db.insert_page_with_kind(
            id,
            id,
            None,
            "The date is 2026-03-01.",
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
    db.mark_page_basis("u2", "u1", 1, "human").await.unwrap();
    edit(&db, "u1", "The date is 2026-04-01.", false).await;

    let citers = |groups: Vec<BasisPendingGroup>| -> Vec<String> {
        let mut out: Vec<String> = groups
            .into_iter()
            .flat_map(|g| g.citers.into_iter().map(|c| c.page_id))
            .collect();
        out.sort();
        out
    };
    assert_eq!(
        citers(
            db.list_basis_pending_in_scope(&ReadScope::Global)
                .await
                .unwrap()
        ),
        vec!["c1", "c2", "u2"]
    );
    assert_eq!(
        citers(
            db.list_basis_pending_in_scope(&ReadScope::Space("sp".into()))
                .await
                .unwrap()
        ),
        vec!["c1"]
    );
    assert_eq!(
        citers(
            db.list_basis_pending_in_scope(&ReadScope::Uncategorized)
                .await
                .unwrap()
        ),
        vec!["u2"]
    );
}

/// Resolved wikilinks live in the `edges` table, not `page_links`.
#[tokio::test]
async fn linked_changed_lists_linked_notes_edited_after_the_note_itself() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "text", "sp").await;
    page(&db, "b", "Beta", "See [[Alpha]].", "sp").await;
    db.replace_page_links(
        "b",
        &[crate::synthesis::wikilinks::Wikilink {
            label: "Alpha".into(),
            target_page_id: Some("a".into()),
        }],
    )
    .await
    .unwrap();
    let stamp = |id: &'static str, at: &'static str| {
        let db = &db;
        async move {
            let conn = db.conn.lock().await;
            conn.execute(
                "UPDATE pages SET last_modified = ?2 WHERE id = ?1",
                libsql::params![id, at],
            )
            .await
            .unwrap();
        }
    };

    // The linked note is older than the one that links to it: nothing to report.
    stamp("b", "2026-01-03T00:00:00Z").await;
    stamp("a", "2026-01-02T00:00:00Z").await;
    assert!(db.linked_changed("b").await.unwrap().is_empty());

    // It changed after: reported with its title and current version.
    stamp("a", "2026-01-04T00:00:00Z").await;
    assert_eq!(
        db.linked_changed("b").await.unwrap(),
        vec![("a".to_string(), "Alpha".to_string(), 1)]
    );
    // Nobody links to "b", and the link is one-way.
    assert!(db.linked_changed("a").await.unwrap().is_empty());

    // An archived or entity target is not a note to go and read.
    set_status(&db, "a", "archived").await;
    assert!(db.linked_changed("b").await.unwrap().is_empty());
    set_status(&db, "a", "active").await;
    set_kind(&db, "a", "entity").await;
    assert!(db.linked_changed("b").await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Arc 2 review: the diff read carries the caller's scope (finding 2)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_scoped_diff_reads_a_pair_moved_out_of_the_callers_space_as_out_of_scope() {
    let (db, _dir) = pair().await;
    edit(&db, "a", "The launch is on 2026-04-01.", false).await;
    let in_sp = crate::read_scope::ReadScope::Space("sp".into());
    assert!(matches!(
        db.basis_diff_in_scope("b", "a", &in_sp).await.unwrap(),
        BasisDiffOutcome::Available(_)
    ));

    // Both notes move to another Space after the caller's visibility check
    // passed (the race): the pair is still in one Space, so an unscoped read
    // hands back the prose. The scoped read must not.
    db.set_page_workspace("a", Some("sp2")).await.unwrap();
    db.set_page_workspace("b", Some("sp2")).await.unwrap();
    assert!(matches!(
        db.basis_diff("b", "a").await.unwrap(),
        BasisDiffOutcome::Available(_)
    ));
    assert_eq!(
        db.basis_diff_in_scope("b", "a", &in_sp).await.unwrap(),
        BasisDiffOutcome::OutOfScope("b".into())
    );
    assert!(matches!(
        db.basis_diff_in_scope("b", "a", &crate::read_scope::ReadScope::Space("sp2".into()))
            .await
            .unwrap(),
        BasisDiffOutcome::Available(_)
    ));

    // Only the basis is away (the citer came back): the basis is the one named.
    db.set_page_workspace("b", Some("sp")).await.unwrap();
    assert_eq!(
        db.basis_diff_in_scope("b", "a", &in_sp).await.unwrap(),
        BasisDiffOutcome::OutOfScope("a".into())
    );
    // A note that is not there at all reads the same way.
    assert_eq!(
        db.basis_diff_in_scope("ghost", "a", &in_sp).await.unwrap(),
        BasisDiffOutcome::OutOfScope("ghost".into())
    );
}

// ---------------------------------------------------------------------------
// Arc 2 review: a locked note's agent write keeps its marks (finding 3, the
// in-CAS gate), an unchanged write still marks (finding 4), and a create's
// marks are atomic with it (finding 5)
// ---------------------------------------------------------------------------

/// An agent write of `id` through the canonical dispatcher, as the refresh
/// route sends it once its own checks pass. Going straight to the dispatcher
/// skips the route's pre-gate, so a locked note is gated by the in-CAS gate.
async fn agent_write(
    db: &MemoryDB,
    id: &str,
    content: &str,
    marks: Vec<(String, i64)>,
) -> Result<crate::post_write::WriteResult, WenlanError> {
    let expected_version = Some(version_of(db, id).await);
    let expected_source_revision = db.get_page_source_revision(id).await.unwrap();
    crate::post_write::page_write(
        db,
        crate::post_write::PageWrite::AgentUpdate {
            citations: None,
            page_id: id,
            req: wenlan_types::requests::UpdatePageRequest {
                content: content.to_string(),
                source_memory_ids: Vec::new(),
                expected_version,
                caller_id: None,
                operation_id: None,
            },
            agent: Some("claude-code"),
            expected_source_revision,
            knowledge_path: None,
            basis: BasisWrite {
                actor: "claude-code".into(),
                marks,
                ..BasisWrite::default()
            },
        },
    )
    .await
}

async fn pending_card_ids(db: &MemoryDB) -> Vec<String> {
    db.list_pending_revisions(50)
        .await
        .unwrap()
        .into_iter()
        .map(|card| card.revision_source_id)
        .collect()
}

#[tokio::test]
async fn a_locked_notes_gated_agent_write_records_its_marks_when_the_card_is_accepted() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "The launch is on 2026-03-01.", "sp").await;
    page(&db, "c", "Gamma", "Glossary.", "sp").await;
    page(&db, "b", "Beta", "Original plans.", "sp").await;
    // `b` already relies on `c` at version 1, and `c` has moved on since:
    // the agent's write reconfirms it at version 2.
    db.mark_page_basis("b", "c", 1, "human").await.unwrap();
    edit(&db, "c", "Glossary, revised.", false).await;
    assert_eq!(entry(&db, "b", "c").await.state, BASIS_STATE_PENDING);
    assert!(db.set_page_review_required("b", true).await.unwrap());

    let staged = agent_write(
        &db,
        "b",
        "Plans built on Alpha.",
        vec![("a".into(), 1), ("c".into(), 2)],
    )
    .await
    .unwrap();
    assert_eq!(staged.outcome, crate::post_write::WriteOutcome::Gated);
    let card_id = staged.revision_card_id.expect("the lock stages a card");
    // Nothing lands while the card waits.
    assert_eq!(
        db.get_page("b").await.unwrap().unwrap().content,
        "Original plans."
    );
    assert_eq!(db.list_page_basis("b").await.unwrap().len(), 1);
    assert_eq!(entry(&db, "b", "c").await.state, BASIS_STATE_PENDING);

    crate::post_write::accept_pending_revision(&db, &card_id, "human")
        .await
        .unwrap();
    assert_eq!(
        db.get_page("b").await.unwrap().unwrap().content,
        "Plans built on Alpha."
    );
    let a = entry(&db, "b", "a").await;
    assert_eq!(a.state, BASIS_STATE_OK);
    assert_eq!(a.confirmed_version, 1);
    let c = entry(&db, "b", "c").await;
    assert_eq!(c.state, BASIS_STATE_OK, "the reconfirm landed on accept");
    assert_eq!(c.confirmed_version, 2);
}

#[tokio::test]
async fn editing_a_gated_card_before_accepting_it_keeps_its_marks() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "The launch is on 2026-03-01.", "sp").await;
    page(&db, "c", "Gamma", "Glossary.", "sp").await;
    page(&db, "b", "Beta", "Original plans.", "sp").await;
    db.mark_page_basis("b", "c", 1, "human").await.unwrap();
    edit(&db, "c", "Glossary, revised.", false).await;
    assert!(db.set_page_review_required("b", true).await.unwrap());

    let staged = agent_write(
        &db,
        "b",
        "Plans built on Alpha.",
        vec![("a".into(), 1), ("c".into(), 2)],
    )
    .await
    .unwrap();
    let card_id = staged.revision_card_id.expect("the lock stages a card");

    // The human rewrites the card's text in the review queue, then accepts.
    let edited = "Plans built on Alpha, reworded by a human.";
    crate::post_write::update_memory(
        &db,
        &card_id,
        crate::post_write::MemoryUpdate {
            content: Some(edited),
            expected_content: None,
            space: None,
            confirm: false,
            memory_type: None,
        },
    )
    .await
    .unwrap();
    crate::post_write::accept_pending_revision(&db, &card_id, "human")
        .await
        .unwrap();

    assert_eq!(db.get_page("b").await.unwrap().unwrap().content, edited);
    let a = entry(&db, "b", "a").await;
    assert_eq!(a.state, BASIS_STATE_OK, "the new basis survived the edit");
    assert_eq!(a.confirmed_version, 1);
    let c = entry(&db, "b", "c").await;
    assert_eq!(c.state, BASIS_STATE_OK, "the reconfirm survived the edit");
    assert_eq!(c.confirmed_version, 2);
}

#[tokio::test]
async fn a_locked_notes_gated_agent_write_with_a_refused_mark_stages_nothing() {
    let (db, _dir) = test_db().await;
    page(&db, "b", "Beta", "Original plans.", "sp").await;
    page(&db, "draft", "Draft", "Not finished.", "sp").await;
    set_status(&db, "draft", "draft").await;
    assert!(db.set_page_review_required("b", true).await.unwrap());

    let error = agent_write(&db, "b", "Plans.", vec![("ghost".into(), 1)])
        .await
        .unwrap_err();
    assert!(matches!(error, WenlanError::NotFound(_)), "{error:?}");
    let error = agent_write(&db, "b", "Plans.", vec![("draft".into(), 1)])
        .await
        .unwrap_err();
    assert!(matches!(error, WenlanError::Validation(_)), "{error:?}");
    assert!(pending_card_ids(&db).await.is_empty(), "no card was staged");
}

#[tokio::test]
async fn accepting_a_card_whose_basis_can_no_longer_be_relied_on_discards_it_with_a_typed_reason() {
    // Moving the basis to another Space is not one of these: a note may rely
    // on a note in any Space.
    for spoil in ["archive", "recreate"] {
        let (db, _dir) = test_db().await;
        page(&db, "a", "Alpha", "The launch is on 2026-03-01.", "sp").await;
        page(&db, "b", "Beta", "Original plans.", "sp").await;
        assert!(db.set_page_review_required("b", true).await.unwrap());
        let card_id = agent_write(&db, "b", "Plans built on Alpha.", vec![("a".into(), 1)])
            .await
            .unwrap()
            .revision_card_id
            .unwrap();

        match spoil {
            "archive" => db.archive_page("a").await.unwrap(),
            _ => {
                // Deleted and recreated under the same id: not the note the
                // agent read.
                let conn = db.conn.lock().await;
                conn.execute("UPDATE pages SET incarnation = 'reborn' WHERE id = 'a'", ())
                    .await
                    .unwrap();
            }
        }

        let error = crate::post_write::accept_pending_revision(&db, &card_id, "human")
            .await
            .unwrap_err();
        let WenlanError::Conflict(message) = &error else {
            panic!("{spoil}: expected a conflict, got {error:?}");
        };
        assert!(
            message.starts_with(wenlan_types::PAGE_REVISION_BASIS_REFUSED),
            "{spoil}: {message}"
        );
        assert!(message.contains(&card_id), "{spoil}: {message}");
        assert_eq!(
            db.get_page("b").await.unwrap().unwrap().content,
            "Original plans.",
            "{spoil}: the page is unchanged"
        );
        assert!(
            db.list_page_basis("b").await.unwrap().is_empty(),
            "{spoil}: no mark landed"
        );
        assert!(
            pending_card_ids(&db).await.is_empty(),
            "{spoil}: the card was discarded, not stranded"
        );
    }
}

#[tokio::test]
async fn an_unchanged_agent_write_still_records_and_reconfirms_its_marks() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "The launch is on 2026-03-01.", "sp").await;
    page(&db, "c", "Gamma", "Glossary.", "sp").await;
    page(&db, "b", "Beta", "Plans.", "sp").await;
    db.mark_page_basis("b", "c", 1, "human").await.unwrap();
    edit(&db, "c", "Glossary, revised.", false).await;
    let version = version_of(&db, "b").await;

    let result = agent_write(&db, "b", "Plans.", vec![("a".into(), 1), ("c".into(), 2)])
        .await
        .unwrap();
    assert_eq!(result.outcome, crate::post_write::WriteOutcome::Unchanged);
    assert_eq!(version_of(&db, "b").await, version, "no page write");
    let a = entry(&db, "b", "a").await;
    assert_eq!(a.state, BASIS_STATE_OK);
    assert_eq!(a.added_by, "claude-code");
    let c = entry(&db, "b", "c").await;
    assert_eq!(c.state, BASIS_STATE_OK, "the reconfirm landed");
    assert_eq!(c.confirmed_version, 2);
}

#[tokio::test]
async fn an_unchanged_agent_write_with_a_refused_mark_is_refused_and_records_none() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "text", "sp").await;
    page(&db, "b", "Beta", "Plans.", "sp").await;

    let error = agent_write(
        &db,
        "b",
        "Plans.",
        vec![("a".into(), 1), ("ghost".into(), 1)],
    )
    .await
    .unwrap_err();
    assert!(matches!(error, WenlanError::NotFound(_)), "{error:?}");
    assert!(
        db.list_page_basis("b").await.unwrap().is_empty(),
        "the good mark rolled back with the refused one"
    );
}

/// An unchanged write compares itself against one version of the note. An
/// edit that lands before its marks do means the marks describe text that is
/// gone, so none are recorded.
#[tokio::test]
async fn marks_for_an_unchanged_write_are_refused_when_the_note_changed_since() {
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "text", "sp").await;
    page(&db, "b", "Beta", "Plans.", "sp").await;
    let compared = version_of(&db, "b").await;
    edit(&db, "b", "Plans, revised by hand.", false).await;

    let error = db
        .mark_page_bases("b", compared, &[("a".into(), 1)], &[], "claude-code", None)
        .await
        .unwrap_err();
    assert!(
        matches!(error, BasisError::CiterMovedOn { current } if current == compared + 1),
        "{error:?}"
    );
    assert!(db.list_page_basis("b").await.unwrap().is_empty());
}

fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out
}

async fn create_with(
    db: &MemoryDB,
    pages: &std::path::Path,
    title: &str,
    basis_marks: Vec<(String, i64)>,
) -> Result<crate::post_write::WriteResult, WenlanError> {
    crate::post_write::page_write(
        db,
        crate::post_write::PageWrite::Create {
            page_id: None,
            req: wenlan_types::requests::CreateConceptRequest {
                title: title.to_string(),
                content: format!("{title}, derived from Alpha."),
                summary: None,
                entity_id: None,
                space: wenlan_types::WriteSpaceTarget::Named("sp".into()),
                source_memory_ids: Vec::new(),
                creation_kind: Some("authored".to_string()),
                workspace: Some("sp".into()),
            },
            agent: "claude-code",
            knowledge_path: Some(pages),
            page_min_cluster_size: 1,
            // Never attach to an existing note: these tests are about a create.
            page_match_threshold: 2.0,
            citations_json: None,
            basis_marks,
            basis_space_guard: None,
        },
    )
    .await
}

#[tokio::test]
async fn a_create_whose_mark_is_refused_at_write_time_creates_nothing() {
    let (db, dir) = test_db().await;
    let pages = dir.path().join("pages");
    std::fs::create_dir_all(&pages).unwrap();
    page(&db, "a", "Alpha", "The launch is on 2026-03-01.", "sp").await;

    // A mark that can be relied on lands with the note, in its transaction.
    let created = create_with(&db, &pages, "Derived Plan", vec![("a".into(), 1)])
        .await
        .unwrap();
    assert!(created.wrote);
    assert_eq!(entry(&db, &created.id, "a").await.state, BASIS_STATE_OK);
    assert_eq!(entry(&db, &created.id, "a").await.added_by, "claude-code");

    // The route checked this basis; it was archived before the create landed.
    db.archive_page("a").await.unwrap();
    let files_before = files_under(&pages);
    let error = create_with(&db, &pages, "Refused Plan", vec![("a".into(), 1)])
        .await
        .unwrap_err();
    assert!(matches!(error, WenlanError::Validation(_)), "{error:?}");
    {
        let conn = db.conn.lock().await;
        let mut rows = conn
            .query("SELECT id FROM pages WHERE title = 'Refused Plan'", ())
            .await
            .unwrap();
        assert!(rows.next().await.unwrap().is_none(), "no page row");
    }
    assert_eq!(files_under(&pages), files_before, "no page file");
}

/// A Space-locked caller's mark re-checks both notes' Space inside the mark
/// itself. Moving a note to another Space does not bump its version, so a
/// basis moved out after the route's visibility check would otherwise be
/// marked. A Global caller still marks across Spaces.
#[tokio::test]
async fn a_space_locked_mark_refuses_a_note_moved_out_of_its_space() {
    use crate::read_scope::ReadScope;
    let (db, _dir) = test_db().await;
    page(&db, "a", "Alpha", "The launch is on 2026-03-01.", "sp").await;
    page(&db, "b", "Beta", "Plans built on Alpha.", "sp").await;
    let locked = ReadScope::Space("sp".into());
    let marks = [("a".to_string(), 1)];

    db.set_page_workspace("a", Some("sp2")).await.unwrap();
    assert!(matches!(
        db.mark_page_basis_in("b", "a", 1, "agent", Some(&locked))
            .await,
        Err(BasisError::BasisNotFound)
    ));
    assert!(matches!(
        db.mark_page_bases("b", 1, &marks, &[], "agent", Some(&locked))
            .await,
        Err(BasisError::BasisNotFound)
    ));
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM page_basis WHERE page_id = ?1",
            "b"
        )
        .await,
        0
    );

    db.set_page_workspace("a", Some("sp")).await.unwrap();
    db.set_page_workspace("b", Some("sp2")).await.unwrap();
    assert!(matches!(
        db.mark_page_basis_in("b", "a", 1, "agent", Some(&locked))
            .await,
        Err(BasisError::CiterNotFound)
    ));

    db.mark_page_basis_in("b", "a", 1, "human", None)
        .await
        .unwrap();
    assert_eq!(entry(&db, "b", "a").await.state, BASIS_STATE_OK);
}
