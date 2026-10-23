// SPDX-License-Identifier: Apache-2.0
//! Automatic mode rewrites a note whose code changed with its own model,
//! through the on-demand claim and commit. These run on the no-embedder
//! scratch database and the code steep fixtures.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use super::{rewrite_one_code_page, CodeRewriteOutcome};
use crate::code_basis::{CODE_BASIS_STATE_OK, CODE_BASIS_STATE_PENDING};
use crate::db::code_basis_test::test_db;
use crate::db::steep_code_test::{changed_note, judge_with, only, verdict, Repo, BODY, FIXED};
use crate::db::MemoryDB;
use crate::llm_provider::{LlmBackend, LlmError, LlmProvider, LlmRequest};
use crate::prompts::PromptRegistry;
use crate::read_scope::ReadScope;

/// Answers every call with `reply` and counts the calls.
struct Scripted {
    reply: String,
    calls: AtomicUsize,
    last_prompt: std::sync::Mutex<String>,
}

impl Scripted {
    fn new(reply: &str) -> Arc<Self> {
        Arc::new(Self {
            reply: reply.to_string(),
            calls: AtomicUsize::new(0),
            last_prompt: std::sync::Mutex::new(String::new()),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmProvider for Scripted {
    async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last_prompt.lock().unwrap() = request.user_prompt;
        Ok(self.reply.clone())
    }
    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "scripted"
    }
    fn backend(&self) -> LlmBackend {
        LlmBackend::OnDevice
    }
}

async fn body_of(db: &MemoryDB, id: &str) -> String {
    db.get_page(id).await.unwrap().unwrap().content
}

async fn stale_note(db: &MemoryDB, repo: &Repo, id: &str) {
    changed_note(db, repo, id, "authored").await;
    judge_with(db, id, &verdict("stale", "The note should say 48 bits.")).await;
}

async fn run(db: &MemoryDB, model: &Arc<Scripted>) -> Option<CodeRewriteOutcome> {
    let llm: Arc<dyn LlmProvider> = model.clone();
    rewrite_one_code_page(db, &llm, &PromptRegistry::default(), None)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_note_judged_stale_is_rewritten_by_the_automatic_model_and_settled() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    stale_note(&db, &repo, "page_a").await;
    // A fence around the whole answer is not part of the page.
    let model = Scripted::new(&format!("```markdown\n{FIXED}\n```"));

    assert_eq!(run(&db, &model).await, Some(CodeRewriteOutcome::Written));
    let prompt = model.last_prompt.lock().unwrap().clone();
    assert!(prompt.contains("line 12 changed"), "{prompt}");
    assert!(prompt.contains("The note should say 48 bits."), "{prompt}");
    assert!(prompt.contains("32 bits"), "{prompt}");
    assert!(prompt.contains("write no [n] markers"), "{prompt}");
    assert_eq!(body_of(&db, "page_a").await, FIXED);
    assert_eq!(only(&db, "page_a").await.state, CODE_BASIS_STATE_OK);

    // Nothing is left for it.
    assert_eq!(run(&db, &model).await, None);
    assert_eq!(model.calls(), 1);
}

#[tokio::test]
async fn code_no_model_has_judged_is_not_rewritten() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    changed_note(&db, &repo, "page_a", "authored").await;
    let model = Scripted::new(FIXED);

    assert_eq!(run(&db, &model).await, None);
    assert_eq!(model.calls(), 0);
    assert_eq!(body_of(&db, "page_a").await, BODY);
}

#[tokio::test]
async fn a_still_accurate_answer_settles_the_code_and_keeps_the_note() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    stale_note(&db, &repo, "page_a").await;
    let model = Scripted::new("STILL_ACCURATE\n");

    assert_eq!(
        run(&db, &model).await,
        Some(CodeRewriteOutcome::StillAccurate)
    );
    assert_eq!(body_of(&db, "page_a").await, BODY);
    assert_eq!(only(&db, "page_a").await.state, CODE_BASIS_STATE_OK);
}

#[tokio::test]
async fn an_answer_that_does_not_land_is_not_asked_for_again() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    stale_note(&db, &repo, "page_a").await;
    // Loses a sentence that still holds, so it is refused.
    let model = Scripted::new("Keys are cut to 48 bits.");

    assert_eq!(run(&db, &model).await, Some(CodeRewriteOutcome::Failed));
    assert_eq!(body_of(&db, "page_a").await, BODY);
    assert_eq!(only(&db, "page_a").await.state, CODE_BASIS_STATE_PENDING);

    // The same note and code would fail the same way.
    assert_eq!(run(&db, &model).await, None);
    assert_eq!(model.calls(), 1);
    // The hold was given back: an on-demand caller can still take it.
    assert!(super::super::claim(&db, "page_a", &ReadScope::Global, None)
        .await
        .is_ok());
}

#[tokio::test]
async fn markers_on_a_note_with_no_sources_are_dropped_not_refused() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    stale_note(&db, &repo, "page_a").await;
    let model = Scripted::new(
        "Keys are cut to 48 bits on purpose [2].\n\nThe cut happens in truncate(). [1, 3]",
    );

    assert_eq!(run(&db, &model).await, Some(CodeRewriteOutcome::Written));
    assert_eq!(body_of(&db, "page_a").await, FIXED);
}

#[tokio::test]
async fn a_rewrite_that_drops_most_of_the_note_is_not_saved() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    stale_note(&db, &repo, "page_a").await;
    // Corrects the first sentence but loses the second, which still holds.
    let model = Scripted::new("Keys are cut to 48 bits.");

    assert_eq!(run(&db, &model).await, Some(CodeRewriteOutcome::Failed));
    assert_eq!(body_of(&db, "page_a").await, BODY);
    assert_eq!(only(&db, "page_a").await.state, CODE_BASIS_STATE_PENDING);
}

#[tokio::test]
async fn a_note_an_on_demand_caller_holds_is_left_to_it() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    stale_note(&db, &repo, "page_a").await;
    super::super::claim(&db, "page_a", &ReadScope::Global, None)
        .await
        .unwrap();
    let model = Scripted::new(FIXED);

    assert_eq!(run(&db, &model).await, None);
    assert_eq!(model.calls(), 0);
}

#[tokio::test]
async fn the_background_redistill_turn_takes_a_code_changed_note() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    stale_note(&db, &repo, "page_a").await;
    let model = Scripted::new(FIXED);
    let llm: Arc<dyn LlmProvider> = model.clone();

    let report = crate::refinery::run_redistill_page_slice(
        &db,
        Some(&llm),
        &PromptRegistry::default(),
        None,
    )
    .await
    .unwrap();
    assert!(report.selected && report.progressed, "{report:?}");
    assert_eq!(report.items_processed, 1);
    assert_eq!(body_of(&db, "page_a").await, FIXED);
}

#[test]
fn a_reply_that_opens_with_a_diagram_keeps_its_fences() {
    let note = "```mermaid\nflowchart LR\n  a[\"CLI\"] --> b[\"daemon\"]\n```\n\nThe CLI calls the daemon.";
    assert_eq!(super::unfence(note), note);
    let wrapped = format!("```markdown\n{note}\n```");
    assert_eq!(super::unfence(&wrapped), note);
}

#[tokio::test]
async fn a_diagram_the_model_broke_is_dropped_and_the_rewrite_kept() {
    let (db, _dir) = test_db().await;
    let repo = Repo::new();
    stale_note(&db, &repo, "page_a").await;
    let model = Scripted::new(&format!(
        "{FIXED}\n\n```mermaid\nflowchart LR\n  cli[\"CLI\"] -->\n```"
    ));

    assert_eq!(run(&db, &model).await, Some(CodeRewriteOutcome::Written));
    assert_eq!(body_of(&db, "page_a").await, FIXED);
}
