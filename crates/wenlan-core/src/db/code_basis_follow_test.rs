// SPDX-License-Identifier: Apache-2.0
//! A cited file that is gone: a rename is followed, a deletion becomes
//! steep's work, and only a file missing for no reason git gives (or lines
//! that cannot be found at all) is left for a person.

use super::code_basis_test::test_db;
use super::MemoryDB;
use crate::code_basis::{
    prepare_code_basis, recheck_code_basis, CODE_BASIS_STATE_MOVED, CODE_BASIS_STATE_PENDING,
    CODE_BASIS_STATE_UNAVAILABLE, LINES_NOT_FOUND,
};
use crate::code_basis_follow::FILE_DELETED;
use crate::read_scope::ReadScope;
use crate::steep::{claim, commit};
use wenlan_types::steep::{
    STEEP_ACTION_REWRITE, STEEP_ACTION_STILL_ACCURATE, STEEP_PLAN_NEEDS_YOU, STEEP_PLAN_REWRITE,
    STEEP_REASON_CODE_CHANGED,
};
use wenlan_types::{CodeBasisInput, CodeBasisRef};

const T0: &str = "2026-01-01T00:00:00Z";
const BODY: &str = "Jobs are taken by the worker.\n\nThe worker retries three times.";

struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn new() -> Self {
        let repo = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        repo.run(&["init", "-q"]);
        repo
    }

    fn run(&self, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(self.dir.path())
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git is installed");
        assert!(status.success(), "git {args:?}");
    }

    fn root(&self) -> String {
        self.dir.path().to_str().unwrap().to_string()
    }

    fn write(&self, path: &str, text: &str) {
        let full = self.dir.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }

    fn commit_all(&self, message: &str) {
        self.run(&["add", "-A"]);
        self.run(&["commit", "-q", "-m", message]);
    }

    fn input(&self, path: &str, start: u32, end: u32) -> CodeBasisInput {
        CodeBasisInput {
            repo_root: self.root(),
            path: path.to_string(),
            line_start: start,
            line_end: end,
        }
    }
}

/// `n` lines named after `name`, the ones in `changed` edited.
fn file(name: &str, n: u32, changed: &[u32]) -> String {
    (1..=n)
        .map(|i| {
            if changed.contains(&i) {
                format!("{name} line {i} changed\n")
            } else {
                format!("{name} line {i}\n")
            }
        })
        .collect()
}

/// A note written from code alone, relying on lines 10-14 of
/// `src/worker.rs`, committed.
async fn note_on_worker(db: &MemoryDB, repo: &Repo, id: &str) {
    db.insert_page_with_kind(
        id,
        "How a job is handled",
        None,
        BODY,
        None,
        Some("sp"),
        &[],
        T0,
        "authored",
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
    repo.write("src/worker.rs", &file("worker", 30, &[]));
    repo.write("src/other.rs", &file("other", 30, &[]));
    repo.commit_all("first");
    let prepared = prepare_code_basis(&[repo.input("src/worker.rs", 10, 14)])
        .await
        .unwrap();
    db.upsert_page_code_basis(id, &prepared, "agent")
        .await
        .unwrap();
}

async fn only(db: &MemoryDB, id: &str) -> CodeBasisRef {
    let mut all = db.list_page_code_basis(id).await.unwrap();
    assert_eq!(all.len(), 1, "{all:?}");
    all.remove(0)
}

#[tokio::test]
async fn a_committed_rename_is_followed_without_asking_anyone() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    note_on_worker(&db, &repo, "p1").await;
    repo.run(&["mv", "src/worker.rs", "src/consumer.rs"]);
    repo.commit_all("rename");

    recheck_code_basis(&db, None).await.unwrap();

    let row = only(&db, "p1").await;
    assert_eq!(row.path, "src/consumer.rs");
    assert_eq!(row.state, CODE_BASIS_STATE_MOVED);
    assert_eq!((row.line_start, row.line_end), (10, 14));
    assert_eq!(row.reason, None);
}

#[tokio::test]
async fn a_staged_rename_with_changed_lines_follows_and_waits_for_a_judgement() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    note_on_worker(&db, &repo, "p1").await;
    repo.run(&["mv", "src/worker.rs", "src/consumer.rs"]);
    // Small enough an edit that git still calls it a rename.
    repo.write("src/consumer.rs", &file("worker", 30, &[12]));
    repo.run(&["add", "-A"]);

    recheck_code_basis(&db, None).await.unwrap();

    let row = only(&db, "p1").await;
    assert_eq!(row.path, "src/consumer.rs");
    assert_eq!(row.state, CODE_BASIS_STATE_PENDING);
    assert_eq!((row.line_start, row.line_end), (10, 14));
    assert!(row.changed_at.is_some());
}

#[tokio::test]
async fn a_file_moved_without_telling_git_is_found_by_its_content() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    note_on_worker(&db, &repo, "p1").await;
    std::fs::create_dir_all(repo.dir.path().join("src/jobs")).unwrap();
    std::fs::rename(
        repo.dir.path().join("src/worker.rs"),
        repo.dir.path().join("src/jobs/consumer.rs"),
    )
    .unwrap();

    recheck_code_basis(&db, None).await.unwrap();

    let row = only(&db, "p1").await;
    assert_eq!(row.path, "src/jobs/consumer.rs");
    assert_eq!(row.state, CODE_BASIS_STATE_MOVED);
}

#[tokio::test]
async fn a_deleted_file_is_steeps_work_and_the_rewrite_drops_the_reliance() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    note_on_worker(&db, &repo, "p1").await;
    repo.run(&["rm", "-q", "src/worker.rs"]);
    repo.commit_all("drop the worker");

    recheck_code_basis(&db, None).await.unwrap();
    let row = only(&db, "p1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_UNAVAILABLE);
    assert_eq!(row.reason.as_deref(), Some(FILE_DELETED));
    // A second check keeps it deleted, without asking git again.
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "p1").await.reason.as_deref(), Some(FILE_DELETED));

    let listed = crate::steep::list_work(&db, &ReadScope::Global, |_| true, 20, None, true)
        .await
        .unwrap();
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].stale_reason, STEEP_REASON_CODE_CHANGED);

    let claimed = claim(&db, "p1", &ReadScope::Global, None).await.unwrap();
    assert_eq!(claimed.plan, STEEP_PLAN_REWRITE);
    assert_eq!(claimed.code.len(), 1);
    let code = &claimed.code[0];
    assert!(code.deleted);
    assert_eq!(code.path, "src/worker.rs");
    assert!(code.excerpt.contains("worker line 12"), "{}", code.excerpt);
    assert!(claimed.note.contains("deleted"), "{}", claimed.note);

    let done = commit(
        &db,
        claimed.token.as_deref().unwrap(),
        STEEP_ACTION_REWRITE,
        Some("Jobs are taken from the queue directly."),
        &[],
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "written");
    assert!(db.list_page_code_basis("p1").await.unwrap().is_empty());
    assert!(
        crate::steep::list_work(&db, &ReadScope::Global, |_| true, 20, None, true)
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn a_page_still_right_without_the_deleted_file_just_stops_relying_on_it() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    note_on_worker(&db, &repo, "p1").await;
    // Deleted in the working tree, not committed yet.
    std::fs::remove_file(repo.dir.path().join("src/worker.rs")).unwrap();

    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "p1").await.reason.as_deref(), Some(FILE_DELETED));

    let claimed = claim(&db, "p1", &ReadScope::Global, None).await.unwrap();
    let done = commit(
        &db,
        claimed.token.as_deref().unwrap(),
        STEEP_ACTION_STILL_ACCURATE,
        None,
        &[],
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "still_accurate");
    assert!(db.list_page_code_basis("p1").await.unwrap().is_empty());
    assert_eq!(db.get_page("p1").await.unwrap().unwrap().content, BODY);
}

#[tokio::test]
async fn a_file_missing_for_no_reason_git_gives_is_left_for_a_person() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    db.insert_page_with_kind(
        "p1",
        "Notes",
        None,
        BODY,
        None,
        Some("sp"),
        &[],
        T0,
        "authored",
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
    // Never committed: git has no record of it at all.
    repo.write("scratch.rs", &file("scratch", 20, &[]));
    let prepared = prepare_code_basis(&[repo.input("scratch.rs", 2, 4)])
        .await
        .unwrap();
    db.upsert_page_code_basis("p1", &prepared, "agent")
        .await
        .unwrap();
    std::fs::remove_file(repo.dir.path().join("scratch.rs")).unwrap();

    recheck_code_basis(&db, None).await.unwrap();

    let row = only(&db, "p1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_UNAVAILABLE);
    assert_eq!(row.reason.as_deref(), Some("file_missing"));
    assert!(
        crate::steep::list_work(&db, &ReadScope::Global, |_| true, 20, None, true)
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn lines_that_cannot_be_found_are_marked_for_a_person_not_dropped() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    note_on_worker(&db, &repo, "p1").await;
    // Every line around the cited span rewritten: nothing can fence it.
    repo.write("src/worker.rs", &file("rewritten", 30, &[]));
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "p1").await.state, CODE_BASIS_STATE_PENDING);

    // The caller's small model is asked to judge; there is nothing to show
    // it, so the claim says the page is the user's, and the row says why.
    let claimed = claim(&db, "p1", &ReadScope::Global, Some("judge"))
        .await
        .unwrap();
    assert_eq!(claimed.plan, STEEP_PLAN_NEEDS_YOU);
    assert!(claimed.note.contains("src/worker.rs"), "{}", claimed.note);
    let row = only(&db, "p1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_PENDING);
    assert_eq!(row.reason.as_deref(), Some(LINES_NOT_FOUND));
}
