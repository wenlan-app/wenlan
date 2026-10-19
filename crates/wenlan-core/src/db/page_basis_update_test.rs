// SPDX-License-Identifier: Apache-2.0
//! "Update from sources" on a note that relies on other notes (B1c, R9).
//!
//! Numbers in the test names refer to the acceptance list in the B1 plan
//! (`2026-10-17-page-cites-page-and-lock-toggle.plan.md`). The model is a
//! recording mock: nothing here calls a real LLM.

use super::{MemoryDB, BASIS_STATE_OK, BASIS_STATE_PENDING};
use crate::llm_provider::{LlmBackend, LlmError, LlmProvider, LlmRequest};
use crate::prompts::PromptRegistry;
use crate::synthesis::distill::{
    force_redistill_single, refresh_page, RefreshOutcome, RefreshReason, REFRESH_BLOCKED_HAS_BASIS,
};
use std::sync::Arc;

const T0: &str = "2026-01-01T00:00:00Z";
const SOURCE_TEXT: &str = "Tokio is an async runtime for Rust programs";
const A_V1: &str = "The launch is on 2026-03-01.";
/// A body whose one memory claim verifies against `SOURCE_TEXT` and whose
/// basis claim is a `[[Alpha]]` link, never a numbered marker.
const BODY: &str =
    "Tokio is an async runtime for Rust programs [1]. The launch is on 2026-03-01, see [[Alpha]].";
/// What a basis-only note gets back: links only, no numbered markers.
const BASIS_ONLY_BODY: &str = "The launch is on 2026-03-01, see [[Alpha]].";

/// Answers every request with `reply` and keeps each user prompt it saw.
struct RecordingProvider {
    reply: String,
    prompts: tokio::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl LlmProvider for RecordingProvider {
    async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
        self.prompts.lock().await.push(request.user_prompt);
        Ok(self.reply.clone())
    }

    fn is_available(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "recording"
    }

    fn backend(&self) -> LlmBackend {
        LlmBackend::OnDevice
    }

    fn kind(&self) -> &'static str {
        "mock"
    }
}

fn provider(reply: &str) -> Arc<RecordingProvider> {
    Arc::new(RecordingProvider {
        reply: reply.to_string(),
        prompts: tokio::sync::Mutex::new(Vec::new()),
    })
}

async fn memory(db: &MemoryDB, id: &str, text: &str) {
    let now_ts = chrono::Utc::now().timestamp();
    let conn = db.conn.lock().await;
    conn.execute(
        "INSERT INTO memories (id, source_id, title, content, chunk_index, chunk_type, memory_type, space, source_agent, created_at, last_modified, confirmed, stability, source) \
         VALUES (?1, ?1, ?1, ?2, 0, 'text', 'fact', 'sp', 'claude-code', ?3, ?3, 1, 'confirmed', 'memory')",
        libsql::params![id.to_string(), text, now_ts],
    )
    .await
    .unwrap();
}

async fn page(db: &MemoryDB, id: &str, title: &str, content: &str, kind: &str, sources: &[&str]) {
    db.insert_page_with_kind(
        id,
        title,
        None,
        content,
        None,
        Some("sp"),
        sources,
        T0,
        kind,
        "confirmed",
        None,
        None,
    )
    .await
    .unwrap();
}

/// `a` (Alpha, authored) is the basis; `b` (Beta, distilled from one memory)
/// relies on it at A's first version and is stale. The card path is exercised
/// through `force_redistill_single`, the entry point of "Update from sources".
async fn pair() -> (Arc<MemoryDB>, tempfile::TempDir) {
    let (db, dir) = crate::db::tests::test_db().await;
    memory(&db, "mem_src", SOURCE_TEXT).await;
    page(&db, "a", "Alpha", A_V1, "authored", &[]).await;
    page(
        &db,
        "b",
        "Beta",
        "Old beta body.",
        "distilled",
        &["mem_src"],
    )
    .await;
    db.mark_page_basis("b", "a", 1, "human").await.unwrap();
    db.set_page_stale("b", "source_updated").await.unwrap();
    (Arc::new(db), dir)
}

async fn update_from_sources(
    db: &MemoryDB,
    llm: &Arc<RecordingProvider>,
    page_id: &str,
) -> RefreshOutcome {
    let llm: Arc<dyn LlmProvider> = llm.clone();
    force_redistill_single(db, Some(&llm), &PromptRegistry::default(), page_id, None)
        .await
        .unwrap()
}

/// The ids of the revision cards waiting on `page_id`.
async fn cards(db: &MemoryDB, page_id: &str) -> Vec<String> {
    db.list_pending_revisions(50)
        .await
        .unwrap()
        .into_iter()
        .filter(|card| card.target_source_id == page_id)
        .map(|card| card.revision_source_id)
        .collect()
}

async fn basis_of(db: &MemoryDB, citer: &str, basis: &str) -> Option<crate::db::PageBasisEntry> {
    db.list_page_basis(citer)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.basis_page_id == basis)
}

async fn edit_a(db: &MemoryDB, content: &str) {
    db.update_page_content("a", content, &[], "manual_edit")
        .await
        .unwrap();
}

/// 7: the run stages a card whose prompt carried A's CURRENT text as context
/// (a link, not a numbered source); accepting the card sets B's basis to ok at
/// A's current version.
#[tokio::test]
async fn acceptance_7_update_from_sources_stages_a_card_and_accepting_it_confirms_the_basis() {
    let (db, _dir) = pair().await;
    edit_a(&db, "The launch moved to 2026-04-01.").await;
    assert_eq!(
        basis_of(&db, "b", "a").await.unwrap().state,
        BASIS_STATE_PENDING,
        "precondition: A's edit left B waiting"
    );
    let before = db.get_page("b").await.unwrap().unwrap();
    let llm = provider(BODY);

    let outcome = update_from_sources(&db, &llm, "b").await;

    assert!(
        !outcome.wrote,
        "a note with a basis is previewed, not written"
    );
    assert!(outcome.gated, "the run lands as a revision card");
    let card_id = outcome.revision_card_id.expect("the card id is returned");
    assert_eq!(cards(&db, "b").await, vec![card_id.clone()]);
    let after_run = db.get_page("b").await.unwrap().unwrap();
    assert_eq!(
        after_run.content, before.content,
        "B is untouched by the preview"
    );
    assert_eq!(after_run.version, before.version);
    assert_eq!(
        basis_of(&db, "b", "a").await.unwrap().state,
        BASIS_STATE_PENDING,
        "a preview confirms nothing"
    );

    let prompts = llm.prompts.lock().await;
    assert_eq!(prompts.len(), 1);
    let prompt = &prompts[0];
    assert!(prompt.contains("## Notes this note relies on"), "{prompt}");
    assert!(prompt.contains("### [[Alpha]]"), "{prompt}");
    assert!(
        prompt.contains("The launch moved to 2026-04-01."),
        "the prompt must carry A's CURRENT text: {prompt}"
    );
    assert!(
        !prompt.contains("The launch is on 2026-03-01."),
        "A's old text must not be in the prompt: {prompt}"
    );
    assert!(
        prompt.contains(SOURCE_TEXT),
        "the memory source is still in the prompt"
    );
    assert!(
        !prompt.contains("[2]"),
        "the basis note must not be a numbered source: {prompt}"
    );
    drop(prompts);

    let a_version = db.get_page("a").await.unwrap().unwrap().version;
    crate::post_write::accept_pending_revision(&db, &card_id, "human")
        .await
        .unwrap();

    let accepted = db.get_page("b").await.unwrap().unwrap();
    assert_eq!(accepted.content, BODY);
    assert_eq!(accepted.version, before.version + 1);
    let entry = basis_of(&db, "b", "a").await.unwrap();
    assert_eq!(entry.state, BASIS_STATE_OK);
    assert_eq!(
        entry.confirmed_version, a_version,
        "confirmed at A's current version"
    );
    assert!(db.list_page_basis("b").await.unwrap().len() == 1);
}

/// 10, first half: a card made from A v2 and accepted after A moved to v3
/// leaves the basis pending: the card never read v3.
#[tokio::test]
async fn acceptance_10_a_basis_that_moved_on_after_the_card_stays_pending() {
    let (db, _dir) = pair().await;
    edit_a(&db, "The launch moved to 2026-04-01.").await;
    let llm = provider(BODY);
    let outcome = update_from_sources(&db, &llm, "b").await;
    let card_id = outcome.revision_card_id.expect("card staged");

    edit_a(&db, "The launch moved again, to 2026-05-01.").await;
    assert_eq!(db.get_page("a").await.unwrap().unwrap().version, 3);
    crate::post_write::accept_pending_revision(&db, &card_id, "human")
        .await
        .unwrap();

    assert_eq!(db.get_page("b").await.unwrap().unwrap().content, BODY);
    let entry = basis_of(&db, "b", "a").await.unwrap();
    assert_eq!(
        entry.state, BASIS_STATE_PENDING,
        "A's v3 was never read by the card"
    );
    assert_eq!(entry.confirmed_version, 1, "the confirmation did not move");
}

/// 10, second half: a basis removed after the card was made is not re-added
/// by accepting the card; the basis that stayed is still confirmed.
#[tokio::test]
async fn acceptance_10_a_basis_removed_after_the_card_is_not_re_added() {
    let (db, _dir) = pair().await;
    page(&db, "c", "Gamma", "Gamma says hello.", "authored", &[]).await;
    db.mark_page_basis("b", "c", 1, "human").await.unwrap();
    edit_a(&db, "The launch moved to 2026-04-01.").await;
    let llm = provider(BODY);
    let outcome = update_from_sources(&db, &llm, "b").await;
    let card_id = outcome.revision_card_id.expect("card staged");
    assert!(
        llm.prompts.lock().await[0].contains("### [[Gamma]]"),
        "precondition: both bases were read"
    );

    assert!(db.unmark_page_basis("b", "c").await.unwrap());
    crate::post_write::accept_pending_revision(&db, &card_id, "human")
        .await
        .unwrap();

    assert!(
        basis_of(&db, "b", "c").await.is_none(),
        "a removed basis must not come back"
    );
    assert_eq!(
        basis_of(&db, "b", "a").await.unwrap().state,
        BASIS_STATE_OK,
        "the basis that stayed is confirmed"
    );
}

/// A bold-only edit moves A's version but not its text digest, so it costs
/// no confirmation: the accept confirms at A's CURRENT version.
#[tokio::test]
async fn a_bold_only_edit_between_the_card_and_the_accept_still_confirms_at_the_current_version() {
    let (db, _dir) = pair().await;
    let llm = provider(BODY);
    let outcome = update_from_sources(&db, &llm, "b").await;
    let card_id = outcome.revision_card_id.expect("card staged");

    edit_a(&db, "The launch is on **2026-03-01**.").await;
    let a_version = db.get_page("a").await.unwrap().unwrap().version;
    assert_eq!(a_version, 2, "precondition: A moved a version");
    crate::post_write::accept_pending_revision(&db, &card_id, "human")
        .await
        .unwrap();

    let entry = basis_of(&db, "b", "a").await.unwrap();
    assert_eq!(
        entry.state, BASIS_STATE_OK,
        "a bold-only edit is not a change"
    );
    assert_eq!(
        entry.confirmed_version, a_version,
        "confirmed at the CURRENT version"
    );
}

/// A note without a basis keeps today's behavior: a direct write, no card,
/// and a prompt with no basis block.
#[tokio::test]
async fn a_note_without_a_basis_writes_directly_and_its_prompt_has_no_basis_block() {
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(&db, "mem_src", SOURCE_TEXT).await;
    page(
        &db,
        "b",
        "Beta",
        "Old beta body.",
        "distilled",
        &["mem_src"],
    )
    .await;
    db.set_page_stale("b", "source_updated").await.unwrap();
    let llm = provider("Tokio is an async runtime for Rust programs [1]");

    let outcome = update_from_sources(&db, &llm, "b").await;

    assert!(outcome.wrote, "no basis, no preview");
    assert!(!outcome.gated);
    assert!(outcome.revision_card_id.is_none());
    assert!(!outcome.acknowledged);
    assert!(cards(&db, "b").await.is_empty());
    let after = db.get_page("b").await.unwrap().unwrap();
    assert_eq!(
        after.content,
        "Tokio is an async runtime for Rust programs [1]"
    );
    assert_eq!(after.stale_reason, None);
    let prompts = llm.prompts.lock().await;
    assert!(
        !prompts[0].contains("Notes this note relies on"),
        "{}",
        prompts[0]
    );
}

/// A note built only from other notes can be rebuilt (the early exits let it
/// through) and its link-only body is not held to the citation gate. A note
/// with neither a source nor a basis still exits without calling the model.
#[tokio::test]
async fn a_basis_only_note_is_rebuilt_from_its_bases_and_a_note_with_nothing_still_exits() {
    let (db, _dir) = crate::db::tests::test_db().await;
    page(&db, "a", "Alpha", A_V1, "authored", &[]).await;
    page(&db, "b", "Beta", "Old beta body.", "authored", &[]).await;
    page(
        &db,
        "lonely",
        "Lonely",
        "Nothing behind it.",
        "authored",
        &[],
    )
    .await;
    db.mark_page_basis("b", "a", 1, "human").await.unwrap();
    let db = Arc::new(db);
    let llm = provider(BASIS_ONLY_BODY);

    let outcome = update_from_sources(&db, &llm, "b").await;

    assert!(outcome.gated, "{outcome:?}");
    let card_id = outcome.revision_card_id.expect("card staged");
    let prompts = llm.prompts.lock().await;
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("### [[Alpha]]"), "{}", prompts[0]);
    assert!(
        !prompts[0].contains("[1]\n") && !prompts[0].contains("## Sources"),
        "no numbered source block for a note with no memory source: {}",
        prompts[0]
    );
    drop(prompts);
    crate::post_write::accept_pending_revision(&db, &card_id, "human")
        .await
        .unwrap();
    assert_eq!(
        db.get_page("b").await.unwrap().unwrap().content,
        BASIS_ONLY_BODY
    );
    assert_eq!(basis_of(&db, "b", "a").await.unwrap().state, BASIS_STATE_OK);

    let llm = provider(BASIS_ONLY_BODY);
    let nothing = update_from_sources(&db, &llm, "lonely").await;
    assert!(!nothing.wrote && !nothing.gated && nothing.revision_card_id.is_none());
    assert!(llm.prompts.lock().await.is_empty(), "no model call");
}

/// The citation gate stays memory-only: a note with a memory source and a
/// basis whose body cites nothing numbered is discarded, whatever it says
/// about the basis. No card is staged.
#[tokio::test]
async fn the_citation_gate_still_judges_a_mixed_note_by_its_memory_citations_only() {
    let (db, _dir) = pair().await;
    let llm = provider(BASIS_ONLY_BODY);

    let outcome = update_from_sources(&db, &llm, "b").await;

    assert!(!outcome.wrote);
    assert!(!outcome.gated, "an unverified body is not staged");
    assert!(outcome.revision_card_id.is_none());
    assert!(outcome.discard_reason.is_some(), "{outcome:?}");
    assert!(cards(&db, "b").await.is_empty());
    assert_eq!(
        db.get_page("b").await.unwrap().unwrap().content,
        "Old beta body."
    );
}

/// A body identical to the note's own, from the same sources: no card, the
/// response says so, and nothing is confirmed.
#[tokio::test]
async fn an_identical_body_stages_no_card_reports_unchanged_and_confirms_nothing() {
    let (db, _dir) = pair().await;
    db.update_page_content("b", BODY, &["mem_src"], "manual_edit")
        .await
        .unwrap();
    edit_a(&db, "The launch moved to 2026-04-01.").await;
    db.set_page_stale("b", "source_updated").await.unwrap();
    let before = db.get_page("b").await.unwrap().unwrap();
    let llm = provider(BODY);

    let outcome = update_from_sources(&db, &llm, "b").await;

    assert!(outcome.acknowledged, "{outcome:?}");
    assert!(!outcome.wrote && !outcome.gated);
    assert!(outcome.revision_card_id.is_none());
    assert!(cards(&db, "b").await.is_empty());
    let after = db.get_page("b").await.unwrap().unwrap();
    assert_eq!(after.version, before.version, "no write");
    assert_eq!(after.stale_reason, None, "the compile watermark advanced");
    assert_eq!(
        basis_of(&db, "b", "a").await.unwrap().state,
        BASIS_STATE_PENDING,
        "nothing was confirmed"
    );
}

/// The automatic refresh leaves a note with a basis alone and says so on the
/// page (`has_basis`, rendered as "update blocked", never as an endless
/// update). A later explicit Update from sources clears the marker.
#[tokio::test]
async fn the_automatic_refresh_skips_a_note_with_a_basis_and_a_user_run_clears_the_marker() {
    let (db, _dir) = pair().await;
    let llm = provider(BODY);
    let dyn_llm: Arc<dyn LlmProvider> = llm.clone();

    let skipped = refresh_page(
        &db,
        &dyn_llm,
        &PromptRegistry::default(),
        "b",
        RefreshReason::SourceChanged,
        None,
    )
    .await
    .unwrap();

    assert!(!skipped.wrote && !skipped.gated && skipped.revision_card_id.is_none());
    assert!(
        llm.prompts.lock().await.is_empty(),
        "the model is never called"
    );
    let marked = db.get_page("b").await.unwrap().unwrap();
    assert_eq!(
        marked.refresh_blocked_reason.as_deref(),
        Some(REFRESH_BLOCKED_HAS_BASIS)
    );
    assert_eq!(marked.content, "Old beta body.");
    assert!(cards(&db, "b").await.is_empty());

    let outcome = update_from_sources(&db, &llm, "b").await;
    assert!(outcome.gated, "{outcome:?}");
    let cleared = db.get_page("b").await.unwrap().unwrap();
    assert_eq!(
        cleared.refresh_blocked_reason, None,
        "a user run clears the marker"
    );
    assert_eq!(cleared.stale_reason, None);
}

/// The skip marker belongs to the basis rows: when the last one goes, the
/// marker goes with it and the note is back in the stale queue.
#[tokio::test]
async fn removing_the_last_basis_clears_the_skip_marker_and_requeues_the_note() {
    let (db, _dir) = pair().await;
    let llm = provider(BODY);
    let dyn_llm: Arc<dyn LlmProvider> = llm.clone();
    refresh_page(
        &db,
        &dyn_llm,
        &PromptRegistry::default(),
        "b",
        RefreshReason::SourceChanged,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        db.get_page("b")
            .await
            .unwrap()
            .unwrap()
            .refresh_blocked_reason
            .as_deref(),
        Some(REFRESH_BLOCKED_HAS_BASIS)
    );
    assert!(
        db.get_stale_page_after("source_updated", None)
            .await
            .unwrap()
            .is_none(),
        "precondition: the skipped note is out of the stale queue"
    );

    assert!(db.unmark_page_basis("b", "a").await.unwrap());

    let after = db.get_page("b").await.unwrap().unwrap();
    assert_eq!(after.refresh_blocked_reason, None);
    let queued = db
        .get_stale_page_after("source_updated", None)
        .await
        .unwrap()
        .expect("the note is back in the stale queue");
    assert_eq!(queued.id, "b");
}

/// Only the has_basis marker is released, and only with the last basis.
#[tokio::test]
async fn removing_one_of_two_bases_or_another_reason_keeps_the_marker() {
    let (db, _dir) = pair().await;
    page(&db, "c", "Gamma", "Another note.", "authored", &[]).await;
    db.mark_page_basis("b", "c", 1, "human").await.unwrap();
    db.set_page_refresh_blocked_reason("b", REFRESH_BLOCKED_HAS_BASIS)
        .await
        .unwrap();

    db.unmark_page_basis("b", "a").await.unwrap();
    assert_eq!(
        db.get_page("b")
            .await
            .unwrap()
            .unwrap()
            .refresh_blocked_reason
            .as_deref(),
        Some(REFRESH_BLOCKED_HAS_BASIS),
        "a basis remains"
    );

    db.set_page_refresh_blocked_reason("b", "citation gate: no verified claims")
        .await
        .unwrap();
    db.unmark_page_basis("b", "c").await.unwrap();
    assert_eq!(
        db.get_page("b")
            .await
            .unwrap()
            .unwrap()
            .refresh_blocked_reason
            .as_deref(),
        Some("citation gate: no verified claims"),
        "a discard reason is not this marker"
    );
}

/// A note with no basis is still refreshed automatically: the skip is keyed
/// on the basis row, nothing else.
#[tokio::test]
async fn the_automatic_refresh_still_rebuilds_a_note_without_a_basis() {
    let (db, _dir) = crate::db::tests::test_db().await;
    memory(&db, "mem_src", SOURCE_TEXT).await;
    page(
        &db,
        "b",
        "Beta",
        "Old beta body.",
        "distilled",
        &["mem_src"],
    )
    .await;
    db.set_page_stale("b", "source_updated").await.unwrap();
    let llm = provider("Tokio is an async runtime for Rust programs [1]");
    let dyn_llm: Arc<dyn LlmProvider> = llm.clone();

    let outcome = refresh_page(
        &db,
        &dyn_llm,
        &PromptRegistry::default(),
        "b",
        RefreshReason::SourceChanged,
        None,
    )
    .await
    .unwrap();

    assert!(outcome.wrote);
    assert_eq!(
        db.get_page("b")
            .await
            .unwrap()
            .unwrap()
            .refresh_blocked_reason,
        None
    );
}

/// Marks `b` as relying on `a` while it is asked to write, as another request
/// would between the refresh's early check and its write.
struct MarkingProvider {
    db: Arc<MemoryDB>,
    reply: String,
}

#[async_trait::async_trait]
impl LlmProvider for MarkingProvider {
    async fn generate(&self, _request: LlmRequest) -> Result<String, LlmError> {
        self.db.mark_page_basis("b", "a", 1, "human").await.unwrap();
        Ok(self.reply.clone())
    }

    fn is_available(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "marking"
    }

    fn backend(&self) -> LlmBackend {
        LlmBackend::OnDevice
    }

    fn kind(&self) -> &'static str {
        "mock"
    }
}

/// A basis marked while the automatic refresh is generating is not missed:
/// the write itself refuses, the body stays, and the skip is recorded.
#[tokio::test]
async fn a_basis_marked_during_the_automatic_refresh_blocks_its_write() {
    let (db, _dir) = crate::db::tests::test_db().await;
    let db = Arc::new(db);
    memory(&db, "mem_src", SOURCE_TEXT).await;
    page(&db, "a", "Alpha", A_V1, "authored", &[]).await;
    page(
        &db,
        "b",
        "Beta",
        "Old beta body.",
        "distilled",
        &["mem_src"],
    )
    .await;
    db.set_page_stale("b", "source_updated").await.unwrap();
    assert!(
        !db.page_has_basis("b").await.unwrap(),
        "precondition: no basis when the refresh starts"
    );
    let before = db.get_page("b").await.unwrap().unwrap();
    let llm: Arc<dyn LlmProvider> = Arc::new(MarkingProvider {
        db: db.clone(),
        reply: BODY.to_string(),
    });

    let outcome = refresh_page(
        &db,
        &llm,
        &PromptRegistry::default(),
        "b",
        RefreshReason::SourceChanged,
        None,
    )
    .await
    .unwrap();

    assert!(db.page_has_basis("b").await.unwrap(), "the mark landed");
    assert!(
        !outcome.wrote && !outcome.gated && outcome.revision_card_id.is_none(),
        "{outcome:?}"
    );
    let after = db.get_page("b").await.unwrap().unwrap();
    assert_eq!(after.content, "Old beta body.");
    assert_eq!(after.version, before.version, "no version was written");
    assert_eq!(
        after.refresh_blocked_reason.as_deref(),
        Some(REFRESH_BLOCKED_HAS_BASIS)
    );
    assert!(cards(&db, "b").await.is_empty());
}
