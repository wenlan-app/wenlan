// SPDX-License-Identifier: Apache-2.0
//! Steep on demand for notes whose cited code changed: the caller's small
//! model judges the change (`code_check`), and a note judged `stale` is
//! rewritten from the code as it reads now (`code_changed`).
//!
//! They run on the no-embedder scratch database the code basis tests use. A
//! revision card is a memory row and needs the embedder, so the card case is
//! in `crate::steep::steep_test`, on these fixtures.

use super::code_basis_test::test_db;
use super::MemoryDB;
use crate::code_basis::{
    prepare_code_basis, recheck_code_basis, CODE_BASIS_STATE_OK, CODE_BASIS_STATE_PENDING,
};
use crate::read_scope::ReadScope;
use crate::steep::{claim, commit, list_work, SteepError};
use wenlan_types::steep::{
    SteepCodeAnswer, STEEP_ACTION_JUDGE, STEEP_ACTION_REWRITE, STEEP_ACTION_STILL_ACCURATE,
    STEEP_PLAN_JUDGE, STEEP_PLAN_REWRITE, STEEP_REASON_CODE_CHANGED, STEEP_REASON_CODE_CHECK,
};
use wenlan_types::{CodeBasisInput, CodeBasisRef};

const T0: &str = "2026-01-01T00:00:00Z";
const SPACE: &str = "sp";
const JUDGE: Option<&str> = Some("judge the change");
pub(crate) const BODY: &str =
    "Keys are cut to 32 bits on purpose.\n\nThe cut happens in truncate().";
pub(crate) const FIXED: &str =
    "Keys are cut to 48 bits on purpose.\n\nThe cut happens in truncate().";

pub(crate) struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    pub(crate) fn new() -> Self {
        let repo = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.dir.path())
            .args(["init", "-q"])
            .status()
            .expect("git is installed");
        assert!(status.success());
        repo
    }

    pub(crate) fn write(&self, path: &str, text: &str) {
        std::fs::write(self.dir.path().join(path), text).unwrap();
    }

    fn input(&self, path: &str, start: u32, end: u32) -> CodeBasisInput {
        CodeBasisInput {
            repo_root: self.dir.path().to_str().unwrap().to_string(),
            path: path.to_string(),
            line_start: start,
            line_end: end,
        }
    }
}

/// `n` lines, the ones in `changed` replaced by `<line> changed`.
pub(crate) fn file(n: u32, changed: &[u32]) -> String {
    (1..=n)
        .map(|i| {
            if changed.contains(&i) {
                format!("line {i} changed\n")
            } else {
                format!("line {i}\n")
            }
        })
        .collect()
}

/// A note relying on lines 10-14 of a 30-line file whose line 12 then changed
/// and was checked: its row is `pending` and no model has judged it.
/// `creation_kind` `distilled` is Wenlan's own note from sources, `authored`
/// one written with none (what `/wenlan wiki` writes).
pub(crate) async fn changed_note(db: &MemoryDB, repo: &Repo, id: &str, creation_kind: &str) {
    changed_note_on(db, repo, id, creation_kind, &[]).await;
}

async fn changed_note_on(
    db: &MemoryDB,
    repo: &Repo,
    id: &str,
    creation_kind: &str,
    sources: &[&str],
) {
    db.insert_page_with_kind(
        id,
        &format!("Note {id}"),
        None,
        BODY,
        None,
        Some(SPACE),
        sources,
        T0,
        creation_kind,
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
    repo.write("a.rs", &file(30, &[]));
    let prepared = prepare_code_basis(&[repo.input("a.rs", 10, 14)])
        .await
        .unwrap();
    db.upsert_page_code_basis(id, &prepared, "agent")
        .await
        .unwrap();
    repo.write("a.rs", &file(30, &[12]));
    recheck_code_basis(db, None).await.unwrap();
    assert_eq!(only(db, id).await.state, CODE_BASIS_STATE_PENDING);
}

pub(crate) async fn only(db: &MemoryDB, id: &str) -> CodeBasisRef {
    let mut all = db.list_page_code_basis(id).await.unwrap();
    assert_eq!(all.len(), 1, "{all:?}");
    all.remove(0)
}

async fn body_of(db: &MemoryDB, id: &str) -> String {
    db.get_page(id).await.unwrap().unwrap().content
}

pub(crate) async fn reasons(db: &MemoryDB, judge_code: bool) -> Vec<(String, String)> {
    list_work(db, &ReadScope::Global, |_| true, 20, None, judge_code)
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|item| (item.page_id, item.stale_reason))
        .collect()
}

pub(crate) fn verdict(kind: &str, explanation: &str) -> String {
    serde_json::json!({"verdict": kind, "explanation": explanation}).to_string()
}

/// Claim the note's `code_check` and answer its one question with `answer`.
pub(crate) async fn judge_with(db: &MemoryDB, id: &str, answer: &str) -> String {
    let claimed = claim(db, id, &ReadScope::Global, JUDGE).await.unwrap();
    assert_eq!(claimed.plan, STEEP_PLAN_JUDGE);
    commit(
        db,
        claimed.token.as_deref().unwrap(),
        STEEP_ACTION_JUDGE,
        None,
        &[SteepCodeAnswer {
            n: 1,
            answer: answer.to_string(),
        }],
        None,
        None,
    )
    .await
    .unwrap()
    .judged
    .unwrap()
}

#[tokio::test]
async fn changed_code_is_judged_then_the_note_is_rewritten_and_settled() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    changed_note(&db, &repo, "page_c", "distilled").await;

    // Unjudged changed code is only the caller's when no background model
    // judges it.
    assert_eq!(
        reasons(&db, true).await,
        vec![("page_c".to_string(), STEEP_REASON_CODE_CHECK.to_string())]
    );
    assert!(reasons(&db, false).await.is_empty());
    assert!(matches!(
        claim(&db, "page_c", &ReadScope::Global, None).await,
        Err(SteepError::NotWaiting(_))
    ));

    let claimed = claim(&db, "page_c", &ReadScope::Global, JUDGE)
        .await
        .unwrap();
    assert_eq!(claimed.judge_instructions.as_deref(), JUDGE);
    assert_eq!(claimed.code_checks.len(), 1);
    assert!(claimed.code_checks[0].prompt.contains("line 12 changed"));
    assert!(claimed.code_checks[0].prompt.contains("32 bits"));
    // A judge claim is never rewritten.
    let refused = commit(
        &db,
        claimed.token.as_deref().unwrap(),
        STEEP_ACTION_REWRITE,
        Some(FIXED),
        &[],
        None,
        None,
    )
    .await;
    assert!(
        matches!(refused, Err(SteepError::Invalid(_))),
        "{refused:?}"
    );
    let judged = commit(
        &db,
        claimed.token.as_deref().unwrap(),
        STEEP_ACTION_JUDGE,
        None,
        &[SteepCodeAnswer {
            n: 1,
            answer: verdict("stale", "The note should say 48 bits."),
        }],
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(judged.outcome, "judged");
    assert_eq!(judged.judged.as_deref(), Some("1 stale"));
    assert_eq!(body_of(&db, "page_c").await, BODY);

    // Judged stale: the note waits for a rewrite whichever model judges.
    for judge_code in [true, false] {
        assert_eq!(
            reasons(&db, judge_code).await,
            vec![("page_c".to_string(), STEEP_REASON_CODE_CHANGED.to_string())]
        );
    }
    let claimed = claim(&db, "page_c", &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(claimed.plan, STEEP_PLAN_REWRITE);
    assert_eq!(claimed.code.len(), 1);
    assert!(claimed.code[0].excerpt.contains("line 12 changed"));
    assert_eq!(claimed.code[0].what_changed, "The note should say 48 bits.");

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
    assert_eq!(done.outcome, "written");
    assert_eq!(body_of(&db, "page_c").await, FIXED);
    assert_eq!(only(&db, "page_c").await.state, CODE_BASIS_STATE_OK);
    assert!(reasons(&db, true).await.is_empty());
}

#[tokio::test]
async fn a_no_impact_answer_settles_the_code_without_touching_the_note() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    changed_note(&db, &repo, "page_c", "distilled").await;

    let judged = judge_with(&db, "page_c", &verdict("no_impact", "")).await;
    assert_eq!(judged, "1 no impact");
    assert_eq!(only(&db, "page_c").await.state, CODE_BASIS_STATE_OK);
    assert_eq!(body_of(&db, "page_c").await, BODY);
    assert!(reasons(&db, true).await.is_empty());
}

#[tokio::test]
async fn an_answer_that_is_no_verdict_counts_an_attempt_and_asks_again() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    changed_note(&db, &repo, "page_c", "distilled").await;

    let judged = judge_with(&db, "page_c", "I think it is probably fine?").await;
    assert_eq!(judged, "1 no usable answer");
    assert_eq!(only(&db, "page_c").await.state, CODE_BASIS_STATE_PENDING);
    assert_eq!(
        reasons(&db, true).await,
        vec![("page_c".to_string(), STEEP_REASON_CODE_CHECK.to_string())]
    );
}

#[tokio::test]
async fn code_that_changed_again_after_the_claim_refuses_the_rewrite() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    changed_note(&db, &repo, "page_c", "distilled").await;
    judge_with(
        &db,
        "page_c",
        &verdict("stale", "The note should say 48 bits."),
    )
    .await;
    let claimed = claim(&db, "page_c", &ReadScope::Global, None)
        .await
        .unwrap();

    repo.write("a.rs", &file(30, &[12, 13]));
    recheck_code_basis(&db, None).await.unwrap();

    let refused = commit(
        &db,
        claimed.token.as_deref().unwrap(),
        STEEP_ACTION_REWRITE,
        Some(FIXED),
        &[],
        None,
        None,
    )
    .await;
    assert!(
        matches!(refused, Err(SteepError::Changed(_))),
        "{refused:?}"
    );
    assert_eq!(body_of(&db, "page_c").await, BODY);

    // The verdict was about other lines: they are judged again, not
    // rewritten from what the old verdict said.
    let again = claim(&db, "page_c", &ReadScope::Global, JUDGE)
        .await
        .unwrap();
    assert_eq!(again.plan, STEEP_PLAN_JUDGE);
    assert!(again.code_checks[0].prompt.contains("line 13 changed"));
}

#[tokio::test]
async fn a_note_an_agent_wrote_from_code_alone_is_rewritten_in_place() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    changed_note(&db, &repo, "page_m", "authored").await;
    judge_with(
        &db,
        "page_m",
        &verdict("stale", "The note should say 48 bits."),
    )
    .await;

    let claimed = claim(&db, "page_m", &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(claimed.plan, STEEP_PLAN_REWRITE);
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
    assert_eq!(done.outcome, "written");
    assert_eq!(body_of(&db, "page_m").await, FIXED);
    assert_eq!(only(&db, "page_m").await.state, CODE_BASIS_STATE_OK);
}

#[tokio::test]
async fn a_rewrite_for_code_on_a_note_with_sources_need_not_cite() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    let now = chrono::Utc::now().timestamp();
    {
        let conn = db.test_primary_session().await;
        for id in ["mem_a", "mem_b", "mem_c"] {
            conn.execute(
                "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, \
                 memory_type, space, source_agent, created_at, last_modified, confirmed, \
                 stability, source) VALUES (?1, ?1, ?1, 'Keys are cut in truncate().', 0, \
                 'text', 'fact', ?2, 'claude-code', ?3, ?3, 1, 'confirmed', 'memory')",
                libsql::params![id, SPACE, now],
            )
            .await
            .unwrap();
        }
    }
    changed_note_on(
        &db,
        &repo,
        "page_s",
        "distilled",
        &["mem_a", "mem_b", "mem_c"],
    )
    .await;
    judge_with(
        &db,
        "page_s",
        &verdict("stale", "The note should say 48 bits."),
    )
    .await;

    let claimed = claim(&db, "page_s", &ReadScope::Global, None)
        .await
        .unwrap();
    assert_eq!(claimed.plan, STEEP_PLAN_REWRITE);
    assert_eq!(claimed.sources.len(), 3);
    let done = commit(
        &db,
        claimed.token.as_deref().unwrap(),
        STEEP_ACTION_REWRITE,
        Some(FIXED),
        &[],
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(done.outcome, "written");
    assert_eq!(body_of(&db, "page_s").await, FIXED);
}

#[tokio::test]
async fn a_note_the_code_still_agrees_with_is_settled_by_still_accurate() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    changed_note(&db, &repo, "page_c", "distilled").await;
    judge_with(
        &db,
        "page_c",
        &verdict("stale", "The note should say 48 bits."),
    )
    .await;
    let claimed = claim(&db, "page_c", &ReadScope::Global, None)
        .await
        .unwrap();

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
    assert_eq!(only(&db, "page_c").await.state, CODE_BASIS_STATE_OK);
    assert_eq!(body_of(&db, "page_c").await, BODY);
}
