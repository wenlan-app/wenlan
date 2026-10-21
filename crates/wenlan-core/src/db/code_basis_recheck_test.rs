// SPDX-License-Identifier: Apache-2.0
//! Tests for the background model recheck of changed code basis rows.
//!
//! They run on the no-embedder scratch database the code basis tests use, with
//! a scripted `LlmProvider` standing in for the everyday model. A "tracked"
//! file is also stored as a git blob in a scratch repository, so the lines as
//! cited can be read back the way the daemon reads them.

use super::code_basis_test::test_db;
use super::MemoryDB;
use crate::code_anchor::LineSpan;
use crate::code_basis::{
    prepare_code_basis, recheck_code_basis, CODE_BASIS_STATE_OK, CODE_BASIS_STATE_PENDING,
};
use crate::code_basis_recheck::{
    is_git_blob_id, parse_judgement, read_git_blob, render_excerpt, run_code_basis_recheck_slice,
    RecheckSliceReport, Verdict, MAX_EXPLANATION_CHARS, MAX_LINE_CHARS, RECHECK_CONFIRMED_BY,
    RECHECK_ROWS_PER_SLICE, UNRESOLVED_DIGEST,
};
use crate::llm_provider::{LlmBackend, LlmError, LlmProvider, LlmRequest};
use crate::prompts::PromptRegistry;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use wenlan_types::{CodeBasisInput, CodeBasisRef};

const T0: &str = "2026-01-01T00:00:00Z";
const SPACE: &str = "sp";

type Hook = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Answers from a script, one per call, and records what it was asked.
struct ScriptedProvider {
    replies: Mutex<VecDeque<Result<String, LlmError>>>,
    seen: Mutex<Vec<LlmRequest>>,
    /// Runs inside `generate`, before the reply: a change that lands while the
    /// model is thinking.
    during: Mutex<Option<Hook>>,
}

impl ScriptedProvider {
    fn new(replies: Vec<Result<String, LlmError>>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            seen: Mutex::new(Vec::new()),
            during: Mutex::new(None),
        })
    }

    fn saying(replies: &[&str]) -> Arc<Self> {
        Self::new(replies.iter().map(|r| Ok((*r).to_string())).collect())
    }

    fn during(self: &Arc<Self>, hook: Hook) {
        *self.during.lock().unwrap() = Some(hook);
    }

    fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    fn user_prompt(&self, call: usize) -> String {
        self.seen.lock().unwrap()[call].user_prompt.clone()
    }

    fn as_dyn(self: &Arc<Self>) -> Arc<dyn LlmProvider> {
        self.clone()
    }
}

#[async_trait::async_trait]
impl LlmProvider for ScriptedProvider {
    async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
        self.seen.lock().unwrap().push(request);
        let hook = self.during.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook().await;
        }
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(LlmError::NotAvailable))
    }
    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "scripted"
    }
    fn backend(&self) -> LlmBackend {
        LlmBackend::Api
    }
}

fn verdict(kind: &str, explanation: &str) -> String {
    serde_json::json!({"verdict": kind, "explanation": explanation}).to_string()
}

/// A scratch repository. `track` writes a file and stores its bytes as a git
/// blob (no commit needed), like a file that was committed when it was cited.
struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn git() -> Self {
        let repo = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        repo.run(&["init", "-q"]);
        repo
    }

    /// A folder that is not a git repository: nothing can be read as cited.
    fn plain() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn run(&self, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(self.dir.path())
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

    fn track(&self, path: &str, text: &str) {
        self.write(path, text);
        self.run(&["hash-object", "-w", path]);
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

/// `n` lines, the ones in `changed` replaced by `<line> changed`.
fn file(n: u32, changed: &[u32]) -> String {
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

const BODY: &str = "Keys are cut to 32 bits on purpose.\n\nThe cut happens in truncate().";

async fn note(db: &MemoryDB, id: &str, body: &str) {
    db.insert_page_with_kind(
        id,
        &format!("Note {id}"),
        None,
        body,
        None,
        Some(SPACE),
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

async fn rely(db: &MemoryDB, id: &str, input: CodeBasisInput) {
    let prepared = prepare_code_basis(&[input]).await.unwrap();
    db.upsert_page_code_basis(id, &prepared, "agent")
        .await
        .unwrap();
}

async fn rows(db: &MemoryDB, id: &str) -> Vec<CodeBasisRef> {
    db.list_page_code_basis(id).await.unwrap()
}

async fn only(db: &MemoryDB, id: &str) -> CodeBasisRef {
    let mut all = rows(db, id).await;
    assert_eq!(all.len(), 1, "{all:?}");
    all.remove(0)
}

async fn body_of(db: &MemoryDB, id: &str) -> String {
    db.get_page(id).await.unwrap().unwrap().content
}

/// What the four recheck columns of the note's only row hold.
#[derive(Debug, PartialEq, Eq)]
struct Columns {
    note: Option<String>,
    verdict: Option<String>,
    digest: Option<String>,
    attempts: i64,
}

/// The four recheck columns are empty: the model has not judged the row.
const CLEAR: Columns = Columns {
    note: None,
    verdict: None,
    digest: None,
    attempts: 0,
};

async fn recheck_columns(db: &MemoryDB, id: &str) -> Columns {
    let conn = db.conn.lock().await;
    let mut found = conn
        .query(
            "SELECT recheck_note, recheck_verdict, recheck_digest, recheck_attempts \
             FROM page_code_basis WHERE page_id = ?1",
            libsql::params![id],
        )
        .await
        .unwrap();
    let row = found.next().await.unwrap().unwrap();
    Columns {
        note: row.get::<Option<String>>(0).unwrap(),
        verdict: row.get::<Option<String>>(1).unwrap(),
        digest: row.get::<Option<String>>(2).unwrap(),
        attempts: row.get::<i64>(3).unwrap(),
    }
}

async fn confirmed_by(db: &MemoryDB, id: &str) -> String {
    let conn = db.conn.lock().await;
    let mut found = conn
        .query(
            "SELECT confirmed_by FROM page_code_basis WHERE page_id = ?1",
            libsql::params![id],
        )
        .await
        .unwrap();
    found
        .next()
        .await
        .unwrap()
        .unwrap()
        .get::<String>(0)
        .unwrap()
}

/// A note relying on lines 10-14 of a tracked 30-line file, whose line 12 then
/// changed on disk and was checked: its row is `pending`.
async fn pending_note(db: &MemoryDB, repo: &Repo, id: &str, body: &str) {
    note(db, id, body).await;
    repo.track("a.rs", &file(30, &[]));
    rely(db, id, repo.input("a.rs", 10, 14)).await;
    repo.write("a.rs", &file(30, &[12]));
    recheck_code_basis(db, None).await.unwrap();
    assert_eq!(only(db, id).await.state, CODE_BASIS_STATE_PENDING);
}

async fn slice(
    db: &MemoryDB,
    provider: &Arc<ScriptedProvider>,
    limit: usize,
) -> RecheckSliceReport {
    run_code_basis_recheck_slice(db, &provider.as_dyn(), &PromptRegistry::default(), limit)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// Reading the model's answer
// ---------------------------------------------------------------------------

#[test]
fn each_verdict_parses_with_its_sentence() {
    let no_impact = parse_judgement(&verdict("no_impact", "A comment moved.")).unwrap();
    assert_eq!(no_impact.verdict, Verdict::NoImpact);
    assert_eq!(no_impact.explanation, "A comment moved.");

    let stale = parse_judgement(&verdict("stale", "The note should say 48 bits.")).unwrap();
    assert_eq!(stale.verdict, Verdict::Stale);
    assert_eq!(stale.explanation, "The note should say 48 bits.");

    let human = parse_judgement(&verdict("needs_human", "The note says 32 on purpose.")).unwrap();
    assert_eq!(human.verdict, Verdict::NeedsHuman);

    // `no_impact` needs no sentence.
    assert_eq!(
        parse_judgement(r#"{"verdict":"no_impact"}"#)
            .unwrap()
            .verdict,
        Verdict::NoImpact
    );
}

#[test]
fn a_fenced_or_chatty_reply_is_still_read_and_thinking_is_dropped() {
    let raw = format!(
        "<think>hmm</think>Here you go:\n```json\n{}\n```",
        verdict("no_impact", "Rename only.")
    );
    assert_eq!(parse_judgement(&raw).unwrap().verdict, Verdict::NoImpact);
}

#[test]
fn anything_else_is_no_verdict() {
    for raw in [
        "",
        "I think the note is fine.",
        "{not json}",
        r#"{"verdict":"maybe","explanation":"x"}"#,
        r#"{"explanation":"no verdict key"}"#,
        // stale and needs_human must say why.
        r#"{"verdict":"needs_human"}"#,
        r#"{"verdict":"needs_human","explanation":"   "}"#,
        r#"{"verdict":"stale"}"#,
        r#"{"verdict":"stale","explanation":""}"#,
        // There is no rewrite verdict any more.
        r#"{"verdict":"update","explanation":"x","content":"New body."}"#,
    ] {
        assert_eq!(parse_judgement(raw), None, "{raw:?}");
    }
}

#[test]
fn the_sentence_is_one_trimmed_line() {
    let raw = verdict("needs_human", "  First line\n\nsecond   line.  ");
    assert_eq!(
        parse_judgement(&raw).unwrap().explanation,
        "First line second line."
    );
    let long = "x".repeat(1000);
    assert_eq!(
        parse_judgement(&verdict("needs_human", &long))
            .unwrap()
            .explanation
            .chars()
            .count(),
        MAX_EXPLANATION_CHARS
    );
}

// ---------------------------------------------------------------------------
// What the model is shown
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_prompt_carries_the_note_the_lines_as_cited_and_the_lines_now() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    pending_note(&db, &repo, "n1", BODY).await;
    let provider = ScriptedProvider::saying(&[&verdict("needs_human", "Please look.")]);
    slice(&db, &provider, 3).await;

    let prompt = provider.user_prompt(0);
    assert!(prompt.contains("NOTE TITLE: Note n1"), "{prompt}");
    assert!(prompt.contains(BODY), "{prompt}");
    assert!(
        prompt.contains("CITED LOCATION: a.rs lines 10-14"),
        "{prompt}"
    );
    // As cited: line 12 unchanged. Now: line 12 changed, flagged by `>`, with
    // three lines of context on each side and nothing beyond.
    let cited = prompt
        .split("<<<CITED")
        .nth(1)
        .and_then(|rest| rest.split("CITED>>>").next())
        .unwrap();
    assert!(cited.contains("12 | line 12\n"), "{cited}");
    assert!(!cited.contains("changed"), "{cited}");
    let now = prompt
        .split("<<<NOW")
        .nth(1)
        .and_then(|rest| rest.split("NOW>>>").next())
        .unwrap();
    assert!(now.contains(">12 | line 12 changed\n"), "{now}");
    assert!(now.contains(" 7 | line 7\n"), "{now}");
    assert!(now.contains("17 | line 17\n"), "{now}");
    assert!(
        !now.contains("line 6\n") && !now.contains("line 18\n"),
        "{now}"
    );
    assert!(prompt.contains("Choose no_impact, stale or needs_human."));
    assert!(!prompt.contains("update"), "{prompt}");

    let seen = provider.seen.lock().unwrap();
    assert_eq!(
        seen[0].system_prompt.as_deref(),
        Some(PromptRegistry::default().code_basis_recheck.as_str())
    );
}

#[tokio::test]
async fn a_cited_blob_that_is_not_in_git_says_so_and_the_model_still_judges() {
    let (db, _dir) = test_db().await;
    // Not a git repository: the blob id is recorded, but git cannot produce it.
    let repo = Repo::plain();
    note(&db, "n1", BODY).await;
    repo.write("a.rs", &file(30, &[]));
    rely(&db, "n1", repo.input("a.rs", 10, 14)).await;
    repo.write("a.rs", &file(30, &[12]));
    recheck_code_basis(&db, None).await.unwrap();

    let provider = ScriptedProvider::saying(&[&verdict("needs_human", "Cannot compare.")]);
    slice(&db, &provider, 3).await;
    let prompt = provider.user_prompt(0);
    assert!(prompt.contains("LINES AS CITED: not available"), "{prompt}");
    assert!(!prompt.contains("<<<CITED"), "{prompt}");
    assert!(prompt.contains(">12 | line 12 changed\n"), "{prompt}");
}

#[tokio::test]
async fn stale_is_allowed_without_the_lines_as_cited() {
    let (db, _dir) = test_db().await;
    let repo = Repo::plain();
    note(&db, "n1", BODY).await;
    repo.write("a.rs", &file(30, &[]));
    rely(&db, "n1", repo.input("a.rs", 10, 14)).await;
    repo.write("a.rs", &file(30, &[12]));
    recheck_code_basis(&db, None).await.unwrap();

    let provider = ScriptedProvider::saying(&[&verdict("stale", "The note should say 48 bits.")]);
    let report = slice(&db, &provider, 3).await;
    assert_eq!(report.stale, 1);
    assert_eq!(report.needs_human, 0);
    assert_eq!(body_of(&db, "n1").await, BODY, "the note is untouched");
    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_PENDING);
    assert_eq!(row.ai_verdict.as_deref(), Some("stale"));
    assert_eq!(
        row.explanation.as_deref(),
        Some("The note should say 48 bits.")
    );
}

// ---------------------------------------------------------------------------
// The three verdicts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_impact_confirms_the_row_without_touching_the_note() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    pending_note(&db, &repo, "n1", BODY).await;
    let before = db.get_page("n1").await.unwrap().unwrap().version;
    let provider = ScriptedProvider::saying(&[&verdict("no_impact", "Only a comment changed.")]);

    let report = slice(&db, &provider, 3).await;
    assert_eq!(report.no_impact, 1);
    assert_eq!(report.llm_calls, 1);

    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_OK);
    assert_eq!(row.explanation, None);
    assert_eq!(confirmed_by(&db, "n1").await, RECHECK_CONFIRMED_BY);
    assert_eq!(recheck_columns(&db, "n1").await, CLEAR);
    let page = db.get_page("n1").await.unwrap().unwrap();
    assert_eq!(page.content, BODY);
    assert_eq!(page.version, before, "no new version");
    // The row anchors the lines as they read now: the next check leaves it ok.
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_OK);
}

#[tokio::test]
async fn stale_keeps_the_row_pending_with_the_sentence_and_never_writes_the_page() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    pending_note(&db, &repo, "n1", BODY).await;
    let before = db.get_page("n1").await.unwrap().unwrap();
    let history_before = db.list_page_history("n1", 10).await.unwrap().len();
    let sentence = "The note should say 48 bits, not 32.";
    let provider = ScriptedProvider::saying(&[&verdict("stale", sentence)]);

    let report = slice(&db, &provider, 3).await;
    assert_eq!(report.stale, 1, "{report:?}");
    assert_eq!(report.needs_human, 0);
    assert_eq!(report.no_impact, 0);
    assert!(report.progressed());

    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_PENDING);
    assert!(row.changed_at.is_some());
    assert_eq!(row.explanation.as_deref(), Some(sentence));
    assert_eq!(row.ai_verdict.as_deref(), Some("stale"));
    let columns = recheck_columns(&db, "n1").await;
    assert_eq!(columns.note.as_deref(), Some(sentence));
    assert_eq!(columns.verdict.as_deref(), Some("stale"));
    assert!(columns.digest.is_some());
    assert_eq!(columns.attempts, 0);

    // Nothing wrote the page: same text, same version, same history, no card.
    let after = db.get_page("n1").await.unwrap().unwrap();
    assert_eq!(after.content, before.content);
    assert_eq!(after.content, BODY);
    assert_eq!(after.version, before.version);
    assert_eq!(
        db.list_page_history("n1", 10).await.unwrap().len(),
        history_before
    );
    assert!(db.list_pending_revisions(10).await.unwrap().is_empty());

    // The same change is not asked about again.
    let again = ScriptedProvider::saying(&[&verdict("no_impact", "x")]);
    let report = slice(&db, &again, 3).await;
    assert_eq!(again.calls(), 0);
    assert_eq!(report.skipped, 1);
}

#[tokio::test]
async fn a_stale_row_is_listed_for_the_steep_pass_and_nothing_else_is() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    repo.track("a.rs", &file(30, &[]));
    for id in ["stale-one", "human-one", "unjudged"] {
        note(&db, id, BODY).await;
        rely(&db, id, repo.input("a.rs", 10, 14)).await;
    }
    // Another Space, to see the filter.
    db.insert_page_with_kind(
        "elsewhere",
        "Note elsewhere",
        None,
        BODY,
        None,
        Some("other"),
        &[],
        T0,
        "authored",
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
    rely(&db, "elsewhere", repo.input("a.rs", 10, 14)).await;
    repo.write("a.rs", &file(30, &[12]));
    recheck_code_basis(&db, None).await.unwrap();
    assert!(db.list_code_basis_stale(None).await.unwrap().is_empty());

    // Judge three of the four: stale, needs_human and stale in another Space.
    // The slice goes oldest change first, then by note id.
    {
        let conn = db.conn.lock().await;
        for (id, at) in [
            ("stale-one", 10),
            ("human-one", 20),
            ("elsewhere", 30),
            ("unjudged", 40),
        ] {
            conn.execute(
                "UPDATE page_code_basis SET changed_at = ?2 WHERE page_id = ?1",
                libsql::params![id, at],
            )
            .await
            .unwrap();
        }
    }
    let provider = ScriptedProvider::saying(&[
        &verdict("stale", "The note should say 48."),
        &verdict("needs_human", "The reason is not in the code."),
        &verdict("stale", "The name is different now."),
    ]);
    let report = slice(&db, &provider, 3).await;
    assert_eq!((report.stale, report.needs_human), (2, 1), "{report:?}");

    let all = db.list_code_basis_stale(None).await.unwrap();
    let ids: Vec<&str> = all.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["elsewhere", "stale-one"]);
    let (_, title, found) = all.iter().find(|(id, _, _)| id == "stale-one").unwrap();
    assert_eq!(title, "Note stale-one");
    assert_eq!(found.state, CODE_BASIS_STATE_PENDING);
    assert_eq!(found.ai_verdict.as_deref(), Some("stale"));
    assert_eq!(
        found.explanation.as_deref(),
        Some("The note should say 48.")
    );
    assert_eq!((found.line_start, found.line_end), (10, 14));

    // Filtered by Space.
    let sp = db.list_code_basis_stale(Some(SPACE)).await.unwrap();
    assert_eq!(
        sp.iter().map(|(id, _, _)| id.as_str()).collect::<Vec<_>>(),
        vec!["stale-one"]
    );
    assert!(db
        .list_code_basis_stale(Some("nowhere"))
        .await
        .unwrap()
        .is_empty());

    // Confirming the row (what the steep pass does after its revision card)
    // takes it off the list.
    let locator = wenlan_types::CodeBasisLocator {
        repo_root: repo.root(),
        path: "a.rs".into(),
        line_start: 10,
        line_end: 14,
    };
    crate::code_basis::confirm_code_basis(&db, "stale-one", &locator, "steep")
        .await
        .unwrap();
    let left = db.list_code_basis_stale(Some(SPACE)).await.unwrap();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn needs_human_keeps_the_row_pending_and_shows_the_sentence() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    pending_note(&db, &repo, "n1", BODY).await;
    let provider = ScriptedProvider::saying(&[&verdict(
        "needs_human",
        "The note says 32 bits on purpose; the code now keeps 48.",
    )]);

    let report = slice(&db, &provider, 3).await;
    assert_eq!(report.needs_human, 1);
    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_PENDING);
    assert!(row.changed_at.is_some());
    assert_eq!(
        row.explanation.as_deref(),
        Some("The note says 32 bits on purpose; the code now keeps 48.")
    );
    assert_eq!(row.ai_verdict.as_deref(), Some("needs_human"));
    assert_eq!(body_of(&db, "n1").await, BODY);
    // Only `stale` rows are the steep pass's.
    assert!(db.list_code_basis_stale(None).await.unwrap().is_empty());

    // The same change is not asked about again, however many ticks pass.
    let again = ScriptedProvider::saying(&[&verdict("no_impact", "x")]);
    let report = slice(&db, &again, 3).await;
    assert_eq!(again.calls(), 0);
    assert_eq!(report.skipped, 1);
    assert_eq!(report.llm_calls, 0);
    // Checking the files again keeps the sentence on a row that stays pending.
    recheck_code_basis(&db, None).await.unwrap();
    assert!(only(&db, "n1").await.explanation.is_some());
}

#[tokio::test]
async fn a_further_change_to_the_file_is_judged_afresh() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    pending_note(&db, &repo, "n1", BODY).await;
    let first = ScriptedProvider::saying(&[&verdict("needs_human", "First look.")]);
    slice(&db, &first, 3).await;

    repo.write("a.rs", &file(30, &[12, 13]));
    recheck_code_basis(&db, None).await.unwrap();
    let second = ScriptedProvider::saying(&[&verdict("needs_human", "Second look.")]);
    let report = slice(&db, &second, 3).await;
    assert_eq!(second.calls(), 1);
    assert_eq!(report.needs_human, 1);
    assert_eq!(
        only(&db, "n1").await.explanation.as_deref(),
        Some("Second look.")
    );
}

#[tokio::test]
async fn reverting_the_file_clears_the_models_sentence() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    pending_note(&db, &repo, "n1", BODY).await;
    slice(
        &db,
        &ScriptedProvider::saying(&[&verdict("needs_human", "Look.")]),
        3,
    )
    .await;
    repo.write("a.rs", &file(30, &[]));
    recheck_code_basis(&db, None).await.unwrap();
    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_OK);
    assert_eq!(row.explanation, None);
    assert_eq!(recheck_columns(&db, "n1").await, CLEAR);
}

// ---------------------------------------------------------------------------
// No verdict
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_verdict_is_retried_at_most_three_times_per_change() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    pending_note(&db, &repo, "n1", BODY).await;

    // Garbage, an error and a timeout-like failure all count the same.
    let provider = ScriptedProvider::new(vec![
        Ok("I cannot decide.".to_string()),
        Err(LlmError::InferenceFailed("boom".into())),
        Ok("{\"verdict\":\"???\"}".to_string()),
        Ok(verdict("no_impact", "would settle it")),
    ]);
    for attempt in 1..=3 {
        let report = slice(&db, &provider, 3).await;
        assert_eq!(report.no_verdict, 1, "attempt {attempt}");
        assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_PENDING);
        assert_eq!(only(&db, "n1").await.explanation, None);
        assert_eq!(only(&db, "n1").await.ai_verdict, None);
        assert_eq!(recheck_columns(&db, "n1").await.attempts, attempt);
    }
    // The fourth tick does not ask: the attempts for this change are used up.
    let report = slice(&db, &provider, 3).await;
    assert_eq!(provider.calls(), 3);
    assert_eq!(report.skipped, 1);
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_PENDING);

    // A different change starts again from zero.
    repo.write("a.rs", &file(30, &[12, 13]));
    recheck_code_basis(&db, None).await.unwrap();
    let report = slice(&db, &provider, 3).await;
    assert_eq!(provider.calls(), 4);
    assert_eq!(report.no_impact, 1);
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_OK);
}

// ---------------------------------------------------------------------------
// Races
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_file_that_changes_while_the_model_thinks_makes_the_verdict_a_no_op() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    pending_note(&db, &repo, "n1", BODY).await;
    let provider = ScriptedProvider::saying(&[&verdict("stale", "The note should say 48.")]);
    let root = repo.root();
    provider.during(Arc::new(move || {
        let path = std::path::Path::new(&root).join("a.rs");
        std::fs::write(path, file(30, &[12, 13])).unwrap();
        Box::pin(async {})
    }));

    let report = slice(&db, &provider, 3).await;
    assert_eq!(report.stale, 0);
    assert_eq!(report.skipped, 1);
    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_PENDING);
    assert_eq!(row.explanation, None);
    assert_eq!(row.ai_verdict, None, "the verdict was about older lines");
    assert_eq!(recheck_columns(&db, "n1").await, CLEAR);

    // The next tick judges the file as it reads now.
    let next = ScriptedProvider::saying(&[&verdict("no_impact", "Fine.")]);
    assert_eq!(slice(&db, &next, 3).await.no_impact, 1);
}

#[tokio::test]
async fn a_row_confirmed_by_a_person_while_the_model_thinks_is_left_alone() {
    let (db, _dir) = test_db().await;
    let db = Arc::new(db);
    let repo = Repo::git();
    pending_note(&db, &repo, "n1", BODY).await;
    let provider = ScriptedProvider::saying(&[&verdict("needs_human", "Please look.")]);
    let confirm_db = db.clone();
    let locator = wenlan_types::CodeBasisLocator {
        repo_root: repo.root(),
        path: "a.rs".into(),
        line_start: 10,
        line_end: 14,
    };
    provider.during(Arc::new(move || {
        let db = confirm_db.clone();
        let locator = locator.clone();
        Box::pin(async move {
            crate::code_basis::confirm_code_basis(&db, "n1", &locator, "human")
                .await
                .unwrap();
        })
    }));

    let report = slice(&db, &provider, 3).await;
    assert_eq!(report.needs_human, 0);
    assert_eq!(report.skipped, 1);
    let row = only(&db, "n1").await;
    assert_eq!(row.state, CODE_BASIS_STATE_OK);
    assert_eq!(row.explanation, None);
    assert_eq!(row.ai_verdict, None);
    assert_eq!(confirmed_by(&db, "n1").await, "human");
    assert_eq!(recheck_columns(&db, "n1").await, CLEAR);
}

// ---------------------------------------------------------------------------
// Which rows go to the model
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_slice_asks_about_at_most_limit_rows_oldest_change_first() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    repo.track("a.rs", &file(30, &[]));
    for id in ["n1", "n2", "n3", "n4", "n5"] {
        note(&db, id, BODY).await;
        rely(&db, id, repo.input("a.rs", 10, 14)).await;
    }
    repo.write("a.rs", &file(30, &[12]));
    recheck_code_basis(&db, None).await.unwrap();
    // Make n3 the oldest change and n1 the newest.
    {
        let conn = db.conn.lock().await;
        for (id, at) in [("n3", 10), ("n2", 20), ("n5", 30), ("n4", 40), ("n1", 50)] {
            conn.execute(
                "UPDATE page_code_basis SET changed_at = ?2 WHERE page_id = ?1",
                libsql::params![id, at],
            )
            .await
            .unwrap();
        }
    }
    let provider = ScriptedProvider::saying(&[
        &verdict("no_impact", "a"),
        &verdict("no_impact", "b"),
        &verdict("no_impact", "c"),
        &verdict("no_impact", "d"),
        &verdict("no_impact", "e"),
    ]);
    let report = slice(&db, &provider, RECHECK_ROWS_PER_SLICE).await;
    assert_eq!(report.llm_calls, 3);
    assert_eq!(report.no_impact, 3);
    let asked: Vec<String> = (0..3)
        .map(|call| {
            provider
                .user_prompt(call)
                .lines()
                .find(|line| line.starts_with("NOTE TITLE: "))
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        asked,
        vec![
            "NOTE TITLE: Note n3",
            "NOTE TITLE: Note n2",
            "NOTE TITLE: Note n5"
        ]
    );
    // The two left over are next.
    let report = slice(&db, &provider, RECHECK_ROWS_PER_SLICE).await;
    assert_eq!(report.no_impact, 2);
    assert_eq!(provider.calls(), 5);
}

#[tokio::test]
async fn only_pending_rows_with_findable_lines_reach_the_model() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    repo.track("a.rs", &file(30, &[]));
    repo.track("moved.rs", &file(30, &[]));
    repo.track("gone.rs", &file(30, &[]));
    repo.track("lost.rs", &file(30, &[]));
    for id in ["moved", "gone", "lost", "same"] {
        note(&db, id, BODY).await;
    }
    rely(&db, "moved", repo.input("moved.rs", 10, 14)).await;
    rely(&db, "gone", repo.input("gone.rs", 10, 14)).await;
    rely(&db, "lost", repo.input("lost.rs", 10, 14)).await;
    rely(&db, "same", repo.input("a.rs", 10, 14)).await;
    // Same lines, new place: `moved`. File deleted: `unavailable`. Everything
    // rewritten so nothing can be found: pending but unresolved. Untouched: ok.
    repo.write("moved.rs", &format!("extra\n{}", file(30, &[])));
    std::fs::remove_file(std::path::Path::new(&repo.root()).join("gone.rs")).unwrap();
    repo.write("lost.rs", "completely\ndifferent\n");
    recheck_code_basis(&db, None).await.unwrap();
    assert_eq!(only(&db, "moved").await.state, "moved");
    assert_eq!(only(&db, "gone").await.state, "unavailable");
    assert_eq!(only(&db, "lost").await.state, CODE_BASIS_STATE_PENDING);

    let provider = ScriptedProvider::saying(&[]);
    let report = slice(&db, &provider, 3).await;
    assert_eq!(provider.calls(), 0);
    assert_eq!(report.llm_calls, 0);
    // The unresolved row is remembered, so it is not read again next time.
    assert_eq!(report.skipped, 1);
    assert_eq!(
        recheck_columns(&db, "lost").await.digest.as_deref(),
        Some(UNRESOLVED_DIGEST)
    );
    assert_eq!(only(&db, "moved").await.state, "moved");
    assert_eq!(only(&db, "gone").await.state, "unavailable");
}

#[tokio::test]
async fn a_slice_without_an_available_model_does_nothing() {
    struct Offline;
    #[async_trait::async_trait]
    impl LlmProvider for Offline {
        async fn generate(&self, _: LlmRequest) -> Result<String, LlmError> {
            panic!("an unavailable model must not be called");
        }
        fn is_available(&self) -> bool {
            false
        }
        fn name(&self) -> &str {
            "offline"
        }
        fn backend(&self) -> LlmBackend {
            LlmBackend::Api
        }
    }
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    pending_note(&db, &repo, "n1", BODY).await;
    let provider: Arc<dyn LlmProvider> = Arc::new(Offline);
    let report = run_code_basis_recheck_slice(&db, &provider, &PromptRegistry::default(), 3)
        .await
        .unwrap();
    assert_eq!(report, RecheckSliceReport::default());
    assert_eq!(recheck_columns(&db, "n1").await, CLEAR);
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn confirm_remove_and_a_re_anchor_all_clear_the_four_recheck_columns() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();

    // Confirm.
    pending_note(&db, &repo, "n1", BODY).await;
    slice(
        &db,
        &ScriptedProvider::saying(&[&verdict("stale", "Look.")]),
        3,
    )
    .await;
    let judged = recheck_columns(&db, "n1").await;
    assert_eq!(judged.note.as_deref(), Some("Look."));
    assert_eq!(judged.verdict.as_deref(), Some("stale"));
    assert!(judged.digest.is_some());
    let locator = wenlan_types::CodeBasisLocator {
        repo_root: repo.root(),
        path: "a.rs".into(),
        line_start: 10,
        line_end: 14,
    };
    crate::code_basis::confirm_code_basis(&db, "n1", &locator, "human")
        .await
        .unwrap();
    assert_eq!(recheck_columns(&db, "n1").await, CLEAR);

    // Re-anchor by writing the note again with the same span.
    repo.write("a.rs", &file(30, &[12, 13]));
    recheck_code_basis(&db, None).await.unwrap();
    slice(
        &db,
        &ScriptedProvider::saying(&[&verdict("needs_human", "Look again.")]),
        3,
    )
    .await;
    assert!(recheck_columns(&db, "n1").await.note.is_some());
    assert_eq!(
        recheck_columns(&db, "n1").await.verdict.as_deref(),
        Some("needs_human")
    );
    rely(&db, "n1", repo.input("a.rs", 10, 14)).await;
    assert_eq!(recheck_columns(&db, "n1").await, CLEAR);
    assert_eq!(only(&db, "n1").await.state, CODE_BASIS_STATE_OK);

    // The next check of a file that reverted clears them too.
    repo.write("a.rs", &file(30, &[12, 13, 14]));
    recheck_code_basis(&db, None).await.unwrap();
    slice(
        &db,
        &ScriptedProvider::saying(&[&verdict("stale", "Once more.")]),
        3,
    )
    .await;
    assert_eq!(
        recheck_columns(&db, "n1").await.verdict.as_deref(),
        Some("stale")
    );

    // Remove: the row, and with it the columns, go.
    assert!(db.remove_code_basis("n1", &locator).await.unwrap());
    assert!(rows(&db, "n1").await.is_empty());
}

#[tokio::test]
async fn the_sentence_and_verdict_are_shown_only_while_the_row_is_pending() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    pending_note(&db, &repo, "n1", BODY).await;
    slice(
        &db,
        &ScriptedProvider::saying(&[&verdict("stale", "Look.")]),
        3,
    )
    .await;
    let row = only(&db, "n1").await;
    assert_eq!(row.explanation.as_deref(), Some("Look."));
    assert_eq!(row.ai_verdict.as_deref(), Some("stale"));
    // Values left behind in the columns do not show on a row that is not
    // pending (here: the file went missing).
    std::fs::remove_file(std::path::Path::new(&repo.root()).join("a.rs")).unwrap();
    recheck_code_basis(&db, None).await.unwrap();
    let row = only(&db, "n1").await;
    assert_eq!(row.state, "unavailable");
    assert_eq!(row.explanation, None);
    assert_eq!(row.ai_verdict, None);
    // Blank values never show.
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE page_code_basis SET state = 'pending', recheck_note = '   ', \
             recheck_verdict = '  '",
            (),
        )
        .await
        .unwrap();
    }
    let row = only(&db, "n1").await;
    assert_eq!(row.explanation, None);
    assert_eq!(row.ai_verdict, None);
    // The wire form omits both keys.
    let wire = serde_json::to_value(&row).unwrap();
    assert!(wire.get("explanation").is_none() && wire.get("ai_verdict").is_none());
}

#[tokio::test]
async fn the_brief_knows_when_every_changed_row_has_a_verdict() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    pending_note(&db, &repo, "n1", BODY).await;
    assert!(!db.space_code_basis_all_explained(SPACE).await.unwrap());
    slice(
        &db,
        &ScriptedProvider::saying(&[&verdict("stale", "Look.")]),
        3,
    )
    .await;
    assert!(db.space_code_basis_all_explained(SPACE).await.unwrap());
    // A second changed row nobody has looked at breaks it again.
    note(&db, "n2", BODY).await;
    rely(&db, "n2", repo.input("a.rs", 10, 14)).await;
    repo.write("a.rs", &file(30, &[12, 13]));
    recheck_code_basis(&db, None).await.unwrap();
    assert!(!db.space_code_basis_all_explained(SPACE).await.unwrap());
    // Judged by the model, either way, counts.
    slice(
        &db,
        &ScriptedProvider::saying(&[&verdict("needs_human", "One."), &verdict("stale", "Two.")]),
        3,
    )
    .await;
    assert!(db.space_code_basis_all_explained(SPACE).await.unwrap());
    // Nothing pending at all is not "explained".
    assert!(!db
        .space_code_basis_all_explained("elsewhere")
        .await
        .unwrap());
}

#[tokio::test]
async fn an_attempt_without_a_verdict_does_not_count_as_judged() {
    let (db, _dir) = test_db().await;
    let repo = Repo::git();
    pending_note(&db, &repo, "n1", BODY).await;
    slice(&db, &ScriptedProvider::saying(&["no json here"]), 3).await;
    assert_eq!(recheck_columns(&db, "n1").await.attempts, 1);
    assert_eq!(recheck_columns(&db, "n1").await.verdict, None);
    assert!(!db.space_code_basis_all_explained(SPACE).await.unwrap());
}

#[test]
fn a_blob_id_is_forty_lowercase_hex_characters_and_nothing_else() {
    assert!(is_git_blob_id("0123456789abcdef0123456789abcdef01234567"));
    for bad in [
        "0123456789ABCDEF0123456789abcdef01234567",
        "0123456789abcdef0123456789abcdef0123456",
        "--output=/tmp/x",
        "0123456789abcdef0123456789abcdef0123456g",
        "",
    ] {
        assert!(!is_git_blob_id(bad), "{bad:?}");
    }
}

#[tokio::test]
async fn git_blobs_are_read_by_id_and_a_bad_id_or_folder_is_nothing() {
    let repo = Repo::git();
    repo.track("a.rs", "one\ntwo\n");
    let blob = crate::code_basis::git_blob_id(b"one\ntwo\n");
    assert_eq!(
        read_git_blob(&repo.root(), &blob).await.as_deref(),
        Some("one\ntwo\n")
    );
    assert_eq!(read_git_blob(&repo.root(), "--version").await, None);
    assert_eq!(
        read_git_blob(&repo.root(), &"0".repeat(40)).await,
        None,
        "an object git does not have"
    );
    assert_eq!(read_git_blob("", &blob).await, None);
    assert_eq!(
        read_git_blob(Repo::plain().root().as_str(), &blob).await,
        None
    );
}

#[test]
fn long_excerpts_are_cut_and_marked() {
    let lines: Vec<String> = (1..=500).map(|i| format!("line {i}")).collect();
    let excerpt = render_excerpt(&lines, 1, 500, None);
    assert!(excerpt.truncated);
    assert!(excerpt.text.contains("[300 more lines not shown]"));
    let long = vec!["x".repeat(MAX_LINE_CHARS + 10)];
    let excerpt = render_excerpt(&long, 1, 1, None);
    assert!(excerpt.truncated);
    assert!(excerpt.text.contains("[line cut]"));
    let fine = render_excerpt(&lines, 5, 8, Some(LineSpan { start: 6, end: 7 }));
    assert!(!fine.truncated);
    assert_eq!(
        fine.text,
        " 5 | line 5\n>6 | line 6\n>7 | line 7\n 8 | line 8\n"
    );
}
