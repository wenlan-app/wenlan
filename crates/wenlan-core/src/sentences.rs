// SPDX-License-Identifier: Apache-2.0
//! Who wrote each sentence, sentence proposals, and the sentence write gate.
//!
//! The derivation itself is pure (`crate::authorship`); this module reads its
//! inputs (page history, the sentence history entries, the stored citations
//! and their sources) and writes the decisions:
//!
//! - [`sentence_gate`] is the write gate for a machine write that would land:
//!   it keeps every sentence that is the person's (or of unknown origin on a
//!   human-owned or review-locked page, or imported original text) and plans
//!   one `sentence.propose` entry for each such sentence the write changed or
//!   dropped, but only when the facts behind that sentence changed (its
//!   source does not back it, changed since it was cited, or disproves it).
//!   A rewording alone is dropped and the sentence stays as the person wrote
//!   it, so the person reviews as little as possible. It applies to every
//!   page: there is no "AI may edit directly" mode. On a locked page
//!   (`review_required`) every sentence is protected; a locked write that
//!   would add sentences still stages a whole-page card before this gate.
//! - [`propose_sentence`], [`accept_sentence_proposal`],
//!   [`reject_sentence_proposal`] and [`flag_sentence`] are the per-sentence
//!   decisions, each one history entry. They sit beside the whole-page
//!   revision cards; neither replaces the other.
//! - [`page_sentences`] and [`recheck_sentence`] answer "who said it and how
//!   well is it backed", per sentence.

use std::collections::{HashMap, HashSet};

use wenlan_types::authorship::{
    ActorKind, AiEditMode, EvidenceState, HistoryActor, HistoryEntry, HistoryTarget, PageSentence,
    PageSentencesResponse, SentenceAcceptResponse, SentenceAuthor, SentenceAuthorship,
    SentenceCitation, SentenceFlagResponse, SentenceProposal, SentenceProposalStatus,
    SentenceRecheckResponse, WeakReason, OP_SENTENCE_ACCEPT, OP_SENTENCE_FLAG, OP_SENTENCE_PROPOSE,
    OP_SENTENCE_REJECT,
};
use wenlan_types::pages::PageCitation;

use crate::authorship::{self, GatePlan, SentenceUnit, VersionText};
use crate::db::sentence_history::{cited_text_digest, NewHistoryEntry};
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::pages::Page;

/// How many versions the derivation reads. A page with more starts its
/// derivation from unknown origin at the oldest version read.
const HISTORY_LIMIT: i64 = 2_000;

/// `edited_by` of the synthetic version standing for a body that differs from
/// the newest history row (a write that left no history row). Unknown origin.
const UNRECORDED_WRITE: &str = "unrecorded";

fn page_target(page_id: &str) -> HistoryTarget {
    HistoryTarget {
        kind: "page".to_string(),
        id: page_id.to_string(),
    }
}

fn person() -> HistoryActor {
    HistoryActor {
        kind: ActorKind::Person,
        name: "human".to_string(),
        agent: None,
        session: None,
    }
}

fn str_field<'a>(entry: &'a HistoryEntry, key: &str) -> Option<&'a str> {
    entry.data.get(key).and_then(serde_json::Value::as_str)
}

/// The page's AI-change mode. One rule for every page: the AI updates its
/// own untouched sentences and proposes on the person's. The page's lock
/// (`review_required`) makes every AI change wait for the person.
pub fn ai_edit_mode(page: &Page) -> AiEditMode {
    if page.review_required {
        AiEditMode::Locked
    } else {
        AiEditMode::Propose
    }
}

/// A page whose unknown-origin sentences are protected like the person's.
fn guarded(page: &Page) -> bool {
    crate::post_write::page_is_human_owned(page) || page.review_required
}

/// The creation kind that decides who wrote a page's first version when
/// that version is an older `create` row. Newer pages record
/// `person_create` or `agent_create` and need none of this.
///
/// `authored` alone does not mean the person wrote it: an AI tool's page
/// with no sources lands as `authored` too. An authored page no person has
/// touched was an AI tool's. Once the person has edited it, `user_edited`
/// says only that they edited it, not that they made it ("having seen it
/// does not make it yours"), so its first version's author is unknown: the
/// person's edits still count as theirs, the rest stays unattributed.
fn birth_kind<'a>(page: &'a Page, history: &[crate::db::PageHistoryEntry]) -> &'a str {
    if page.creation_kind != "authored" {
        return &page.creation_kind;
    }
    if !page.user_edited {
        return "agent_authored";
    }
    let person_edited_later = history.iter().any(|entry| {
        entry.version > 1
            && authorship::version_author(&entry.edited_by, "") == authorship::VersionAuthor::Person
    });
    if person_edited_later {
        "legacy_authored"
    } else {
        "authored"
    }
}

/// Derive who wrote each sentence of `page`'s current body.
pub async fn page_authorship(
    db: &MemoryDB,
    page: &Page,
) -> Result<Vec<SentenceAuthorship>, WenlanError> {
    let history = db.list_page_history(&page.id, HISTORY_LIMIT).await?;
    let truncated = i64::try_from(history.len()).unwrap_or(i64::MAX) >= HISTORY_LIMIT;
    let mut versions: Vec<VersionText<'_>> = history
        .iter()
        .rev()
        .map(|entry| VersionText {
            version: entry.version,
            edited_by: &entry.edited_by,
            content: &entry.content,
        })
        .collect();
    if history
        .first()
        .is_none_or(|newest| newest.content != page.content)
    {
        versions.push(VersionText {
            version: page.version,
            edited_by: UNRECORDED_WRITE,
            content: &page.content,
        });
    }
    let mut accepted: HashMap<i64, HashSet<String>> = HashMap::new();
    for entry in db.list_sentence_history("page", &page.id).await? {
        if entry.op != OP_SENTENCE_ACCEPT {
            continue;
        }
        if let (Some(version), Some(hash)) = (entry.version, str_field(&entry, "accepted_hash")) {
            accepted
                .entry(version)
                .or_default()
                .insert(hash.to_string());
        }
    }
    Ok(authorship::derive_authorship(
        &page.id,
        birth_kind(page, &history),
        &versions,
        &accepted,
        truncated,
    ))
}

/// The machine-write gate. The plan is `None` when the write lands as
/// written: nothing the person owns is touched. The flag says the page holds protected sentences at all, which a
/// forced rebuild uses to keep the page marked as the person's.
pub(crate) async fn sentence_gate(
    db: &MemoryDB,
    page: &Page,
    new_content: &str,
) -> Result<(Option<GatePlan>, bool), WenlanError> {
    let rows = page_authorship(db, page).await?;
    let guard = guarded(page);
    let owned: HashSet<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| authorship::is_protected(row.author, guard))
        .map(|(idx, _)| idx)
        .collect();
    // The lock protects every sentence, the AI's own included.
    let protected: HashSet<usize> = if page.review_required {
        (0..rows.len()).collect()
    } else {
        owned.clone()
    };
    let mut plan = authorship::plan_sentence_gate(&page.content, new_content, &protected);
    if let Some(plan) = plan.as_mut() {
        let units = authorship::sentence_units(&page.content);
        let reasons = unit_reasons(db, page, &units).await?;
        let by_hash: HashMap<&str, WeakReason> = units
            .iter()
            .zip(&reasons)
            .filter_map(|(unit, reason)| Some((unit.hash.as_str(), (*reason)?)))
            .collect();
        plan.proposals.retain_mut(|proposal| {
            proposal.reason = by_hash.get(proposal.sentence_hash.as_str()).copied();
            proposal.reason.is_some()
        });
    }
    Ok((plan, !owned.is_empty()))
}

/// Under the page lock, would this machine write only change sentences the
/// page already has? Then every change is a sentence proposal (or dropped as
/// a rewording) and nothing lands. A write that adds sentences is not, and
/// still waits as a whole-page revision card.
pub async fn locked_write_only_changes_sentences(
    db: &MemoryDB,
    page: &Page,
    new_content: &str,
) -> Result<bool, WenlanError> {
    let (plan, _) = sentence_gate(db, page, new_content).await?;
    let landing = plan
        .as_ref()
        .map_or(new_content, |plan| plan.content.as_str());
    let norms = |body: &str| -> Vec<String> {
        authorship::sentence_units(body)
            .into_iter()
            .map(|unit| unit.norm)
            .collect()
    };
    Ok(norms(landing) == norms(&page.content))
}

/// The facts-changed reason of a sentence: the strongest of its citation
/// problems. A missing citation is not one; nothing about the facts changed.
fn facts_changed(weak: &[WeakReason]) -> Option<WeakReason> {
    [
        WeakReason::Disproven,
        WeakReason::Unsupported,
        WeakReason::Stale,
    ]
    .into_iter()
    .find(|reason| weak.contains(reason))
}

/// Each unit's citations, in unit order.
async fn unit_citations(
    db: &MemoryDB,
    page: &Page,
    units: &[SentenceUnit],
) -> Result<Vec<Vec<SentenceCitation>>, WenlanError> {
    let states = source_states(db, &page.citations).await?;
    let by_occurrence: HashMap<u32, &PageCitation> =
        page.citations.iter().map(|c| (c.occurrence, c)).collect();
    let markers = occurrence_units(&page.content, units);
    Ok((0..units.len())
        .map(|idx| {
            markers
                .iter()
                .enumerate()
                .filter(|(_, at)| **at == Some(idx))
                .map(|(k, _)| {
                    let occurrence = u32::try_from(k + 1).unwrap_or(u32::MAX);
                    sentence_citation(
                        occurrence,
                        &page.content,
                        by_occurrence.get(&occurrence).copied(),
                        &states,
                    )
                })
                .collect()
        })
        .collect())
}

/// Each unit's facts-changed reason, in unit order.
async fn unit_reasons(
    db: &MemoryDB,
    page: &Page,
    units: &[SentenceUnit],
) -> Result<Vec<Option<WeakReason>>, WenlanError> {
    Ok(unit_citations(db, page, units)
        .await?
        .iter()
        .map(|citations| facts_changed(&weak_reasons(citations, false)))
        .collect())
}

/// The proposals of a page, oldest first, with their state.
fn proposals_from(entries: &[HistoryEntry], page_id: &str) -> Vec<SentenceProposal> {
    let mut decided: HashMap<&str, SentenceProposalStatus> = HashMap::new();
    for entry in entries {
        let status = match entry.op.as_str() {
            OP_SENTENCE_ACCEPT => SentenceProposalStatus::Accepted,
            OP_SENTENCE_REJECT => SentenceProposalStatus::Rejected,
            _ => continue,
        };
        if let Some(id) = str_field(entry, "proposal_id") {
            decided.insert(id, status);
        }
    }
    entries
        .iter()
        .filter(|entry| entry.op == OP_SENTENCE_PROPOSE)
        .filter_map(|entry| {
            let id = str_field(entry, "proposal_id")?;
            Some(SentenceProposal {
                id: id.to_string(),
                page_id: page_id.to_string(),
                version: entry.version.unwrap_or(0),
                sentence_hash: str_field(entry, "sentence_hash")?.to_string(),
                before: str_field(entry, "before").unwrap_or_default().to_string(),
                after: entry.text.clone().unwrap_or_default(),
                why: entry.why.clone(),
                reason: entry
                    .data
                    .get("reason")
                    .and_then(|value| serde_json::from_value(value.clone()).ok()),
                actor: entry.actor.clone(),
                ts: entry.ts,
                status: decided
                    .get(id)
                    .copied()
                    .unwrap_or(SentenceProposalStatus::Pending),
            })
        })
        .collect()
}

/// Stage the proposals of a gate plan against the version they now apply to:
/// the kept sentences are unchanged in it. A pending proposal with the same
/// sentence and text is not staged twice. Returns the proposal ids.
pub(crate) async fn stage_gate_proposals(
    db: &MemoryDB,
    page_id: &str,
    version: i64,
    plan: &GatePlan,
    edited_by: &str,
    agent: Option<&str>,
) -> Result<Vec<String>, WenlanError> {
    let existing = proposals_from(&db.list_sentence_history("page", page_id).await?, page_id);
    let pending: HashSet<(&str, &str)> = existing
        .iter()
        .filter(|p| p.status == SentenceProposalStatus::Pending)
        .map(|p| (p.sentence_hash.as_str(), p.after.as_str()))
        .collect();
    let actor = HistoryActor {
        kind: ActorKind::Ai,
        name: edited_by.to_string(),
        agent: agent.map(str::to_string),
        session: None,
    };
    let tx = format!("tx_{}", uuid::Uuid::new_v4().simple());
    let mut ids = Vec::new();
    for proposal in &plan.proposals {
        if pending.contains(&(proposal.sentence_hash.as_str(), proposal.after.as_str())) {
            continue;
        }
        let id = format!("sp_{}", uuid::Uuid::new_v4().simple());
        append_proposal(
            db,
            page_id,
            version,
            &tx,
            &actor,
            &id,
            &proposal.sentence_hash,
            &proposal.before,
            &proposal.after,
            Some("the facts behind a sentence that is yours changed, so it is a proposal"),
            proposal.reason,
        )
        .await?;
        ids.push(id);
    }
    Ok(ids)
}

#[allow(clippy::too_many_arguments)]
async fn append_proposal(
    db: &MemoryDB,
    page_id: &str,
    version: i64,
    tx: &str,
    actor: &HistoryActor,
    id: &str,
    sentence_hash: &str,
    before: &str,
    after: &str,
    why: Option<&str>,
    reason: Option<WeakReason>,
) -> Result<i64, WenlanError> {
    let mut data = serde_json::json!({
        "proposal_id": id,
        "sentence_hash": sentence_hash,
        "before": before,
    });
    if let Some(reason) = reason {
        data["reason"] = serde_json::to_value(reason)?;
    }
    let after_hash = (!after.is_empty())
        .then(|| authorship::sentence_hash(&authorship::normalize_sentence(after)));
    db.append_sentence_history(NewHistoryEntry {
        ts: chrono::Utc::now().timestamp(),
        tx,
        actor,
        op: OP_SENTENCE_PROPOSE,
        target: &page_target(page_id),
        version: Some(version),
        why,
        text: Some(after),
        digest_before: Some(sentence_hash),
        digest_after: after_hash.as_deref(),
        data: &data,
    })
    .await
}

async fn load_page(db: &MemoryDB, page_id: &str) -> Result<Page, WenlanError> {
    db.get_page(page_id)
        .await?
        .ok_or_else(|| WenlanError::NotFound(format!("page {page_id} not found")))
}

fn unit_by_hash<'a>(
    units: &'a [SentenceUnit],
    hash: &str,
) -> Result<&'a SentenceUnit, WenlanError> {
    units.iter().find(|u| u.hash == hash).ok_or_else(|| {
        WenlanError::Conflict(format!(
            "sentence_changed: the page no longer has sentence {hash}"
        ))
    })
}

/// An AI's proposal for one sentence, made by name instead of by rewriting
/// the page. `expected_version`, when sent, must be the page's version.
pub async fn propose_sentence(
    db: &MemoryDB,
    page_id: &str,
    req: &wenlan_types::authorship::SentenceProposeRequest,
    agent: &str,
) -> Result<SentenceProposal, WenlanError> {
    let page = load_page(db, page_id).await?;
    if let Some(expected) = req.expected_version {
        if expected != page.version {
            return Err(WenlanError::Conflict(format!(
                "page moved to v{} (expected v{expected}); proposal refused",
                page.version
            )));
        }
    }
    let units = authorship::sentence_units(&page.content);
    let unit = unit_by_hash(&units, &req.sentence_hash)?;
    let after = req.text.trim();
    if authorship::normalize_sentence(after) == unit.norm {
        return Err(WenlanError::Validation(
            "the proposed text is the sentence as it already reads".into(),
        ));
    }
    // A protected sentence (the person's, imported, or any on a locked page)
    // gets a proposal only when its facts changed, and the reason must be
    // one it has now.
    let idx = units
        .iter()
        .position(|u| u.hash == unit.hash)
        .expect("unit came from units");
    let rows = page_authorship(db, &page).await?;
    let protected = page.review_required
        || rows
            .get(idx)
            .is_some_and(|row| authorship::is_protected(row.author, guarded(&page)));
    if protected {
        let citations = unit_citations(db, &page, &units).await?;
        let weak = weak_reasons(&citations[idx], false);
        let Some(given) = req.reason else {
            return Err(WenlanError::Validation(
                "reason_required: this sentence is protected; propose a change only when \
                 its facts changed (reason unsupported, stale or disproven)"
                    .into(),
            ));
        };
        if facts_changed(&[given]).is_none() || !weak.contains(&given) {
            return Err(WenlanError::Validation(format!(
                "reason_not_current: {} is not a facts-changed reason this sentence has now \
                 (it has {weak:?})",
                serde_json::to_string(&given)?
            )));
        }
    }
    let actor = HistoryActor {
        kind: ActorKind::Ai,
        name: agent.to_string(),
        agent: Some(agent.to_string()),
        session: None,
    };
    let id = format!("sp_{}", uuid::Uuid::new_v4().simple());
    let tx = format!("tx_{}", uuid::Uuid::new_v4().simple());
    append_proposal(
        db,
        page_id,
        page.version,
        &tx,
        &actor,
        &id,
        &unit.hash,
        &unit.text,
        after,
        req.why.as_deref(),
        req.reason,
    )
    .await?;
    find_proposal(db, page_id, &id).await
}

async fn find_proposal(
    db: &MemoryDB,
    page_id: &str,
    proposal_id: &str,
) -> Result<SentenceProposal, WenlanError> {
    proposals_from(&db.list_sentence_history("page", page_id).await?, page_id)
        .into_iter()
        .find(|p| p.id == proposal_id)
        .ok_or_else(|| WenlanError::NotFound(format!("sentence proposal {proposal_id} not found")))
}

async fn pending_proposal(
    db: &MemoryDB,
    page_id: &str,
    proposal_id: &str,
) -> Result<SentenceProposal, WenlanError> {
    let proposal = find_proposal(db, page_id, proposal_id).await?;
    if proposal.status != SentenceProposalStatus::Pending {
        return Err(WenlanError::Conflict(format!(
            "sentence proposal {proposal_id} was already decided"
        )));
    }
    Ok(proposal)
}

/// The body with the unit replaced by `after`, or removed with the space
/// after it when `after` is empty.
fn body_with(content: &str, unit: &SentenceUnit, after: &str) -> String {
    let mut body = content.to_string();
    if after.is_empty() {
        let rest = &content[unit.end..];
        let trailing = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        let end = if unit.whole_line {
            content[unit.end..]
                .find('\n')
                .map_or(content.len(), |i| unit.end + i + 1)
        } else {
            unit.end + trailing
        };
        let start = if unit.whole_line {
            content[..unit.start].rfind('\n').map_or(0, |i| i + 1)
        } else {
            unit.start
        };
        body.replace_range(start..end, "");
    } else {
        body.replace_range(unit.start..unit.end, after);
    }
    body
}

/// The person accepts one proposal: the proposed text lands as their own
/// edit (one new version, written like a save in the app), and a
/// `sentence.accept` entry records which sentence they took from the AI.
pub async fn accept_sentence_proposal(
    db: &MemoryDB,
    page_id: &str,
    proposal_id: &str,
    why: Option<&str>,
    knowledge_path: Option<&std::path::Path>,
) -> Result<SentenceAcceptResponse, WenlanError> {
    let proposal = pending_proposal(db, page_id, proposal_id).await?;
    let page = load_page(db, page_id).await?;
    let units = authorship::sentence_units(&page.content);
    let unit = unit_by_hash(&units, &proposal.sentence_hash)?;
    let content = body_with(&page.content, unit, &proposal.after);
    if content.trim().is_empty() {
        return Err(WenlanError::Validation(
            "accepting this proposal would leave the page empty".into(),
        ));
    }
    let result = crate::post_write::page_write(
        db,
        crate::post_write::PageWrite::UpdatePreservingSources {
            page_id,
            req: wenlan_types::requests::UpdatePageRequest {
                content,
                source_memory_ids: vec![],
                expected_version: Some(page.version),
                caller_id: None,
                operation_id: None,
            },
            edited_by: "manual_edit",
            knowledge_path,
            // Accepting one sentence protects that sentence (it is the
            // person's from now on); it does not lock the whole page.
            basis: crate::db::BasisWrite {
                actor: "human".to_string(),
                ..crate::db::BasisWrite::default()
            },
        },
    )
    .await?;
    if result.outcome != crate::post_write::WriteOutcome::Wrote {
        return Err(WenlanError::Conflict(format!(
            "sentence proposal {proposal_id} was not applied: {}",
            result.warnings.join("; ")
        )));
    }
    let version = load_page(db, page_id).await?.version;
    let accepted_hash = (!proposal.after.is_empty())
        .then(|| authorship::sentence_hash(&authorship::normalize_sentence(&proposal.after)));
    let data = serde_json::json!({
        "proposal_id": proposal.id,
        "sentence_hash": proposal.sentence_hash,
        "accepted_hash": accepted_hash,
    });
    db.append_sentence_history(NewHistoryEntry {
        ts: chrono::Utc::now().timestamp(),
        tx: &format!("tx_{}", uuid::Uuid::new_v4().simple()),
        actor: &person(),
        op: OP_SENTENCE_ACCEPT,
        target: &page_target(page_id),
        version: Some(version),
        why,
        text: Some(&proposal.after),
        digest_before: Some(&proposal.sentence_hash),
        digest_after: accepted_hash.as_deref(),
        data: &data,
    })
    .await?;
    Ok(SentenceAcceptResponse {
        proposal: find_proposal(db, page_id, proposal_id).await?,
        version,
    })
}

/// The person rejects one proposal. The page is not touched.
pub async fn reject_sentence_proposal(
    db: &MemoryDB,
    page_id: &str,
    proposal_id: &str,
    why: Option<&str>,
) -> Result<SentenceProposal, WenlanError> {
    let proposal = pending_proposal(db, page_id, proposal_id).await?;
    let page = load_page(db, page_id).await?;
    let data = serde_json::json!({
        "proposal_id": proposal.id,
        "sentence_hash": proposal.sentence_hash,
    });
    db.append_sentence_history(NewHistoryEntry {
        ts: chrono::Utc::now().timestamp(),
        tx: &format!("tx_{}", uuid::Uuid::new_v4().simple()),
        actor: &person(),
        op: OP_SENTENCE_REJECT,
        target: &page_target(page_id),
        version: Some(page.version),
        why,
        text: None,
        digest_before: Some(&proposal.sentence_hash),
        digest_after: None,
        data: &data,
    })
    .await?;
    find_proposal(db, page_id, proposal_id).await
}

/// The person's 「不對」 on one sentence. It stays flagged until the sentence
/// changes; a fix the AI makes for it is a proposal like any other.
pub async fn flag_sentence(
    db: &MemoryDB,
    page_id: &str,
    sentence_hash: &str,
    why: Option<&str>,
) -> Result<SentenceFlagResponse, WenlanError> {
    let page = load_page(db, page_id).await?;
    let units = authorship::sentence_units(&page.content);
    let unit = unit_by_hash(&units, sentence_hash)?;
    let data = serde_json::json!({
        "sentence_hash": unit.hash,
        "sentence": unit.text,
    });
    let seq = db
        .append_sentence_history(NewHistoryEntry {
            ts: chrono::Utc::now().timestamp(),
            tx: &format!("tx_{}", uuid::Uuid::new_v4().simple()),
            actor: &person(),
            op: OP_SENTENCE_FLAG,
            target: &page_target(page_id),
            version: Some(page.version),
            why,
            text: None,
            digest_before: Some(&unit.hash),
            digest_after: None,
            data: &data,
        })
        .await?;
    Ok(SentenceFlagResponse {
        page_id: page_id.to_string(),
        version: page.version,
        sentence_hash: unit.hash.clone(),
        seq,
    })
}

/// Citation occurrence (1-based) → the index of the unit its marker sits in.
fn occurrence_units(content: &str, units: &[SentenceUnit]) -> Vec<Option<usize>> {
    crate::citations::marker_spans(content)
        .into_iter()
        .map(|(start, _)| units.iter().position(|u| u.start <= start && start < u.end))
        .collect()
}

/// A cited source's current version and text digest, per locator.
async fn source_states(
    db: &MemoryDB,
    citations: &[PageCitation],
) -> Result<HashMap<String, Option<(i64, String, String)>>, WenlanError> {
    let mut states = HashMap::new();
    for citation in citations {
        if states.contains_key(&citation.locator) {
            continue;
        }
        let state = db
            .cited_source_state(&citation.locator)
            .await?
            .map(|(version, text)| (version, cited_text_digest(&text), text));
        states.insert(citation.locator.clone(), state);
    }
    Ok(states)
}

fn is_stale(citation: &PageCitation, current: Option<&(i64, String, String)>) -> bool {
    match (
        citation.cited_version,
        citation.cited_digest.as_deref(),
        current,
    ) {
        // Only a changed text makes a citation stale: a newer version that
        // says the same still supports it (same rule as the basis check).
        (Some(cited), Some(digest), Some((version, now, _))) => *version > cited && digest != now,
        _ => false,
    }
}

fn needs_citation(row: &SentenceAuthorship, unit: &SentenceUnit, guard: bool) -> bool {
    if unit.text.starts_with('#') {
        return false;
    }
    match row.origin {
        SentenceAuthor::Ai => true,
        SentenceAuthor::Unknown => !guard,
        SentenceAuthor::Person | SentenceAuthor::Original => false,
    }
}

/// The "who said it" view of a page.
pub async fn page_sentences(
    db: &MemoryDB,
    page_id: &str,
) -> Result<PageSentencesResponse, WenlanError> {
    let page = load_page(db, page_id).await?;
    let mode = ai_edit_mode(&page);
    let rows = page_authorship(db, &page).await?;
    let units = authorship::sentence_units(&page.content);
    let entries = db.list_sentence_history("page", page_id).await?;
    let present: HashSet<&str> = units.iter().map(|u| u.hash.as_str()).collect();
    let pending: Vec<SentenceProposal> = proposals_from(&entries, page_id)
        .into_iter()
        .filter(|p| p.status == SentenceProposalStatus::Pending)
        .filter(|p| present.contains(p.sentence_hash.as_str()))
        .collect();
    let flags: Vec<(String, i64)> = entries
        .iter()
        .filter(|e| e.op == OP_SENTENCE_FLAG)
        .filter_map(|e| Some((str_field(e, "sentence_hash")?.to_string(), e.version?)))
        .collect();
    let states = source_states(db, &page.citations).await?;
    let by_occurrence: HashMap<u32, &PageCitation> =
        page.citations.iter().map(|c| (c.occurrence, c)).collect();
    let markers = occurrence_units(&page.content, &units);
    let guard = guarded(&page);
    let sentences = units
        .iter()
        .zip(&rows)
        .enumerate()
        .map(|(idx, (unit, row))| {
            let citations: Vec<SentenceCitation> = markers
                .iter()
                .enumerate()
                .filter(|(_, at)| **at == Some(idx))
                .map(|(k, _)| {
                    let occurrence = u32::try_from(k + 1).unwrap_or(u32::MAX);
                    sentence_citation(
                        occurrence,
                        &page.content,
                        by_occurrence.get(&occurrence).copied(),
                        &states,
                    )
                })
                .collect();
            let flagged = flags
                .iter()
                .any(|(hash, version)| hash == &unit.hash && *version >= row.since_version);
            PageSentence {
                idx: row.idx,
                text: unit.text.clone(),
                hash: unit.hash.clone(),
                author: row.author,
                origin: row.origin,
                since_version: row.since_version,
                confidence: row.confidence,
                accepted: row.accepted,
                protected: authorship::is_protected(row.author, guard),
                evidence: evidence_of(&citations),
                weak: weak_reasons(&citations, needs_citation(row, unit, guard)),
                citations,
                flagged,
                proposals: pending
                    .iter()
                    .filter(|p| p.sentence_hash == unit.hash)
                    .map(|p| p.id.clone())
                    .collect(),
            }
        })
        .collect();
    Ok(PageSentencesResponse {
        page_id: page.id,
        version: page.version,
        ai_edits: mode,
        sentences,
        proposals: pending,
    })
}

fn sentence_citation(
    occurrence: u32,
    content: &str,
    record: Option<&PageCitation>,
    states: &HashMap<String, Option<(i64, String, String)>>,
) -> SentenceCitation {
    match record {
        Some(record) => {
            let current = states.get(&record.locator).and_then(Option::as_ref);
            SentenceCitation {
                occurrence,
                marker: record.marker,
                source_kind: record.source_kind.clone(),
                locator: record.locator.clone(),
                status: record.status.clone(),
                cited_version: record.cited_version,
                current_version: current.map(|(version, _, _)| *version),
                stale: is_stale(record, current),
                disproven: record.disproven,
            }
        }
        // A marker the page has no stored check for: never shown as verified.
        None => {
            let marker = crate::citations::marker_spans(content)
                .get(occurrence as usize - 1)
                .and_then(|(start, end)| content[start + 1..end - 1].parse().ok())
                .unwrap_or(0);
            SentenceCitation {
                occurrence,
                marker,
                source_kind: String::new(),
                locator: String::new(),
                status: "unevaluated".to_string(),
                cited_version: None,
                current_version: None,
                stale: false,
                disproven: false,
            }
        }
    }
}

fn evidence_of(citations: &[SentenceCitation]) -> EvidenceState {
    if citations.is_empty() {
        return EvidenceState::None;
    }
    if citations
        .iter()
        .any(|c| c.status == "unverified" || c.disproven)
    {
        return EvidenceState::Unverified;
    }
    if citations.iter().all(|c| c.status == "verified" && !c.stale) {
        return EvidenceState::Verified;
    }
    EvidenceState::Unevaluated
}

fn weak_reasons(citations: &[SentenceCitation], needs_citation: bool) -> Vec<WeakReason> {
    let mut weak = Vec::new();
    if citations.is_empty() {
        if needs_citation {
            weak.push(WeakReason::NoCitation);
        }
        return weak;
    }
    if citations
        .iter()
        .any(|c| c.status == "unverified" && !c.disproven)
    {
        weak.push(WeakReason::Unsupported);
    }
    if citations.iter().any(|c| c.stale) {
        weak.push(WeakReason::Stale);
    }
    if citations.iter().any(|c| c.disproven) {
        weak.push(WeakReason::Disproven);
    }
    weak
}

/// The paragraph (blank-line block) of `content` around byte `at`.
fn paragraph_at(content: &str, at: usize) -> &str {
    let start = content[..at].rfind("\n\n").map_or(0, |i| i + 2);
    let end = content[at..].find("\n\n").map_or(content.len(), |i| at + i);
    &content[start..end]
}

/// Recheck one sentence's citations against their sources' current text,
/// and store the result on those citations only. A citation that was
/// verified and no longer finds support is marked disproven; one that never
/// had support stays unsupported. Every rechecked citation is restamped with
/// the version it was checked against.
pub async fn recheck_sentence(
    db: &MemoryDB,
    page_id: &str,
    sentence_hash: &str,
) -> Result<SentenceRecheckResponse, WenlanError> {
    let page = load_page(db, page_id).await?;
    let units = authorship::sentence_units(&page.content);
    let unit_index = units
        .iter()
        .position(|u| u.hash == sentence_hash)
        .ok_or_else(|| {
            WenlanError::Conflict(format!(
                "sentence_changed: the page no longer has sentence {sentence_hash}"
            ))
        })?;
    let unit = &units[unit_index];
    let markers = occurrence_units(&page.content, &units);
    let occurrences: HashSet<u32> = markers
        .iter()
        .enumerate()
        .filter(|(_, at)| **at == Some(unit_index))
        .map(|(k, _)| u32::try_from(k + 1).unwrap_or(u32::MAX))
        .collect();
    let mut citations = page.citations.clone();
    let targets: Vec<usize> = citations
        .iter()
        .enumerate()
        .filter(|(_, c)| occurrences.contains(&c.occurrence))
        .map(|(i, _)| i)
        .collect();
    if !targets.is_empty() {
        let states = source_states(db, &citations).await?;
        let texts: Vec<&str> = targets
            .iter()
            .map(|&i| {
                states
                    .get(&citations[i].locator)
                    .and_then(Option::as_ref)
                    .map_or("", |(_, _, text)| text.as_str())
            })
            .collect();
        let sentence = crate::citations::strip_markers(&unit.text);
        let paragraph = crate::citations::strip_markers(paragraph_at(&page.content, unit.start));
        let (verified, scope, scores) =
            crate::citations::claim_support(&sentence, &paragraph, &texts);
        for (k, &i) in targets.iter().enumerate() {
            let state = states.get(&citations[i].locator).and_then(Option::as_ref);
            let citation = &mut citations[i];
            let was_verified = citation.status == "verified" && !citation.disproven;
            citation.disproven = !verified && was_verified;
            citation.status = if verified { "verified" } else { "unverified" }.to_string();
            citation.scope = scope.to_string();
            citation.score = scores.get(k).copied().unwrap_or(0.0);
            citation.cited_version = state.map(|(version, _, _)| *version);
            citation.cited_digest = state.map(|(_, digest, _)| digest.clone());
        }
        let json = serde_json::to_string(&citations)?;
        if !db
            .set_page_citations_at_page_version(page_id, &json, page.version)
            .await?
        {
            return Err(WenlanError::Conflict(format!(
                "page {page_id} changed during the recheck; read it again"
            )));
        }
    }
    let view = page_sentences(db, page_id).await?;
    let sentence = view
        .sentences
        .into_iter()
        .find(|s| s.hash == sentence_hash)
        .ok_or_else(|| {
            WenlanError::Conflict(format!(
                "sentence_changed: the page no longer has sentence {sentence_hash}"
            ))
        })?;
    Ok(SentenceRecheckResponse {
        page_id: page_id.to_string(),
        version: view.version,
        sentence,
    })
}

#[cfg(test)]
#[path = "sentences_test.rs"]
mod tests;
