use super::{page_dispatch::PageGrowthCommit, WriteOutcome, WriteResult};
use crate::{
    db::{BasisWrite, BasisWriteOutcome, MemoryDB, PageFence},
    error::WenlanError,
    read_scope::ReadScope,
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
    /// Another page was renamed and this page's `[[links]]` to it now name the
    /// new title. A mechanical rewrite of link targets only, computed from the
    /// exact version it fences on; see `post_write::page_link_rename`.
    LinkRename,
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
            "link_rename" => Writer::Pipeline(PipelineStage::LinkRename),
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
                PipelineStage::LinkRename => "link_rename",
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
    /// A person is never held to it: the guard asks whether a machine stayed
    /// close to its sources, and a person writing their own thoughts on their
    /// own page (in the app or in the vault) is not a machine drifting. It
    /// used to refuse a hand edit with "diverges from cited sources", even a
    /// stale-version edit that deserved a version conflict instead.
    ///
    /// A link rewrite after a rename changes only link targets, so the body
    /// says what it said before and there is nothing new to hold to sources.
    pub(super) fn skips_hallucination_guard(&self) -> bool {
        matches!(
            self,
            Writer::Human(_) | Writer::Agent(_) | Writer::Pipeline(PipelineStage::LinkRename)
        ) || self.is_llm_rewrite()
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

/// How many recent versions are read to find the lines a person wrote.
const USER_LINE_HISTORY: i64 = 200;

/// The refusal code for a machine write to an original's page.
const ORIGINAL_PAGE_READ_ONLY: &str = "original_page_is_read_only";

/// A page made from an original (`creation_kind = "original"`: an artifact,
/// canvas or HTML page kept from a chat) is never rewritten by a machine
/// (UX-ART-3). Later facts can only add a note: an agent may send the page
/// as it is with a note added after it, and nothing else. A pipeline never
/// writes it. A person may still edit it, and the mechanical `[[link]]`
/// rewrite after a rename still lands, as it changes link targets only; the
/// kept file under `originals/` is never touched.
fn refuses_machine_rewrite(writer: Writer<'_>, page: &crate::pages::Page, content: &str) -> bool {
    if page.creation_kind != "original" || !writer.is_machine() {
        return false;
    }
    match writer {
        Writer::Pipeline(PipelineStage::LinkRename) => false,
        Writer::Agent(_) => !only_adds_a_note(&page.content, content),
        _ => true,
    }
}

/// Whether `new` is `current` unchanged with something added after it.
pub(crate) fn only_adds_a_note(current: &str, new: &str) -> bool {
    let kept = current.trim_end();
    new.starts_with(kept) && !new[kept.len()..].trim().is_empty()
}

pub fn page_is_human_owned(page: &crate::pages::Page) -> bool {
    page.user_edited || page.creation_kind == "authored"
}

fn selection_review_pending(page: &crate::pages::Page) -> bool {
    page.refresh_blocked_reason.as_deref()
        == Some(wenlan_types::zotero::ZOTERO_SELECTION_REVIEW_REQUIRED)
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
/// - The same holds for the link rewrite that follows a rename the person
///   made: it is computed from the exact version it fences on and changes
///   only link targets. The review lock still gates it, above.
/// - Every other machine writer is gated on a human-owned page, and on a page
///   that holds the person's sentences (`holds_person_text`) even when no
///   ownership flag says so. A forced rebuild is such a writer: pressing
///   rebuild asks for a new draft, not for the person's edits to be replaced,
///   so it waits as a card and the page stays as it is.
///
/// `version_checked` must only be true once the caller's `expected_version`
/// has been compared with the loaded page: a mismatch is refused before this
/// gate runs, so it never reaches here. `holds_person_text` is the caller's
/// answer to "does this page hold sentences that are the person's or
/// original text" (see `sentences::holds_person_sentences`); a caller that did
/// not ask passes `false`.
pub fn machine_write_requires_review(
    writer: Writer<'_>,
    page: &crate::pages::Page,
    version_checked: bool,
    holds_person_text: bool,
) -> bool {
    if !writer.is_machine() {
        return false;
    }
    if page.review_required || selection_review_pending(page) {
        return true;
    }
    match writer {
        Writer::Agent(_) | Writer::Pipeline(PipelineStage::LinkRename) if version_checked => false,
        _ => page_is_human_owned(page) || holds_person_text,
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
        &[],
        None,
    )
    .await
}

/// [`stage_page_revision_card`] naming who proposed the change. `proposer`
/// is the agent the human sees on the card ("proposed by Claude Code"); when
/// it is `None`, a pipeline writer is shown as Wenlan and any other writer
/// leaves the card unattributed rather than showing an internal label.
///
/// `basis_marks` are the notes the write said it relied on, already checked
/// with [`MemoryDB::prepare_staged_basis_marks`]. They ride the card
/// (`basis_marks` in its structured fields) and are recorded when the human
/// accepts it; see `accept_page_revision_card` for a mark that can no longer
/// be applied by then.
///
/// `space_guard` is a Space-locked agent's Space: the card is staged only
/// while the note is still in it, checked in the insert's transaction, and a
/// note moved out of it answers not found with no card.
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
    basis_marks: &[crate::db::StagedBasisMark],
    space_guard: Option<&ReadScope>,
) -> Result<WriteResult, WenlanError> {
    stage_page_revision_card_with_citation_snapshot(
        db,
        page,
        content,
        source_memory_ids,
        source_revision,
        edited_by,
        proposer,
        retry,
        basis_marks,
        space_guard,
        &[],
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn stage_page_revision_card_with_citation_snapshot(
    db: &MemoryDB,
    page: &crate::pages::Page,
    content: &str,
    source_memory_ids: &[String],
    source_revision: i64,
    edited_by: &str,
    proposer: Option<&str>,
    retry: Option<&RetryIdentity>,
    basis_marks: &[crate::db::StagedBasisMark],
    space_guard: Option<&ReadScope>,
    citation_snapshots: &[crate::db::CitationSourceSnapshot],
) -> Result<WriteResult, WenlanError> {
    stage_page_revision_card_with_citations(
        db,
        page,
        content,
        source_memory_ids,
        source_revision,
        edited_by,
        proposer,
        retry,
        basis_marks,
        space_guard,
        citation_snapshots,
        None,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn stage_page_revision_card_with_citations(
    db: &MemoryDB,
    page: &crate::pages::Page,
    content: &str,
    source_memory_ids: &[String],
    source_revision: i64,
    edited_by: &str,
    proposer: Option<&str>,
    retry: Option<&RetryIdentity>,
    basis_marks: &[crate::db::StagedBasisMark],
    space_guard: Option<&ReadScope>,
    citation_snapshots: &[crate::db::CitationSourceSnapshot],
    citations_json: Option<&str>,
    research_write: Option<&crate::db::research_write::ResearchWriteFence>,
) -> Result<WriteResult, WenlanError> {
    stage_page_revision_card_inner(
        db,
        page,
        content,
        source_memory_ids,
        source_revision,
        None,
        space_guard,
        edited_by,
        proposer,
        retry,
        None,
        basis_marks,
        citation_snapshots,
        citations_json,
        research_write,
    )
    .await?
    .ok_or_else(|| match space_guard {
        Some(_) => page_not_in_space(&page.id),
        None => WenlanError::VectorDb(
            "unfenced revision card staging reported a lost page fence".into(),
        ),
    })
}

/// What a Space-locked write answers once the note is no longer in its
/// Space: the same not found as a note that was never there.
fn page_not_in_space(page_id: &str) -> WenlanError {
    WenlanError::NotFound(format!("page {page_id} not found"))
}

fn in_space_guard(basis: &BasisWrite, page: &crate::pages::Page) -> bool {
    basis
        .space_guard
        .as_ref()
        .is_none_or(|scope| scope.matches(page.space.as_deref()))
}

/// [`stage_page_revision_card`] for a caller that captured the page fence
/// before it began compiling. The card commits only while `page` is still
/// the exact row it was computed from (active, same version, same source
/// revision, same incarnation), checked inside the insert's transaction.
/// `Ok(None)` when the fence no longer holds; nothing was staged or logged.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn stage_page_revision_card_at_fence(
    db: &MemoryDB,
    page: &crate::pages::Page,
    content: &str,
    source_memory_ids: &[String],
    fence: &PageFence,
    edited_by: &str,
    retry: Option<&RetryIdentity>,
    basis_marks: &[crate::db::StagedBasisMark],
) -> Result<Option<WriteResult>, WenlanError> {
    stage_page_revision_card_inner(
        db,
        page,
        content,
        source_memory_ids,
        fence.source_revision,
        Some(fence),
        None,
        edited_by,
        None,
        retry,
        None,
        basis_marks,
        &[],
        None,
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
        None,
        edited_by,
        None,
        None,
        Some(basis_snapshot),
        &[],
        &[],
        None,
        None,
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
    space_guard: Option<&ReadScope>,
    edited_by: &str,
    proposer: Option<&str>,
    retry: Option<&RetryIdentity>,
    basis_snapshot: Option<&[crate::db::BasisSnapshotEntry]>,
    basis_marks: &[crate::db::StagedBasisMark],
    citation_snapshots: &[crate::db::CitationSourceSnapshot],
    citations_json: Option<&str>,
    research_write: Option<&crate::db::research_write::ResearchWriteFence>,
) -> Result<Option<WriteResult>, WenlanError> {
    let checked_scope = page
        .space
        .clone()
        .filter(|space| space != crate::db::UNFILED_SPACE_ID)
        .map(ReadScope::Space)
        .unwrap_or(ReadScope::Uncategorized);
    let space_guard = if citation_snapshots.is_empty() {
        space_guard
    } else {
        Some(&checked_scope)
    };
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

    // A page has at most one open card of a kind: a new proposal is staged
    // under a new id and the open card it replaces is retired once the new
    // one is written (below), so an id always names the one body it was
    // staged with and an Accept of the old card can never land this newer
    // body unseen. A card bound to a research write is never retired by
    // another proposal (its adoption fence names it), nor is a card the
    // person changed after staging (`open_page_write_card_id` checks its
    // `proposal_digest`). A link card keeps its own lifecycle
    // (`page_link_rename`).
    let open_card = match research_write {
        None if !matches!(
            Writer::classify(edited_by),
            Writer::Pipeline(PipelineStage::LinkRename)
        ) =>
        {
            db.open_page_write_card_id(&page.id).await?
        }
        _ => None,
    };
    let replaces_open_card = open_card.is_some();
    let revision_card_id = format!(
        "mem_{}",
        uuid::Uuid::new_v4()
            .to_string()
            .replace('-', "")
            .chars()
            .take(12)
            .collect::<String>()
    );
    // One timestamp for the row and its digest: the upsert grounds relative
    // dates against `last_modified`.
    let staged_at = chrono::Utc::now().timestamp();
    // Why this write needed review, so the card can say so: the review lock
    // first, then Wenlan's own background lanes, else the page has the
    // person's edits. A locked page the person wrote is `authored` (pages
    // auto-locked before "Lock pages I've written" was removed are such
    // pages), and the card says they wrote it rather than that they asked to
    // review every change. A pipeline write
    // an AI tool made on demand (steep) names that tool, so it is not called
    // background.
    let review_reason = if selection_review_pending(page) {
        "source_selection_changed"
    } else if page.review_required && page.user_edited {
        "authored"
    } else if page.review_required {
        "locked"
    } else if matches!(Writer::classify(edited_by), Writer::Pipeline(_))
        && proposer.as_deref() == Some("wenlan")
    {
        "background"
    } else {
        "owned"
    };
    let page_incarnation = db
        .try_get_page_fence(&page.id)
        .await?
        .ok_or_else(|| WenlanError::NotFound("revision target is gone".into()))?
        .incarnation;
    let mut structured = serde_json::json!({
        "page_incarnation": page_incarnation,
        "revision_kind": "page_write",
        "target_kind": "page",
        "revises_page": page.id,
        "page_version": page.version,
        "source_revision": source_revision,
        "edited_by": edited_by,
        "source_memory_ids": source_memory_ids,
        "review_reason": review_reason,
        // The body as stored (PII redacted, dates grounded, as the upsert
        // writes it), so a later proposal can tell an untouched card (safe
        // to replace) from one the person changed.
        "proposal_digest": crate::provenance::revision_content_digest(
            &crate::db::revision_cards::stored_card_body(content, staged_at)
        ),
    });
    // Only an update-from-sources card carries the notes it was written from.
    if let Some(snapshot) = basis_snapshot {
        structured["basis_snapshot"] = serde_json::to_value(snapshot)?;
    }
    // The notes an agent's gated write relied on, recorded when it is accepted.
    if !basis_marks.is_empty() {
        structured["basis_marks"] = serde_json::to_value(basis_marks)?;
    }
    if !citation_snapshots.is_empty() {
        structured["citation_source_snapshots"] = serde_json::to_value(citation_snapshots)?;
    }
    if let Some(json) = citations_json {
        structured["citations"] = serde_json::from_str(json)?;
        structured["citation_body_digest"] =
            serde_json::json!(crate::pdf_citations::body_digest(content));
    }
    if let Some(fence) = research_write {
        let mut adoption = fence.clone();
        adoption.revision += 1;
        structured["research_write"] = serde_json::to_value(adoption)?;
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
        last_modified: staged_at,
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
        warnings: if replaces_open_card {
            vec![
                "human-owned page; staged a revision card that replaces the page's open card \
                 instead of overwriting the page or adding a second card"
                    .to_string(),
            ]
        } else {
            vec![
                "human-owned page; staged revision card instead of overwriting content".to_string(),
            ]
        },
        wrote: false,
        revision_card_id: Some(revision_card_id.clone()),
        gated: true,
        outcome: WriteOutcome::Gated,
        acknowledged: false,
        sentence_proposals: Vec::new(),
        edits_not_applied: Vec::new(),
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
    let write = match (fence, space_guard, receipt) {
        // Only an agent's write carries a Space and only a pipeline rebuild
        // carries a page fence; a card is never fenced on both.
        (Some(_), Some(_), _) => {
            return Err(WenlanError::Validation(
                "a revision card is fenced on its page row or on its Space, not both".into(),
            ));
        }
        (Some(fence), None, receipt) => {
            db.upsert_documents_at_page_fence(vec![row], receipt, &page.id, page.version, fence)
                .await
        }
        (None, Some(scope), receipt) => {
            db.upsert_documents_in_page_space(
                vec![row],
                receipt,
                &page.id,
                scope,
                citation_snapshots,
                research_write.map(|fence| (fence, source_memory_ids, revision_card_id.as_str())),
            )
            .await
        }
        (None, None, Some(receipt)) => db
            .upsert_documents_with_operation_receipt(vec![row], receipt)
            .await
            .map(|_| true),
        (None, None, None) => db.upsert_documents(vec![row]).await.map(|_| true),
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
    // Retire the card this one replaces only now that the new card is
    // written: a failure in between leaves two open cards, which the nightly
    // scan reports, never a page with none.
    if let Some(old_card_id) = open_card.as_deref() {
        match db
            .retire_replaced_page_card(
                old_card_id,
                &revision_card_id,
                proposer.as_deref().unwrap_or(edited_by),
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => log::info!(
                "[page_revision_card] open card {old_card_id} of page {} changed before it \
                 could be replaced; left it open beside {revision_card_id}",
                page.id
            ),
            Err(e) => log::warn!(
                "[page_revision_card] retiring replaced card {old_card_id} of page {} \
                 failed: {e}",
                page.id
            ),
        }
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

/// The smallest share of a page an LLM rewrite may keep when
/// `WENLAN_MERGE_SHRINK_GUARD` is not set. Page growth, which only adds,
/// holds itself to 0.7 against the last good version before it gets here.
pub(crate) const REWRITE_MIN_KEPT: f64 = 0.5;

/// Parse WENLAN_MERGE_SHRINK_GUARD env var as f64 threshold.
/// Returns Some(t) when set to a valid float; None when unset/unparseable,
/// in which case callers use their default ([`REWRITE_MIN_KEPT`]).
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
    if !basis.code_basis.is_empty() {
        field(b"code_basis");
        field(&(basis.code_basis.len() as u64).to_le_bytes());
        // The file stamp is left out: a retry of the same request must hash
        // the same however the file's mtime has moved since.
        for prepared in &basis.code_basis {
            let (input, anchor) = (&prepared.input, &prepared.anchor);
            field(input.repo_root.as_bytes());
            field(input.path.as_bytes());
            field(&input.line_start.to_le_bytes());
            field(&input.line_end.to_le_bytes());
            field(anchor.digest.as_bytes());
        }
    }
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
    log_write_outcome(page_id, edited_by, agent, &result);
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

/// One log line for a write that did not simply land: a refusal, or work
/// staged for the person's review instead of applied. Guard rejections log
/// where they happen, since they return an error rather than a result.
fn log_write_outcome(page_id: &str, edited_by: &str, agent: Option<&str>, result: &WriteResult) {
    let agent = agent.unwrap_or("-");
    match result.outcome {
        WriteOutcome::Refused => log::info!(
            "[update_page] refused {edited_by} (agent {agent}) on {page_id}: {}",
            result.warnings.join("; ")
        ),
        WriteOutcome::Gated => log::info!(
            "[update_page] staged {edited_by} (agent {agent}) on {page_id} for review as revision card {}",
            result.revision_card_id.as_deref().unwrap_or("-")
        ),
        WriteOutcome::Wrote if !result.sentence_proposals.is_empty() => log::info!(
            "[update_page] {edited_by} (agent {agent}) on {page_id}: {} sentence change(s) staged for review",
            result.sentence_proposals.len()
        ),
        _ => {}
    }
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
    let carries_basis = basis.minor
        || !basis.marks.is_empty()
        || basis.confirm.is_some()
        || !basis.code_basis.is_empty();
    if carries_basis && (page_growth.is_some() || user_forced || require_stale) {
        return Err(WenlanError::Validation(
            "a basis or minor flag rides only an agent refresh or a human save".into(),
        ));
    }
    // A Space guard only narrows where a write may land. Besides an agent
    // refresh, it rides an on-demand steep rewrite, which is a stale rebuild
    // an agent asked for.
    if basis.space_guard.is_some() && (page_growth.is_some() || user_forced) {
        return Err(WenlanError::Validation(
            "a Space guard rides only an agent's write or a human save".into(),
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
        sentence_proposals: Vec::new(),
        edits_not_applied: Vec::new(),
    };

    let (
        delta_summary,
        current_version,
        new_version,
        basis_outcome,
        was_user_edited,
        gate,
        keeps_person_text,
    ) = 'cas: {
        for attempt in 1..=MAX_CAS_ATTEMPTS {
            let current = db.get_page(page_id).await?.ok_or_else(|| {
                WenlanError::Validation(format!("page '{page_id}' does not exist"))
            })?;
            if !in_space_guard(basis, &current) {
                return Err(page_not_in_space(page_id));
            }
            if refuses_machine_rewrite(writer, &current, &req.content) {
                return Err(WenlanError::Validation(format!(
                    "{ORIGINAL_PAGE_READ_ONLY}: page '{page_id}' keeps an original the AI made \
                     in a chat; it is not rewritten. To add a note, send the page exactly as it \
                     is with the note added after it."
                )));
            }
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
                if page_is_human_owned(&current)
                    || current.review_required
                    || selection_review_pending(&current)
                {
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
            // guards on. A user-forced rebuild of a page the person edited or
            // wrote, or one holding their sentences, gets a revision card and
            // leaves the page as it is, exactly like any other machine
            // rewrite. An agent whose `expected_version` matched above may
            // land on a human-owned page; see `machine_write_requires_review`.
            // Under the page lock, a write that only changes sentences the
            // page already has goes through the sentence gate instead of a
            // whole-page card: every sentence is protected, so each change is
            // a sentence proposal (when its facts changed) and nothing lands.
            // A forced rebuild and a rename's link rewrite keep the card.
            let lock_proposes = basis.citation_snapshots.is_empty()
                && basis.research_write.is_none()
                && writer.is_machine()
                && current.review_required
                && !selection_review_pending(&current)
                && !user_forced
                && !matches!(writer, Writer::Pipeline(PipelineStage::LinkRename))
                && crate::sentences::locked_write_only_changes_sentences(
                    db,
                    &current,
                    &req.content,
                )
                .await?;
            // A forced rebuild of a page no flag marks as the person's still
            // waits as a card when the page holds their sentences (ones they
            // wrote, edited or accepted): replacing those is theirs to
            // decide, whatever the flags say. Original (imported) text alone
            // does not make a card; the sentence gate below keeps it in place.
            let forced_over_person_text = user_forced
                && writer.is_machine()
                && !page_is_human_owned(&current)
                && crate::sentences::holds_person_sentences(db, &current).await?;
            // A forced rebuild that comes back byte-identical, from the same
            // sources, proposes nothing: it is acknowledged below like any
            // no-op compile, with no card.
            let forced_identical = user_forced
                && req.content == current.content
                && current
                    .source_memory_ids
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<std::collections::HashSet<_>>()
                    == source_refs
                        .iter()
                        .copied()
                        .collect::<std::collections::HashSet<_>>();
            if !lock_proposes
                && !forced_identical
                && machine_write_requires_review(
                    writer,
                    &current,
                    req.expected_version.is_some(),
                    forced_over_person_text,
                )
            {
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
                // The marks this write carried ride the card and land when it
                // is accepted. They are checked now, so a refused mark refuses
                // the write here exactly as it would a direct one.
                let staged_marks = db.prepare_staged_basis_marks(page_id, &basis.marks).await?;
                // A rewrite from sources (steep, on demand or automatic)
                // cannot know what the person wrote that no source says, so
                // the card keeps those lines: accepting it must not delete
                // their writing. An agent's own edit is left as written; the
                // person may have asked it to remove a line.
                let (card_content, restored) = if matches!(
                    writer,
                    Writer::Pipeline(PipelineStage::Distill | PipelineStage::ReDistill)
                ) {
                    let history = db.list_page_history(page_id, USER_LINE_HISTORY).await?;
                    let user_lines = crate::pages::user_written_lines(&history, &current.content);
                    crate::pages::keep_user_lines(&req.content, &current.content, &user_lines)
                } else {
                    (req.content.clone(), 0)
                };
                if restored > 0 {
                    log::info!(
                        "[update_page] {page_id}: kept {restored} line(s) the user wrote in the revision card"
                    );
                }
                // A card that would leave the page exactly as it is (the
                // rewrite came back identical once the person's lines were
                // kept) proposes nothing: no card, and the compile is
                // acknowledged like any no-op. A card that changes only the
                // sources, or carries notes, citations or a research write to
                // record on accept, is still a change, and is staged.
                let proposes_nothing = card_content == current.content
                    && staged_marks.is_empty()
                    && basis.citation_snapshots.is_empty()
                    && basis.research_write.is_none()
                    && citations.is_none()
                    && current
                        .source_memory_ids
                        .iter()
                        .map(String::as_str)
                        .collect::<std::collections::HashSet<_>>()
                        == source_refs
                            .iter()
                            .copied()
                            .collect::<std::collections::HashSet<_>>();
                if proposes_nothing {
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
                    return terminal_result_with_receipt(
                        db,
                        retry.as_ref(),
                        no_op(
                            WriteOutcome::Unchanged,
                            vec![
                                "the rewrite proposes no change to this page; no revision card \
                                 staged"
                                    .to_string(),
                            ],
                        ),
                    )
                    .await;
                }
                let result = match page_fence {
                    Some(fence) => match stage_page_revision_card_at_fence(
                        db,
                        &current,
                        &card_content,
                        effective_sources,
                        fence,
                        edited_by,
                        retry.as_ref(),
                        &staged_marks,
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
                        stage_page_revision_card_with_citations(
                            db,
                            &current,
                            &card_content,
                            effective_sources,
                            staged_source_revision,
                            edited_by,
                            agent,
                            retry.as_ref(),
                            &staged_marks,
                            basis.space_guard.as_ref(),
                            &basis.citation_snapshots,
                            citations.as_ref().map(|(json, _)| json.as_str()),
                            basis.research_write.as_ref(),
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

            // Sentence gate. A machine write that reaches this point would
            // land, so it lands only on the AI's sentences: every sentence
            // that is the person's (and, on a human-owned or review-locked
            // page, every sentence of unknown origin) that the write changed
            // or dropped is kept as it is and staged as a `sentence.propose`
            // entry instead. This is what replaced the version-checked agent
            // write landing outright on a human-owned page, and a forced
            // rebuild wiping the person's prose. It applies on every page;
            // there is no "AI may edit directly" setting. A link rewrite
            // after a rename changes link targets only and keeps authors.
            let (gate, keeps_person_text) = if writer.is_machine()
                && !matches!(writer, Writer::Pipeline(PipelineStage::LinkRename))
            {
                crate::sentences::sentence_gate(db, &current, &req.content).await?
            } else {
                (None, false)
            };
            // Checked source generations ride a whole proposal when a person's
            // sentences need approval. Per-sentence proposals do not carry this
            // source fence; keep the page untouched until atomic Adopt/Adjust.
            if (!basis.citation_snapshots.is_empty() || basis.research_write.is_some())
                && gate.as_ref().is_some_and(|plan| !plan.proposals.is_empty())
            {
                let revision = match expected_source_revision {
                    Some(revision) => revision,
                    None => db.get_page_source_revision(page_id).await?,
                };
                let marks = db.prepare_staged_basis_marks(page_id, &basis.marks).await?;
                return stage_page_revision_card_with_citations(
                    db,
                    &current,
                    &req.content,
                    effective_sources,
                    revision,
                    edited_by,
                    agent,
                    retry.as_ref(),
                    &marks,
                    basis.space_guard.as_ref(),
                    &basis.citation_snapshots,
                    citations.as_ref().map(|(json, _)| json.as_str()),
                    basis.research_write.as_ref(),
                )
                .await;
            }
            let landing_content: &str = gate
                .as_ref()
                .map_or(req.content.as_str(), |plan| plan.content.as_str());
            // The landing body keeps the person's sentences with their
            // markers, so their citation records come along from the current
            // page and every record is renumbered to its marker's place.
            let citations: Option<(String, String)> = match &gate {
                Some(plan) => crate::authorship::merge_gate_citations(
                    citations.as_ref().map(|(json, _)| json.as_str()),
                    &current.citations,
                    &plan.markers,
                )
                .map(|json| {
                    let summary = citations
                        .as_ref()
                        .map(|(_, summary)| summary.clone())
                        .unwrap_or_else(|| "citations kept on your sentences".to_string());
                    (json, summary)
                }),
                None if preserve_sources && matches!(writer, Writer::Human(_)) && citations.is_none() => {
                    crate::citations::retain_unchanged_manual_references(
                        &current.content, &req.content, &current.citations,
                    ).map(|records| {
                        let summary = format!("0 verified, {} unevaluated; reference targets retained after manual edit", records.len());
                        serde_json::to_string(&records).map(|json| (json, summary))
                    }).transpose()?
                }
                None => citations.clone(),
            };

            // Shrink-guard: on for every LLM rewrite. A rewrite that keeps
            // less than half the page is a failed rewrite, not an edit (a 9B
            // model shrank one page 2522 -> 209 chars on 2026-11-04).
            // WENLAN_MERGE_SHRINK_GUARD=<f64> overrides the floor; `0` turns
            // it off. Human edits are never blocked. Placed AFTER current page
            // load (needs old body), BEFORE early-return. NOT inside the
            // skips_hallucination_guard block: that skips page_growth/re_distill.
            if writer.is_llm_rewrite() {
                let threshold = merge_shrink_threshold().unwrap_or(REWRITE_MIN_KEPT);
                if !crate::retrieval::integrity::body_shrink_ok(
                    &current.content,
                    landing_content,
                    threshold,
                ) {
                    log::warn!(
                        "[update_page] shrink-guard rejected {edited_by} on {page_id}: new body ({} chars) < {}% of old ({} chars)",
                        landing_content.chars().count(),
                        (threshold * 100.0) as u32,
                        current.content.chars().count(),
                    );
                    return Err(WenlanError::Validation(format!(
                        "page body shrank below {:.0}% of original (shrink-guard); update rejected",
                        threshold * 100.0
                    )));
                }
            }
            // Damage guard (UX-WHO-8): a machine rewrite that brings in
            // broken characters, a chat preamble or a repeated paragraph does
            // not land. A background pass is also refused when it stops
            // mid-sentence, leaves half a link or loses most of the page's
            // CJK text; an agent's write (a translation the person asked for)
            // is not. Only what this write introduced counts.
            if writer.is_llm_rewrite() || matches!(writer, Writer::Agent(_)) {
                if let Some(reason) = crate::retrieval::integrity::rewrite_damage(
                    &current.content,
                    landing_content,
                    writer.is_llm_rewrite(),
                ) {
                    log::warn!(
                        "[update_page] damage-guard rejected {edited_by} on {page_id}: {reason}"
                    );
                    return Err(WenlanError::Validation(format!(
                        "page rewrite refused because {reason} (damage-guard); update rejected"
                    )));
                }
            }

            // ── Build changelog entry ───────────────────────────────────────
            let delta_summary = crate::db::compute_page_delta_summary(
                &current.content,
                &current.source_memory_ids,
                landing_content,
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
            if (delta_summary.is_none() && old_set == new_set) || lock_proposes {
                // Nothing lands, but the changes the write wanted to make to
                // the person's sentences are still proposals. On a locked
                // page nothing lands by design, sources included, and a
                // refused mark still refuses the write, as it would a card.
                if lock_proposes {
                    db.prepare_staged_basis_marks(page_id, &basis.marks).await?;
                }
                let (sentence_proposals, edits_not_applied) = match &gate {
                    Some(plan) => {
                        let staged = crate::sentences::stage_gate_proposals(
                            db,
                            page_id,
                            current_version,
                            plan,
                            edited_by,
                            agent,
                        )
                        .await?;
                        (staged.ids, staged.not_applied)
                    }
                    None => (Vec::new(), Vec::new()),
                };
                // No page write for the marks to ride on, so the notes and
                // lines of code this write relied on (and reconfirms of ones it
                // already had) are recorded on their own, in one transaction. A
                // refused mark still refuses the write, and so does an edit that
                // landed after the comparison above or a move out of the Space.
                // On a locked page the write did not land, so what it relied
                // on describes text the page does not have: nothing is
                // recorded, and the caller is told why.
                let mut lock_warnings = Vec::new();
                if lock_proposes && (!basis.marks.is_empty() || !basis.code_basis.is_empty()) {
                    lock_warnings.push(
                        "basis and code_basis were not recorded: this page is locked, so the \
                         write changed nothing and its sentence changes wait as proposals"
                            .to_string(),
                    );
                }
                if !lock_proposes && (!basis.marks.is_empty() || !basis.code_basis.is_empty()) {
                    match db
                        .mark_page_bases(
                            page_id,
                            current_version,
                            &basis.marks,
                            &basis.code_basis,
                            &basis.actor,
                            basis.space_guard.as_ref(),
                        )
                        .await
                    {
                        Ok(()) => {}
                        Err(crate::db::BasisError::CiterMovedOn { current }) => {
                            return Ok(no_op(
                                WriteOutcome::Refused,
                                vec![format!(
                                    "page moved to v{current} (expected v{current_version}); write refused"
                                )],
                            ));
                        }
                        Err(crate::db::BasisError::CiterNotFound)
                            if basis.space_guard.is_some() =>
                        {
                            return Err(page_not_in_space(page_id));
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                let acknowledged = if require_stale || user_forced {
                    if let Some((caller, operation, digest)) = retry.as_ref() {
                        let acknowledged_result = WriteResult {
                            acknowledged: true,
                            sentence_proposals: sentence_proposals.clone(),
                            edits_not_applied: edits_not_applied.clone(),
                            ..no_op(WriteOutcome::Unchanged, lock_warnings.clone())
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
                        sentence_proposals,
                        edits_not_applied,
                        ..no_op(WriteOutcome::Unchanged, lock_warnings)
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
                landing_content,
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
                match projection.write_page_gated(db, &current).await {
                    Ok(_) => {}
                    // The file holds an edit this very write is reading back
                    // (Obsidian, another editor): keeping it is the normal case.
                    Err(WenlanError::Conflict(reason))
                        if reason.starts_with("page_projection_external_edit") =>
                    {
                        log::info!("[update_page] baseline pending for {page_id}: {reason}");
                    }
                    Err(error) => {
                        log::warn!("[update_page] baseline pending for {page_id}: {error}");
                    }
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
                    sentence_proposals: Vec::new(),
                    edits_not_applied: Vec::new(),
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

            // Files first: the page's file gets this version before the
            // database does. If the database write below does not land, the
            // file is put back when `file_first` drops.
            let file_first = match knowledge_path {
                Some(path) => {
                    let mut next = current.clone();
                    next.content = landing_content.to_string();
                    next.source_memory_ids = effective_sources.clone();
                    next.version = new_version;
                    next.last_modified = chrono::Utc::now().to_rfc3339();
                    let projection = crate::export::knowledge::KnowledgeProjectionWrite::new(
                        path.to_path_buf(),
                        db,
                    );
                    match projection.write_page_first(db, &next).await {
                        Ok(written) => written,
                        // An edit the person made in the file is not
                        // overwritten; the write lands in the database and
                        // the file is brought up to it afterwards, as before.
                        Err(error) => {
                            log::info!("[update_page] {page_id}: file first skipped: {error}");
                            None
                        }
                    }
                }
                None => None,
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
                    landing_content,
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
                    landing_content,
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
                    landing_content,
                    &source_refs,
                    edited_by,
                    require_stale,
                    &new_changelog,
                    citations.as_ref().map(|(json, _)| json.as_str()),
                    source_revision,
                    // Only the agent lane and the link rewrite after a rename
                    // fence on both. Distill also carries an `expected_version`
                    // here, but its write has always been source-revision-only
                    // and the db layer refuses the pair for any other writer.
                    req.expected_version.filter(|_| {
                        matches!(
                            writer,
                            Writer::Agent(_) | Writer::Pipeline(PipelineStage::LinkRename)
                        )
                    }),
                    receipt,
                    basis,
                )
                .await?
            } else if require_stale {
                db.try_update_page_content_with_changelog(
                    page_id,
                    landing_content,
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
                    landing_content,
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
                if let Some(file_first) = file_first {
                    file_first.keep();
                }
                break 'cas (
                    delta_summary,
                    current_version,
                    new_version,
                    outcome,
                    current.user_edited,
                    gate,
                    keeps_person_text,
                );
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
            // The write's own Space check refused it: the note left the
            // Space-locked agent's Space after the check at the top.
            if basis.space_guard.is_some()
                && !landed.as_ref().is_some_and(|p| in_space_guard(basis, p))
            {
                return Err(page_not_in_space(page_id));
            }
            let landed_version = landed.as_ref().map(|p| p.version);
            // A review lock or selection loss after the gate read can leave
            // the version alone. Re-read so the next attempt's gate sees
            // the durable review requirement and stages a card.
            let lock_appeared = writer.is_machine()
                && landed.as_ref().is_some_and(|p| {
                    (!current.review_required && p.review_required)
                        || (!selection_review_pending(&current) && selection_review_pending(p))
                });
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

    // A forced rebuild released `user_edited` with the body it landed; when
    // that body still holds the person's sentences, the page stays theirs.
    if user_forced && was_user_edited && keeps_person_text {
        if let Err(error) = db.keep_user_edited_at_version(page_id, new_version).await {
            log::warn!(
                "[update_page] {page_id}: rebuilt, but keeping it user-edited failed: {error}"
            );
        }
    }
    let mut warnings = write_warnings(&delta_summary, current_version, new_version);
    let (sentence_proposals, edits_not_applied) = match &gate {
        Some(plan) => match crate::sentences::stage_gate_proposals(
            db,
            page_id,
            new_version,
            plan,
            edited_by,
            agent,
        )
        .await
        {
            Ok(staged) => {
                if !staged.ids.is_empty() {
                    warnings.push(format!(
                        "{} change(s) to the person's sentences were staged as proposals",
                        staged.ids.len()
                    ));
                }
                (staged.ids, staged.not_applied)
            }
            Err(error) => {
                // The write has committed with the person's sentences kept;
                // only the proposals for them were lost.
                log::warn!("[update_page] {page_id}: staging sentence proposals failed: {error}");
                warnings.push("the AI's changes to the person's sentences were not kept".into());
                (Vec::new(), crate::sentences::gate_not_applied(plan))
            }
        },
        None => (Vec::new(), Vec::new()),
    };

    Ok(WriteResult {
        id: page_id.to_string(),
        attached_to: None,
        warnings,
        wrote: true,
        revision_card_id: None,
        gated: false,
        outcome: WriteOutcome::Wrote,
        acknowledged: false,
        basis_confirm: basis_outcome.confirm,
        sentence_proposals,
        edits_not_applied,
    })
}
