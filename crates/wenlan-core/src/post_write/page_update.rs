use super::{page_dispatch::PageGrowthCommit, WriteOutcome, WriteResult};
use crate::{
    db::{BasisWrite, BasisWriteOutcome, MemoryDB, PageFence},
    error::WenlanError,
};
use std::path::Path;
use wenlan_types::{requests::UpdatePageRequest, RawDocument};

/// A daemon pipeline stage that rewrites page prose with an LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineStage<'a> {
    Distill,
    ReDistill,
    PageGrowth,
    RefineryMerge,
    /// A writer string the gate does not recognize, carried verbatim.
    ///
    /// Two kinds of string land here. Legitimate ones: `edited_by` values that
    /// reach `page_history` without passing this gate (`create` and
    /// `migration_84` are SQL literals in db.rs; `citation_backfill` and
    /// `revision_accept` write via the db layer). Illegitimate ones: a typo in
    /// a caller's literal. The gate cannot tell them apart, so both get the
    /// conservative answer — see `Writer::is_machine`.
    Unknown(&'a str),
}

/// Who is writing a page, as the spec's `human | agent | pipeline(stage)`.
///
/// This is a **lens over the persisted string, not a replacement for it**.
/// Every page write records provenance as text in `pages.changelog` and
/// `page_history.edited_by`, and those bytes are already in users' databases,
/// so `as_str` round-trips the exact literal that was classified.
///
/// Its job is to put the write gate's three authority questions in one place
/// with one vocabulary. They used to be three independent `matches!` lists over
/// `&str`, which could drift apart silently and had no name for the fallthrough
/// case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writer<'a> {
    /// A person edited the page directly — in the app editor, or in the vault
    /// on disk. The only identity that may overwrite a human-owned page.
    Human(&'a str),
    /// An agent asked for a faithful re-synth of a page it already owns.
    Agent(&'a str),
    /// A daemon pipeline stage.
    Pipeline(PipelineStage<'a>),
}

impl<'a> Writer<'a> {
    /// Total over every `&str` — there is no failure mode, only `Unknown`.
    pub fn classify(edited_by: &'a str) -> Self {
        match edited_by {
            "manual_edit" | "fs_edit" => Writer::Human(edited_by),
            "agent_refresh" => Writer::Agent(edited_by),
            "distill" => Writer::Pipeline(PipelineStage::Distill),
            "re_distill" => Writer::Pipeline(PipelineStage::ReDistill),
            "page_growth" => Writer::Pipeline(PipelineStage::PageGrowth),
            "refinery_merge" => Writer::Pipeline(PipelineStage::RefineryMerge),
            other => Writer::Pipeline(PipelineStage::Unknown(other)),
        }
    }

    /// The persisted `edited_by` literal, byte-identical to what was classified.
    pub fn as_str(&self) -> &'a str {
        match self {
            Writer::Human(s) | Writer::Agent(s) => s,
            Writer::Pipeline(stage) => match stage {
                PipelineStage::Distill => "distill",
                PipelineStage::ReDistill => "re_distill",
                PipelineStage::PageGrowth => "page_growth",
                PipelineStage::RefineryMerge => "refinery_merge",
                PipelineStage::Unknown(s) => s,
            },
        }
    }

    /// Everything that is not a human. **Unknown counts as machine on purpose**:
    /// that is the fail-safe direction, because a machine write to a
    /// human-owned page is staged as a revision card rather than overwriting
    /// the human's prose. Trusting an unrecognized string as human would let it
    /// clobber that prose instead.
    pub(super) fn is_machine(&self) -> bool {
        !matches!(self, Writer::Human(_))
    }

    /// Writers that bypass the hallucination guard. Incremental updates can push
    /// aggregate cosine sim below 0.6; the HTTP/MCP `agent_refresh` route
    /// historically accepted agent-provided refreshes without this guard, so
    /// routing it through PageWrite preserves that behavior.
    ///
    /// This is `is_llm_rewrite` plus `Agent` — the two used to be parallel
    /// `matches!` lists whose only difference was the `agent_refresh` arm, with
    /// a comment warning not to merge them. Here the difference is structural.
    pub(super) fn skips_hallucination_guard(&self) -> bool {
        matches!(self, Writer::Agent(_)) || self.is_llm_rewrite()
    }

    /// A recognized LLM-rewrite stage, checked by the shrink-guard. `Unknown` is
    /// excluded: the shrink-guard rejects a write outright, so an unrecognized
    /// writer must not be able to opt itself into being rejected.
    pub(super) fn is_llm_rewrite(&self) -> bool {
        matches!(
            self,
            Writer::Pipeline(
                PipelineStage::Distill
                    | PipelineStage::ReDistill
                    | PipelineStage::PageGrowth
                    | PipelineStage::RefineryMerge
            )
        )
    }
}

pub fn page_is_human_owned(page: &crate::pages::Page) -> bool {
    page.user_edited || page.creation_kind == "authored"
}

/// The write gate's one ownership question: must this write be staged as a
/// revision card instead of landing on `page`?
///
/// - A human writer is never gated, not even by the review lock.
/// - The review lock (`page.review_required`) gates every machine writer.
/// - An agent that proved it read the current page (`version_checked`: it
///   sent an `expected_version` that matched) may update a human-owned page
///   directly; the human-owned rule exists to stop blind overwrites, and a
///   version-checked write is not blind.
/// - A user-forced rebuild may release `user_edited`, but an authored page
///   still gets a card.
/// - Every other machine writer is gated on a human-owned page.
///
/// `version_checked` must only be true once the caller's `expected_version`
/// has been compared with the loaded page: a mismatch is refused before this
/// gate runs, so it never reaches here.
pub fn machine_write_requires_review(
    writer: Writer<'_>,
    page: &crate::pages::Page,
    version_checked: bool,
    user_forced: bool,
) -> bool {
    if !writer.is_machine() {
        return false;
    }
    if page.review_required {
        return true;
    }
    match writer {
        Writer::Agent(_) if version_checked => false,
        _ if user_forced => page.creation_kind == "authored",
        _ => page_is_human_owned(page),
    }
}

/// Stage a machine write to a human-owned page as a pending revision card
/// instead of overwriting the page's prose. Uses the same grammar as L3
/// doc-grounded revisions (`crate::reconcile::write_revision`): a
/// `source='memory'`, `pending_revision=1`, `supersedes=<page id>` row that
/// `list_pending_revisions` surfaces on the `/curate revisions` queue. The page
/// itself is never mutated here — the human accepts or dismisses the card.
/// Returns a gated `WriteResult` carrying the new card id.
///
/// `source_revision` is required: every caller must fence the card with a
/// real counter. Note this is an *acceptance-time staging token*, not
/// necessarily a compile fence — a caller that captured its own counter
/// before it began compiling passes that value straight through (a true
/// compile fence, catching staleness from before the request even started);
/// a caller with no such value falls back to reading the counter here, at
/// staging time, which only fences staging→accept and cannot detect a
/// request that was already stale before it reached this call.
pub async fn stage_page_revision_card(
    db: &MemoryDB,
    page: &crate::pages::Page,
    content: &str,
    source_memory_ids: &[String],
    source_revision: i64,
    edited_by: &str,
    retry: Option<&RetryIdentity>,
) -> Result<WriteResult, WenlanError> {
    stage_page_revision_card_proposed_by(
        db,
        page,
        content,
        source_memory_ids,
        source_revision,
        edited_by,
        None,
        retry,
    )
    .await
}

/// [`stage_page_revision_card`] naming who proposed the change. `proposer`
/// is the agent the human sees on the card ("proposed by Claude Code"); when
/// it is `None`, a pipeline writer is shown as Wenlan and any other writer
/// leaves the card unattributed rather than showing an internal label.
#[allow(clippy::too_many_arguments)]
pub async fn stage_page_revision_card_proposed_by(
    db: &MemoryDB,
    page: &crate::pages::Page,
    content: &str,
    source_memory_ids: &[String],
    source_revision: i64,
    edited_by: &str,
    proposer: Option<&str>,
    retry: Option<&RetryIdentity>,
) -> Result<WriteResult, WenlanError> {
    stage_page_revision_card_inner(
        db,
        page,
        content,
        source_memory_ids,
        source_revision,
        None,
        edited_by,
        proposer,
        retry,
        None,
    )
    .await?
    .ok_or_else(|| {
        WenlanError::VectorDb("unfenced revision card staging reported a lost page fence".into())
    })
}

/// [`stage_page_revision_card`] for a caller that captured the page fence
/// before it began compiling. The card commits only while `page` is still
/// the exact row it was computed from (active, same version, same source
/// revision, same incarnation), checked inside the insert's transaction.
/// `Ok(None)` when the fence no longer holds; nothing was staged or logged.
pub(crate) async fn stage_page_revision_card_at_fence(
    db: &MemoryDB,
    page: &crate::pages::Page,
    content: &str,
    source_memory_ids: &[String],
    fence: &PageFence,
    edited_by: &str,
    retry: Option<&RetryIdentity>,
) -> Result<Option<WriteResult>, WenlanError> {
    stage_page_revision_card_inner(
        db,
        page,
        content,
        source_memory_ids,
        fence.source_revision,
        Some(fence),
        edited_by,
        None,
        retry,
        None,
    )
    .await
}

/// The preview card of an "Update from sources" run on a note that relies on
/// other notes (R9). Staged exactly like [`stage_page_revision_card_at_fence`],
/// plus `basis_snapshot`: what each basis note looked like when the body was
/// written, so accepting the card confirms only the ones that still match.
pub(crate) async fn stage_basis_preview_card(
    db: &MemoryDB,
    page: &crate::pages::Page,
    content: &str,
    source_memory_ids: &[String],
    fence: &PageFence,
    edited_by: &str,
    basis_snapshot: &[crate::db::BasisSnapshotEntry],
) -> Result<Option<WriteResult>, WenlanError> {
    stage_page_revision_card_inner(
        db,
        page,
        content,
        source_memory_ids,
        fence.source_revision,
        Some(fence),
        edited_by,
        None,
        None,
        Some(basis_snapshot),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn stage_page_revision_card_inner(
    db: &MemoryDB,
    page: &crate::pages::Page,
    content: &str,
    source_memory_ids: &[String],
    source_revision: i64,
    fence: Option<&PageFence>,
    edited_by: &str,
    proposer: Option<&str>,
    retry: Option<&RetryIdentity>,
    basis_snapshot: Option<&[crate::db::BasisSnapshotEntry]>,
) -> Result<Option<WriteResult>, WenlanError> {
    crate::export::provenance::validate_canonical_page_content(content)?;
    // Who the human sees on the card. Never the internal `page_write` kind:
    // the named agent when the caller knows it, Wenlan for its own pipeline
    // lanes, otherwise nothing.
    let proposer: Option<String> = proposer
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            matches!(Writer::classify(edited_by), Writer::Pipeline(_)).then(|| "wenlan".to_string())
        });

    let revision_card_id = format!(
        "mem_{}",
        uuid::Uuid::new_v4()
            .to_string()
            .replace('-', "")
            .chars()
            .take(12)
            .collect::<String>()
    );
    // Why this write needed review, so the card can say so: the review lock
    // first, then Wenlan's own background lanes, else the page became
    // user-owned while the write was in flight.
    let review_reason = if page.review_required {
        "locked"
    } else if matches!(Writer::classify(edited_by), Writer::Pipeline(_)) {
        "background"
    } else {
        "owned"
    };
    let mut structured = serde_json::json!({
        "revision_kind": "page_write",
        "target_kind": "page",
        "revises_page": page.id,
        "page_version": page.version,
        "source_revision": source_revision,
        "edited_by": edited_by,
        "source_memory_ids": source_memory_ids,
        "review_reason": review_reason,
    });
    // Only an update-from-sources card carries the notes it was written from.
    if let Some(snapshot) = basis_snapshot {
        structured["basis_snapshot"] = serde_json::to_value(snapshot)?;
    }
    let structured = structured.to_string();
    let title: String = format!("Revision: {}", page.title)
        .chars()
        .take(80)
        .collect();
    let row = RawDocument {
        source: "memory".to_string(),
        source_id: revision_card_id.clone(),
        title,
        content: content.to_string(),
        last_modified: chrono::Utc::now().timestamp(),
        memory_type: Some("fact".to_string()),
        space: page.space.clone().or_else(|| page.workspace.clone()),
        source_agent: proposer.clone(),
        confidence: Some(0.9),
        confirmed: Some(false),
        stability: Some("new".to_string()),
        supersedes: Some(page.id.clone()),
        pending_revision: true,
        structured_fields: Some(structured.clone()),
        source_text: Some(content.to_string()),
        ..Default::default()
    };
    let result = WriteResult {
        basis_confirm: None,
        id: page.id.clone(),
        attached_to: None,
        warnings: vec![
            "human-owned page; staged revision card instead of overwriting content".to_string(),
        ],
        wrote: false,
        revision_card_id: Some(revision_card_id.clone()),
        gated: true,
        outcome: WriteOutcome::Gated,
        acknowledged: false,
    };
    let response = match retry {
        Some(_) => Some(serde_json::to_string(&result)?),
        None => None,
    };
    let receipt = match (retry, response.as_deref()) {
        (Some((caller, operation, digest)), Some(response)) => Some(crate::db::OperationReceipt {
            caller_id: caller,
            operation_id: operation,
            request_digest: digest,
            response,
        }),
        _ => None,
    };
    let write = match (fence, receipt) {
        (Some(fence), receipt) => {
            db.upsert_documents_at_page_fence(vec![row], receipt, &page.id, page.version, fence)
                .await
        }
        (None, Some(receipt)) => db
            .upsert_documents_with_operation_receipt(vec![row], receipt)
            .await
            .map(|_| true),
        (None, None) => db.upsert_documents(vec![row]).await.map(|_| true),
    };
    match (write, retry) {
        (Ok(true), _) => {}
        (Ok(false), _) => return Ok(None),
        (Err(error @ WenlanError::Conflict(_)), Some(retry_identity)) => {
            return replay_matching_operation_receipt(db, retry_identity, error)
                .await
                .map(Some);
        }
        (Err(error), _) => return Err(error),
    }
    if let Err(e) = db
        .log_agent_activity(
            proposer.as_deref().unwrap_or(edited_by),
            "page_revision_card",
            &[page.id.clone(), revision_card_id.clone()],
            None,
            &structured,
        )
        .await
    {
        log::warn!("[page_revision_card] activity log failed: {e}");
    }

    Ok(Some(result))
}

/// Parse WENLAN_MERGE_SHRINK_GUARD env var as f64 threshold.
/// Returns Some(t) when set to a valid float; None when unset/unparseable
/// (guard OFF = byte-identical behavior to pre-T17).
/// Mirrors page_channel_enabled() env-read discipline in db.rs.
pub(crate) fn merge_shrink_threshold() -> Option<f64> {
    std::env::var("WENLAN_MERGE_SHRINK_GUARD")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
}

/// Test-only seam. A test installs a `(page_id, parked, go)` handshake here and
/// `update_page_impl` uses it once for that page only, *after* deciding
/// ownership and *before* writing — i.e. in the exact window a competing edit
/// has to land in. It announces that it is parked, then blocks until released,
/// so the test can land a full competing write in between with no ordering
/// guesswork. Binding the seam to a page keeps unrelated parallel tests from
/// consuming it.
///
/// This is the only way to deterministically exercise the version CAS: with no
/// interleaving edit, a guarded write and an unguarded one behave identically.
///
/// One-shot (`take`), so the retry attempt after a CAS miss runs unblocked.
/// Compiled out entirely in non-test builds.
#[cfg(test)]
type PreWriteGate = (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

/// One armed gate per page id, so race tests on different pages can run in
/// parallel without overwriting each other's gate.
#[cfg(test)]
pub(crate) static PRE_WRITE_GATE: std::sync::Mutex<Vec<PreWriteGate>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(test)]
async fn pre_write_pause(page_id: &str) {
    let gate = {
        let mut slots = PRE_WRITE_GATE.lock().unwrap();
        slots
            .iter()
            .position(|(target, _, _)| target == page_id)
            .map(|i| slots.swap_remove(i))
    };
    if let Some((_, parked, go)) = gate {
        let _ = parked.send(());
        let _ = go.await;
    }
}

#[cfg(not(test))]
#[inline(always)]
async fn pre_write_pause(_page_id: &str) {}

/// Test-only seam at the production projection site. The real projection call
/// completes first, then a matching test can observe the already-committed DB,
/// page history, receipt, and projected file before releasing an injected
/// error into the production best-effort handling branch.
#[cfg(test)]
type PostCommitProjectionGate = (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

#[cfg(test)]
pub(super) static POST_COMMIT_PROJECTION_GATE: std::sync::Mutex<Option<PostCommitProjectionGate>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
async fn post_commit_projection_pause_and_fail(
    page_id: &str,
    result: Result<Option<String>, WenlanError>,
) -> Result<Option<String>, WenlanError> {
    let gate = {
        let mut slot = POST_COMMIT_PROJECTION_GATE.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|(target, _, _)| target == page_id)
        {
            slot.take()
        } else {
            None
        }
    };
    if let Some((_, parked, resume)) = gate {
        let _ = parked.send(());
        let _ = resume.await;
        return Err(WenlanError::VectorDb(
            "injected post-commit projection failure".to_string(),
        ));
    }
    result
}

#[cfg(not(test))]
#[inline(always)]
async fn post_commit_projection_pause_and_fail(
    _page_id: &str,
    result: Result<Option<String>, WenlanError>,
) -> Result<Option<String>, WenlanError> {
    result
}

#[allow(clippy::too_many_arguments)]
/// The advisory line a successful page update returns. Shared between the
/// receipt and the return value so a replay hands back exactly what the
/// original call did, rather than a lookalike rebuilt at replay time.
fn write_warnings(delta_summary: &Option<String>, from: i64, to: i64) -> Vec<String> {
    match delta_summary {
        Some(summary) => vec![format!("v{from} → v{to}: {summary}")],
        None => vec![],
    }
}

/// Fingerprint of a page write, used to tell an honest retry (same request,
/// replay the stored response) from an operation id being reused for a
/// different write (a conflict).
///
/// Covers everything that decides what the write does: which page, the body,
/// the sources, who is writing, and the version precondition. Field lengths
/// are hashed alongside the values so two different requests cannot collide by
/// shifting a boundary — `["ab","c"]` and `["a","bc"]` must not agree.
fn page_write_digest(
    page_id: &str,
    req: &UpdatePageRequest,
    edited_by: &str,
    preserve_sources: bool,
    basis: &BasisWrite,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut field = |bytes: &[u8]| {
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    field(page_id.as_bytes());
    field(req.content.as_bytes());
    field(edited_by.as_bytes());
    if preserve_sources {
        // Server-derived state is intentionally excluded: an honest retry is
        // the same caller request even if another writer attached a source
        // after the first response was lost.
        field(b"preserve-sources");
    } else {
        field(b"replace-sources");
        field(&(req.source_memory_ids.len() as u64).to_le_bytes());
        for sid in &req.source_memory_ids {
            field(sid.as_bytes());
        }
    }
    match req.expected_version {
        Some(v) => {
            field(b"v");
            field(&v.to_le_bytes());
        }
        None => field(b"-"),
    }
    // The basis part is appended only when the write carries one, so a digest
    // recorded before notes could rely on notes still matches its retry.
    if basis.minor || !basis.marks.is_empty() || basis.confirm.is_some() {
        field(b"basis");
        field(&[u8::from(basis.minor)]);
        field(&(basis.marks.len() as u64).to_le_bytes());
        for (basis_page_id, version) in &basis.marks {
            field(basis_page_id.as_bytes());
            field(&version.to_le_bytes());
        }
        match &basis.confirm {
            Some((basis_page_id, version)) => {
                field(b"confirm");
                field(basis_page_id.as_bytes());
                field(&version.to_le_bytes());
            }
            None => field(b"-"),
        }
    }
    format!("{:x}", hasher.finalize())
}

pub(super) type RetryIdentity = (String, String, String);

/// A transaction-coupled receipt insert can lose a race only after its domain
/// mutation was rolled back. In that case the winning transaction is the
/// authoritative response for an identical digest; a different digest remains
/// an operation-id conflict.
async fn replay_matching_operation_receipt(
    db: &MemoryDB,
    retry: &RetryIdentity,
    original_error: WenlanError,
) -> Result<WriteResult, WenlanError> {
    let (caller, operation, digest) = retry;
    let Some(stored) = db.get_operation_receipt(caller, operation).await? else {
        return Err(original_error);
    };
    if stored.request_digest != *digest {
        return Err(WenlanError::Conflict(format!(
            "operation id '{operation}' was already used by '{caller}' for a \
             different page write"
        )));
    }
    serde_json::from_str::<WriteResult>(&stored.response).map_err(|e| {
        WenlanError::VectorDb(format!(
            "receipt for {caller}/{operation} is unreadable: {e}"
        ))
    })
}

/// Persist a successful terminal response that made no domain mutation. If an
/// identical concurrent attempt won the receipt race, replay what it stored;
/// a different digest remains an operation-id conflict.
async fn terminal_result_with_receipt(
    db: &MemoryDB,
    retry: Option<&RetryIdentity>,
    result: WriteResult,
) -> Result<WriteResult, WenlanError> {
    let Some(retry_identity @ (caller, operation, digest)) = retry else {
        return Ok(result);
    };
    let response = serde_json::to_string(&result)?;
    let receipt = crate::db::OperationReceipt {
        caller_id: caller,
        operation_id: operation,
        request_digest: digest,
        response: &response,
    };
    match db.record_operation_receipt(receipt).await {
        Ok(()) => Ok(result),
        Err(error @ WenlanError::Conflict(_)) => {
            replay_matching_operation_receipt(db, retry_identity, error).await
        }
        Err(error) => Err(error),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn update_page_impl(
    db: &MemoryDB,
    page_id: &str,
    req: UpdatePageRequest,
    edited_by: &str,
    require_stale: bool,
    expected_source_revision: Option<i64>,
    knowledge_path: Option<&Path>,
    citations: Option<(String, String)>,
    page_growth: Option<PageGrowthCommit<'_>>,
    preserve_sources: bool,
    user_forced: bool,
    page_fence: Option<&PageFence>,
    agent: Option<&str>,
    basis: &BasisWrite,
) -> Result<WriteResult, WenlanError> {
    let mut result = update_page_db_impl(
        db,
        page_id,
        req,
        edited_by,
        require_stale,
        expected_source_revision,
        knowledge_path,
        citations,
        page_growth,
        preserve_sources,
        user_forced,
        page_fence,
        agent,
        basis,
    )
    .await?;
    // A confirm asked for with a save that did not land (unchanged text, a
    // contended write) confirmed nothing; say so rather than saying nothing.
    if basis.confirm.is_some() && result.basis_confirm.is_none() {
        result.basis_confirm = Some(wenlan_types::BasisConfirmOutcome::NotSaved);
    }
    // Receipts describe the DB transaction. Repairing an independent
    // projection on replay must neither change that receipt nor replay old
    // content over a newer Page. Unchanged saves repair too.
    if matches!(
        result.outcome,
        WriteOutcome::Wrote | WriteOutcome::Unchanged
    ) {
        if let Some(path) = knowledge_path {
            project_latest_page(db, page_id, path).await;
        }
    }
    Ok(result)
}

async fn project_latest_page(db: &MemoryDB, page_id: &str, path: &Path) {
    let result = async {
        let page = db
            .get_page(page_id)
            .await?
            .ok_or_else(|| WenlanError::Validation(format!("page '{page_id}' does not exist")))?;
        let projection =
            crate::export::knowledge::KnowledgeProjectionWrite::new(path.to_path_buf(), db);
        let result = projection.write_page_gated(db, &page).await;
        post_commit_projection_pause_and_fail(page_id, result).await
    }
    .await;
    if let Err(error) = result {
        log::warn!("[update_page] md repair pending for {page_id}: {error}");
    }
}

#[allow(clippy::too_many_arguments)]
async fn update_page_db_impl(
    db: &MemoryDB,
    page_id: &str,
    req: UpdatePageRequest,
    edited_by: &str,
    require_stale: bool,
    expected_source_revision: Option<i64>,
    knowledge_path: Option<&Path>,
    citations: Option<(String, String)>,
    page_growth: Option<PageGrowthCommit<'_>>,
    preserve_sources: bool,
    user_forced: bool,
    page_fence: Option<&PageFence>,
    agent: Option<&str>,
    basis: &BasisWrite,
) -> Result<WriteResult, WenlanError> {
    // ── Pre-write validation ────────────────────────────────────────────────
    if req.content.trim().is_empty() {
        return Err(WenlanError::Validation(
            "page content must not be empty".into(),
        ));
    }
    // Marks and a confirm ride the two writes that have a person or an agent
    // behind them. Every pipeline write fails closed rather than dropping them.
    let carries_basis = basis.minor || !basis.marks.is_empty() || basis.confirm.is_some();
    if carries_basis && (page_growth.is_some() || user_forced || require_stale) {
        return Err(WenlanError::Validation(
            "a basis or minor flag rides only an agent refresh or a human save".into(),
        ));
    }
    // Preserve mode derives provenance from the Page generation inside the
    // CAS. It is intentionally human-only: a machine writer must declare the
    // source set its output was computed from.
    let writer = Writer::classify(edited_by);
    // A user-forced rebuild may replace a `user_edited` body, so it must be a
    // machine rewrite fenced on the page fence (source revision AND
    // incarnation) it captured before compiling. The revision alone cannot
    // see the page deleted and recreated under the same id. Every terminal
    // action below (body write, staged card, acknowledgement, staleness
    // clear) carries that one fence.
    if user_forced
        && (!writer.is_machine()
            || preserve_sources
            || require_stale
            || expected_source_revision.is_none()
            || page_fence.map(|fence| fence.source_revision) != expected_source_revision
            || page_growth.is_some())
    {
        return Err(WenlanError::Validation(
            "a user-forced page rebuild requires a machine write under a source-revision fence"
                .into(),
        ));
    }
    if !user_forced && page_fence.is_some() {
        return Err(WenlanError::Validation(
            "a page incarnation fence is only carried by a user-forced page rebuild".into(),
        ));
    }
    if preserve_sources && writer.is_machine() {
        return Err(WenlanError::Validation(
            "only human Page writes may preserve server-owned sources".into(),
        ));
    }
    // A machine replacement write must carry provenance. A human write need not: an
    // authored page is legitimately born with zero sources (create_page
    // allows exactly that), so demanding one here would reject every later
    // human edit of that page — in the app and in the vault alike.
    // The check itself runs inside the CAS loop below, against the loaded
    // page: an empty set is accepted only when the page has no sources to
    // drop (an authored page written from a conversation).
    let machine_empty_sources =
        !preserve_sources && req.source_memory_ids.is_empty() && writer.is_machine();
    // Source-existence check removed. create_page validates sources at
    // creation time. Updates only carry forward or extend an already-valid
    // source list; re-checking on every update would break daemon-internal
    // callers (fs_edit, re_distill) whose sources may reference pruned
    // memories.

    // ── Conditional hallucination guard ────────────────────────────────────
    if !preserve_sources && !writer.skips_hallucination_guard() {
        let passed =
            crate::kg_quality::hallucination_guard(db, &req.content, &req.source_memory_ids)
                .await?;
        if !passed {
            return Err(WenlanError::Validation(
                "page body diverges from cited sources (cos sim < 0.6)".into(),
            ));
        }
    }

    // ── Retry identity ──────────────────────────────────────────────────────
    // A caller that sends both ids gets exactly-once semantics. The same pair
    // with the same request replays the recorded response without writing
    // again; the same pair with a different request is refused rather than
    // quietly becoming a second version. Either id alone is ignored — an
    // operation id only means anything within the caller that minted it.
    let retry = match (req.caller_id.as_deref(), req.operation_id.as_deref()) {
        (Some(caller), Some(operation)) if !caller.is_empty() && !operation.is_empty() => Some((
            caller.to_string(),
            operation.to_string(),
            page_write_digest(page_id, &req, edited_by, preserve_sources, basis),
        )),
        _ => None,
    };
    if let Some((caller, operation, digest)) = retry.as_ref() {
        if let Some(stored) = db.get_operation_receipt(caller, operation).await? {
            if stored.request_digest != *digest {
                return Err(WenlanError::Conflict(format!(
                    "operation id '{operation}' was already used by '{caller}' for a \
                     different page write"
                )));
            }
            log::debug!("[update_page] {page_id}: replaying receipt for {caller}/{operation}");
            return serde_json::from_str::<WriteResult>(&stored.response).map_err(|e| {
                WenlanError::VectorDb(format!(
                    "receipt for {caller}/{operation} is unreadable: {e}"
                ))
            });
        }
    }

    // Preserve receipt replay/collision precedence, then reject daemon-owned
    // projection delimiters before ownership gating, revision-card staging, or
    // any canonical page/projection mutation.
    crate::export::provenance::validate_canonical_page_content(&req.content)?;

    // ── Load, decide ownership, and write under one version CAS ─────────────
    // The ownership decision is made from a loaded row, and the write CASes on
    // *that row's* version — so the row we decided from is provably the row we
    // wrote. An edit landing in the gap fails the CAS instead of being
    // clobbered, and we reload and re-decide rather than forcing the write
    // through: a page that became human-owned in the gap gets a revision card.
    //
    // Bounded because each retry only re-runs on a version that actually moved;
    // a caller losing three races in a row is a write storm, not a lost update,
    // and yielding is the safe answer.
    const MAX_CAS_ATTEMPTS: usize = 3;
    let no_op = |outcome: WriteOutcome, warnings: Vec<String>| WriteResult {
        id: page_id.to_string(),
        attached_to: None,
        warnings,
        wrote: false,
        revision_card_id: None,
        gated: false,
        outcome,
        acknowledged: false,
        basis_confirm: None,
    };

    let (delta_summary, current_version, new_version, basis_outcome) = 'cas: {
        for attempt in 1..=MAX_CAS_ATTEMPTS {
            let current = db.get_page(page_id).await?.ok_or_else(|| {
                WenlanError::Validation(format!("page '{page_id}' does not exist"))
            })?;
            if machine_empty_sources && !current.source_memory_ids.is_empty() {
                return Err(WenlanError::Validation(
                    "page must cite at least one source memory".into(),
                ));
            }
            let effective_sources = if preserve_sources {
                &current.source_memory_ids
            } else {
                &req.source_memory_ids
            };
            if preserve_sources && !writer.skips_hallucination_guard() {
                let passed =
                    crate::kg_quality::hallucination_guard(db, &req.content, effective_sources)
                        .await?;
                if !passed {
                    return Err(WenlanError::Validation(
                        "page body diverges from cited sources (cos sim < 0.6)".into(),
                    ));
                }
            }
            let source_refs: Vec<&str> = effective_sources.iter().map(|s| s.as_str()).collect();

            // Page Growth is computed against one exact machine-owned Page
            // generation. It must never retarget the inference to a newer
            // version or stage a card onto a Page that became human-owned.
            if let Some(guard) = page_growth {
                if current.version != guard.expected_page_version {
                    return Ok(no_op(WriteOutcome::Refused, vec![]));
                }
                // A review-locked Page is skipped the same way: Page Growth
                // never stages cards, it only grows machine-owned Pages.
                if page_is_human_owned(&current) || current.review_required {
                    return terminal_result_with_receipt(
                        db,
                        retry.as_ref(),
                        no_op(WriteOutcome::Unchanged, vec![]),
                    )
                    .await;
                }
            }

            // `require_stale` asks "write only if this page is stale". A page
            // that is not stale is the answer to that question, not a lost
            // write — and it must short-circuit *here*, before the unchanged
            // early-return below, which would otherwise acknowledge a compile
            // against a page that was never stale to begin with.
            if require_stale && current.stale_reason.is_none() {
                return terminal_result_with_receipt(
                    db,
                    retry.as_ref(),
                    no_op(WriteOutcome::Unchanged, vec![]),
                )
                .await;
            }

            let current_version = current.version;
            let new_version = current_version + 1;

            // A caller-supplied `expected_version` is a precondition, not a retry
            // hint: once it stops matching, the write is refused outright rather
            // than re-aimed at a row the caller never saw.
            //
            // This has to come BEFORE the ownership gate, not after. Staging a
            // card first would take content the agent computed against an old
            // base and bind it to the version we just loaded — so accepting that
            // card silently reverts whatever the human wrote in between. Refusing
            // first drops no agent work either: the caller re-reads the fresh
            // content and stages a better card against it.
            if let Some(expected) = req.expected_version {
                if expected != current_version {
                    log::debug!(
                        "[update_page] {page_id}: expected_version {expected} != current {current_version}; refusing write"
                    );
                    return Ok(no_op(
                        WriteOutcome::Refused,
                        vec![format!(
                            "page moved to v{current_version} (expected v{expected}); write refused"
                        )],
                    ));
                }
            }

            // Ownership gate, re-evaluated on every attempt. Inside the CAS loop
            // it is no longer advisory: whatever it decided is what the write
            // guards on. A user-forced rebuild overrides `user_edited` (the
            // write releases it together with the new body) but never an
            // authored page or the review lock, which still get a revision
            // card. An agent whose `expected_version` matched above may land
            // on a human-owned page; see `machine_write_requires_review`.
            if machine_write_requires_review(
                writer,
                &current,
                req.expected_version.is_some(),
                user_forced,
            ) {
                // `expected_source_revision` is the caller's compile fence
                // when it supplied one. A caller with none (e.g. the generic
                // `update_page` wrapper) gets an acceptance-time staging
                // token instead: the counter as of right now, which fences
                // staging→accept for this card but cannot detect that the
                // caller's own compile was already stale before it got here
                // — see `stage_page_revision_card`'s doc comment.
                let staged_source_revision = match expected_source_revision {
                    Some(source_revision) => source_revision,
                    None => db.get_page_source_revision(page_id).await?,
                };
                let result = match page_fence {
                    Some(fence) => match stage_page_revision_card_at_fence(
                        db,
                        &current,
                        &req.content,
                        effective_sources,
                        fence,
                        edited_by,
                        retry.as_ref(),
                    )
                    .await?
                    {
                        Some(result) => result,
                        None => {
                            log::info!(
                                "[update_page] {page_id}: page changed or was replaced after the rebuild began; no revision card staged"
                            );
                            return terminal_result_with_receipt(
                                db,
                                retry.as_ref(),
                                no_op(WriteOutcome::Unchanged, vec![]),
                            )
                            .await;
                        }
                    },
                    None => {
                        stage_page_revision_card_proposed_by(
                            db,
                            &current,
                            &req.content,
                            effective_sources,
                            staged_source_revision,
                            edited_by,
                            agent,
                            retry.as_ref(),
                        )
                        .await?
                    }
                };
                // A gated compile still consumed the staleness it was dispatched
                // for: the work landed as a revision card awaiting review, so the
                // page must not be re-compiled on the next sweep. Clearing at the
                // source revision keeps that safe — a source that moved since
                // dispatch leaves the page stale. A fenced rebuild also requires
                // the same row incarnation.
                if require_stale || user_forced {
                    let _ = match page_fence {
                        Some(fence) => {
                            db.clear_page_staleness_at_fence(page_id, current.version, fence)
                                .await?
                        }
                        None => {
                            db.clear_page_staleness_at_source_revision(
                                page_id,
                                current.version,
                                expected_source_revision,
                            )
                            .await?
                        }
                    };
                }
                return Ok(result);
            }

            // Shrink-guard (T17): opt-in via WENLAN_MERGE_SHRINK_GUARD=<f64>.
            // OFF by default: unset/unparseable = None = zero regression.
            // Only fires for LLM-rewrite edited_by; human edits are never blocked.
            // Placed AFTER current page load (needs old body), BEFORE early-return.
            // NOT inside the skips_hallucination_guard block: that skips page_growth/re_distill.
            if writer.is_llm_rewrite() {
                if let Some(threshold) = merge_shrink_threshold() {
                    if !crate::retrieval::integrity::body_shrink_ok(
                        &current.content,
                        &req.content,
                        threshold,
                    ) {
                        log::warn!(
                            "[update_page] shrink-guard rejected {edited_by} on {page_id}: new body ({} chars) < {}% of old ({} chars)",
                            req.content.chars().count(),
                            (threshold * 100.0) as u32,
                            current.content.chars().count(),
                        );
                        return Err(WenlanError::Validation(format!(
                            "page body shrank below {:.0}% of original (shrink-guard); update rejected",
                            threshold * 100.0
                        )));
                    }
                }
            }

            // ── Build changelog entry ───────────────────────────────────────
            let delta_summary = crate::db::compute_page_delta_summary(
                &current.content,
                &current.source_memory_ids,
                &req.content,
                &source_refs,
                edited_by,
            );

            // Compute added sources for the changelog entry
            let old_set: std::collections::HashSet<&str> = current
                .source_memory_ids
                .iter()
                .map(|s| s.as_str())
                .collect();
            let new_set: std::collections::HashSet<&str> = source_refs.iter().copied().collect();

            // Early return: identical content and identical source set — nothing to write.
            // A stale page that recompiles to the byte-identical body still had its
            // compile done, so acknowledge it; otherwise the sweep re-dispatches the
            // same no-op work forever.
            if delta_summary.is_none() && old_set == new_set {
                let acknowledged = if require_stale || user_forced {
                    if let Some((caller, operation, digest)) = retry.as_ref() {
                        let acknowledged_result = WriteResult {
                            acknowledged: true,
                            ..no_op(WriteOutcome::Unchanged, vec![])
                        };
                        let response = serde_json::to_string(&acknowledged_result)?;
                        let receipt = crate::db::OperationReceipt {
                            caller_id: caller,
                            operation_id: operation,
                            request_digest: digest,
                            response: &response,
                        };
                        let acknowledged = match page_fence {
                            Some(fence) => {
                                db.acknowledge_page_compile_at_fence(
                                    page_id,
                                    current_version,
                                    fence,
                                    Some(receipt),
                                )
                                .await?
                            }
                            None => {
                                db.acknowledge_page_compile_with_receipt(
                                    page_id,
                                    current_version,
                                    expected_source_revision,
                                    receipt,
                                )
                                .await?
                            }
                        };
                        if acknowledged {
                            return Ok(acknowledged_result);
                        }
                        false
                    } else {
                        match page_fence {
                            Some(fence) => {
                                db.acknowledge_page_compile_at_fence(
                                    page_id,
                                    current_version,
                                    fence,
                                    None,
                                )
                                .await?
                            }
                            None => {
                                db.acknowledge_page_compile(
                                    page_id,
                                    current_version,
                                    expected_source_revision,
                                )
                                .await?
                            }
                        }
                    }
                } else {
                    false
                };
                return terminal_result_with_receipt(
                    db,
                    retry.as_ref(),
                    WriteResult {
                        acknowledged,
                        ..no_op(WriteOutcome::Unchanged, vec![])
                    },
                )
                .await;
            }

            let mut added_sources: Vec<&str> = new_set.difference(&old_set).copied().collect();
            added_sources.sort_unstable();
            let added_sources_json = serde_json::Value::Array(
                added_sources
                    .iter()
                    .map(|s| serde_json::Value::String(s.to_string()))
                    .collect(),
            );

            let mut entry = serde_json::json!({
                "version": new_version,
                "at": chrono::Utc::now().timestamp(),
                "edited_by": edited_by,
                "delta_summary": delta_summary,
                "incoming_source_ids": added_sources_json,
            });
            crate::db::add_page_delta_counts(
                &mut entry,
                &current.content,
                &current.source_memory_ids,
                &req.content,
                &source_refs,
            );
            if let Some(agent) = agent {
                entry["agent"] = serde_json::Value::String(agent.to_string());
            }
            if basis.minor {
                entry["minor"] = serde_json::Value::Bool(true);
            }
            if let Some((_, ref stats_summary)) = citations {
                entry["citations_summary"] = serde_json::Value::String(stats_summary.clone());
            }

            // Read existing changelog and append the new entry
            let existing_cl = db.get_page_changelog(page_id).await?;
            const DEFAULT_CHANGELOG_CAP: usize = 20;
            let new_changelog =
                crate::db::append_changelog_entry(&existing_cl, entry, DEFAULT_CHANGELOG_CAP)?;

            // Seed a legacy byte baseline from the exact CAS generation.
            // Differing external prose is preserved; this is best-effort and
            // cannot fail a canonical write. A lost CAS reloads and rechecks.
            if let Some(path) = knowledge_path {
                let projection =
                    crate::export::knowledge::KnowledgeProjectionWrite::new(path.to_path_buf(), db);
                if let Err(error) = projection.write_page_gated(db, &current).await {
                    log::warn!("[update_page] baseline pending for {page_id}: {error}");
                }
            }

            // ── Apply DB update ─────────────────────────────────────────────
            // The receipt records the response this call is about to return,
            // so a replay hands back the identical envelope rather than a
            // reconstruction. It commits inside the write's own transaction.
            let receipt_response = match retry {
                Some(_) => Some(serde_json::to_string(&WriteResult {
                    id: page_id.to_string(),
                    attached_to: None,
                    warnings: write_warnings(&delta_summary, current_version, new_version),
                    wrote: true,
                    revision_card_id: None,
                    gated: false,
                    outcome: WriteOutcome::Wrote,
                    acknowledged: false,
                    basis_confirm: None,
                })?),
                None => None,
            };
            let receipt = match (retry.as_ref(), receipt_response.as_deref()) {
                (Some((caller, operation, digest)), Some(response)) => {
                    Some(crate::db::OperationReceipt {
                        caller_id: caller,
                        operation_id: operation,
                        request_digest: digest,
                        response,
                    })
                }
                _ => None,
            };

            pre_write_pause(page_id).await;
            // citations: None -> resets `citations` to SQL NULL (no fresh
            // citation source for this write; a stale claim-map must not
            // survive a content change, and the new body re-enters bounded
            // annotation).
            // A source-revision caller is guarded by the revision it compiled
            // against. An agent that also sent `expected_version` (already
            // checked equal to `current_version` above) fences on both, so a
            // version bump that leaves the source revision alone, such as a
            // human rename, still refuses the write.
            let landed: Option<BasisWriteOutcome> = if let Some(guard) = page_growth {
                db.try_update_page_growth_at_versions(
                    page_id,
                    &req.content,
                    &source_refs,
                    &new_changelog,
                    citations.as_ref().map(|(json, _)| json.as_str()),
                    guard.expected_page_version,
                    guard.expected_source_revision,
                    guard.source_id,
                    guard.expected_memory_version,
                )
                .await?
                .then(BasisWriteOutcome::default)
            } else if let (true, Some(fence)) = (user_forced, page_fence) {
                db.try_user_forced_page_content_at_source_revision(
                    page_id,
                    &req.content,
                    &source_refs,
                    edited_by,
                    &new_changelog,
                    citations.as_ref().map(|(json, _)| json.as_str()),
                    fence.source_revision,
                    &fence.incarnation,
                    receipt,
                )
                .await?
                .then(BasisWriteOutcome::default)
            } else if let Some(source_revision) = expected_source_revision {
                db.try_update_page_content_with_changelog_at_source_revision_and_basis(
                    page_id,
                    &req.content,
                    &source_refs,
                    edited_by,
                    require_stale,
                    &new_changelog,
                    citations.as_ref().map(|(json, _)| json.as_str()),
                    source_revision,
                    // Only the agent lane fences on both. Distill also carries
                    // an `expected_version` here, but its write has always been
                    // source-revision-only and the db layer refuses the pair
                    // for any writer but `agent_refresh`.
                    req.expected_version
                        .filter(|_| matches!(writer, Writer::Agent(_))),
                    receipt,
                    basis,
                )
                .await?
            } else if require_stale {
                db.try_update_page_content_with_changelog(
                    page_id,
                    &req.content,
                    &source_refs,
                    edited_by,
                    true,
                    &new_changelog,
                    citations.as_ref().map(|(json, _)| json.as_str()),
                    Some(current_version),
                    receipt,
                )
                .await?
                .then(BasisWriteOutcome::default)
            } else {
                db.try_update_page_content_with_changelog_at_version_and_basis(
                    page_id,
                    &req.content,
                    &source_refs,
                    edited_by,
                    &new_changelog,
                    citations.as_ref().map(|(json, _)| json.as_str()),
                    current_version,
                    receipt,
                    basis,
                )
                .await?
            };

            if let Some(outcome) = landed {
                break 'cas (delta_summary, current_version, new_version, outcome);
            }

            // Nothing was written. Two distinguishable causes share this branch:
            // the `require_stale` gate (row untouched) and a version conflict
            // (row moved under us). Only the latter is worth retrying.
            //
            // Losing every CAS attempt used to return a bare no-op — byte-identical
            // to the "content is already what you asked for" return above. A caller
            // whose work was thrown away under contention could not tell that apart
            // from having had nothing to do, so it now says so.
            //
            // The `require_stale` skip stays silent on purpose: the caller asked to
            // write only if the page was stale, and it wasn't. That is the answer to
            // the question it asked, not a discarded write, and it is the common
            // path on every re-distill sweep.
            let landed = db.get_page(page_id).await?;
            let landed_version = landed.as_ref().map(|p| p.version);
            // A review lock set after the gate read leaves the version alone,
            // but the UPDATE's lock predicate matched zero rows. Re-read so
            // the next attempt's gate sees the lock and stages a card.
            let lock_appeared = writer.is_machine()
                && !current.review_required
                && landed.as_ref().is_some_and(|p| p.review_required);
            if landed_version != Some(current_version) || lock_appeared {
                if attempt < MAX_CAS_ATTEMPTS {
                    log::debug!(
                        "[update_page] {page_id}: version moved {current_version} -> {landed_version:?} mid-write; reloading (attempt {attempt})"
                    );
                    continue;
                }
                log::warn!(
                    "[update_page] {page_id}: gave up after {MAX_CAS_ATTEMPTS} attempts; page still moving"
                );
                return Ok(no_op(
                    WriteOutcome::Contended,
                    vec![format!(
                        "page kept moving under this write ({MAX_CAS_ATTEMPTS} attempts); nothing was written"
                    )],
                ));
            }
            return terminal_result_with_receipt(
                db,
                retry.as_ref(),
                no_op(WriteOutcome::Unchanged, vec![]),
            )
            .await;
        }
        // Unreachable: every path through the loop returns, continues, or breaks
        // with a value. The compiler cannot prove that, so the block needs a
        // tail value.
        return terminal_result_with_receipt(
            db,
            retry.as_ref(),
            no_op(WriteOutcome::Unchanged, vec![]),
        )
        .await;
    };

    Ok(WriteResult {
        id: page_id.to_string(),
        attached_to: None,
        warnings: write_warnings(&delta_summary, current_version, new_version),
        wrote: true,
        revision_card_id: None,
        gated: false,
        outcome: WriteOutcome::Wrote,
        acknowledged: false,
        basis_confirm: basis_outcome.confirm,
    })
}
