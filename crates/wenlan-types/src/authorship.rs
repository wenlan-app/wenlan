// SPDX-License-Identifier: Apache-2.0
//! Wire types for who wrote each sentence of a page, the per-sentence
//! proposal ledger, and how well each sentence is backed.
//!
//! Shapes follow the storage migration spec (2026-10-31), sections 3 and 4:
//!
//! - [`SentenceAuthorship`] is one row of the derived `authorship(page_id,
//!   version, idx, hash, author, since_version, confidence)` cache, plus the
//!   evidentiary `origin`, which is a separate column on purpose: a person
//!   touching up an AI sentence makes it theirs but keeps its citation
//!   requirement.
//! - [`HistoryEntry`] is the exact shape of one line of the future
//!   `history/YYYY-MM/*.jsonl`. Sentence decisions (`sentence.propose`,
//!   `sentence.accept`, `sentence.reject`, `sentence.flag`) are stored in this
//!   shape today, so the phase B export copies them without translation.

use serde::{Deserialize, Serialize};

/// The `op` of a history entry that stages an AI's change to one sentence.
pub const OP_SENTENCE_PROPOSE: &str = "sentence.propose";
/// The `op` of a history entry that records the person accepting a proposal.
pub const OP_SENTENCE_ACCEPT: &str = "sentence.accept";
/// The `op` of a history entry that records the person rejecting a proposal.
pub const OP_SENTENCE_REJECT: &str = "sentence.reject";
/// The `op` of a history entry that records the person's 「不對」 on a sentence.
pub const OP_SENTENCE_FLAG: &str = "sentence.flag";
/// The person undid one AI write's sentence changes. The page version it
/// landed in restored `data.restored_hashes`, which keep the author they had
/// before that AI write.
pub const OP_SENTENCE_REVERT: &str = "sentence.revert";

/// Who wrote a sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SentenceAuthor {
    /// The person wrote it, edited it, or accepted it.
    Person,
    /// An AI wrote it and nobody has touched it since.
    Ai,
    /// Nobody can say: the page predates per-version history (the
    /// `migration_87` seed) or the writer is not recognised. Protected like the
    /// person's on a page that is human-owned or review-locked.
    Unknown,
    /// Imported text from another tool (an AI chat's own words). Never the
    /// person's, never rewritten by the AI.
    Original,
}

/// How sure the derivation is about a sentence's author.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorshipConfidence {
    /// Derived from versions whose writer is known.
    Certain,
    /// The sentence came from a version whose writer is unknown.
    Uncertain,
}

/// One row of a page version's derived authorship (spec section 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentenceAuthorship {
    pub page_id: String,
    pub version: i64,
    /// Position of the sentence in the page body, from 0.
    pub idx: u32,
    /// Hash of the normalized sentence (citation markers removed, whitespace
    /// collapsed). Proposals, flags and rechecks address a sentence by it.
    pub hash: String,
    pub author: SentenceAuthor,
    /// The version that last changed this sentence's text.
    pub since_version: i64,
    pub confidence: AuthorshipConfidence,
    /// Who first introduced what this sentence says. Differs from `author`
    /// when the person touched up an AI sentence: the sentence is theirs, but
    /// it still needs the citation an AI sentence needs.
    pub origin: SentenceAuthor,
    /// The person accepted this sentence from an AI proposal (「你點頭的」).
    #[serde(default, skip_serializing_if = "is_false")]
    pub accepted: bool,
}

/// What an AI may change on a page. One rule for every page, plus the page's
/// lock (`review_required`); there is no "edit directly" mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiEditMode {
    /// The default: the AI updates the sentences it wrote that the person
    /// has not touched (recorded, revertible); a change to a sentence the
    /// person wrote, edited or accepted is staged as a sentence proposal, and
    /// only when the facts changed (see [`SentenceProposal::reason`]).
    #[default]
    Propose,
    /// The page is locked: every sentence, the AI's included, is protected
    /// like the person's, so an AI change to one is a sentence proposal (on
    /// the same facts-changed rule). A write that would add new sentences
    /// still waits as a whole-page revision card.
    Locked,
}

/// Why a sentence is weakly backed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeakReason {
    /// An AI sentence with no citation.
    NoCitation,
    /// A citation was checked against its source and does not support it.
    Unsupported,
    /// A citation points at an older version of its source whose text has
    /// changed since. Not disproven: the evidence needs a recheck.
    Stale,
    /// A recheck against the source's current version found no support for a
    /// citation that had support at the version it cited.
    Disproven,
}

/// The best a sentence's citations say about it. Never `Verified` unless a
/// check said so: an unchecked citation is `Unevaluated`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    Verified,
    Unverified,
    Unevaluated,
    /// The sentence carries no citation.
    None,
}

/// One citation on a sentence, with the version it cited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SentenceCitation {
    pub occurrence: u32,
    pub marker: u32,
    pub source_kind: String,
    pub locator: String,
    /// `verified`, `unverified`, or `unevaluated` when the marker has no
    /// stored check.
    pub status: String,
    /// The source version this citation was made against (`locator@version`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cited_version: Option<i64>,
    /// The source's version now, when it still exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_version: Option<i64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub stale: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub disproven: bool,
}

/// One sentence of a page as the "who said it" view needs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageSentence {
    pub idx: u32,
    pub text: String,
    pub hash: String,
    pub author: SentenceAuthor,
    pub origin: SentenceAuthor,
    pub since_version: i64,
    pub confidence: AuthorshipConfidence,
    #[serde(default, skip_serializing_if = "is_false")]
    pub accepted: bool,
    /// A machine write may not change this sentence without a proposal.
    pub protected: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub citations: Vec<SentenceCitation>,
    pub evidence: EvidenceState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weak: Vec<WeakReason>,
    /// The person said 「不對」 about this sentence and it has not changed since.
    #[serde(default, skip_serializing_if = "is_false")]
    pub flagged: bool,
    /// Pending proposals for this sentence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<String>,
}

/// Where a sentence proposal stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SentenceProposalStatus {
    Pending,
    Accepted,
    Rejected,
}

/// A staged change to one sentence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SentenceProposal {
    pub id: String,
    pub page_id: String,
    /// The page version the proposal was made against.
    pub version: i64,
    pub sentence_hash: String,
    /// The sentence as it was when proposed.
    pub before: String,
    /// The proposed text; empty proposes deleting the sentence.
    pub after: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Why the facts behind the sentence changed: `unsupported` (its source
    /// does not back it), `stale` (its source changed since it was cited) or
    /// `disproven`. A protected sentence only gets a proposal with one; a
    /// rewording alone never does. `None` on proposals made before the rule,
    /// and on a proposal for a sentence that is not protected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<WeakReason>,
    pub actor: HistoryActor,
    pub ts: i64,
    pub status: SentenceProposalStatus,
}

/// `GET /api/pages/{id}/sentences`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageSentencesResponse {
    pub page_id: String,
    pub version: i64,
    pub ai_edits: AiEditMode,
    pub sentences: Vec<PageSentence>,
    /// Every pending proposal on the page.
    #[serde(default)]
    pub proposals: Vec<SentenceProposal>,
    /// The page's newest AI write in its history, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ai_update: Option<LastAiUpdate>,
}

/// `POST /api/pages/{id}/sentences/propose`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SentenceProposeRequest {
    pub sentence_hash: String,
    /// The proposed sentence; empty proposes deleting it.
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Required when the sentence is protected (the person's, imported, or
    /// any sentence on a locked page): one of the facts-changed reasons the
    /// sentence has right now (`unsupported`, `stale`, `disproven`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<WeakReason>,
    /// The page version the caller read. Refused when the page moved on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

/// `POST /api/pages/{id}/sentences/proposals/{proposal_id}/accept|reject`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SentenceDecisionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// `POST /api/pages/{id}/sentences/flag`: the person's 「不對」.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SentenceFlagRequest {
    pub sentence_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// `POST /api/pages/{id}/sentences/revert`: undo the sentence changes of one
/// AI write.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SentenceRevertRequest {
    /// The page version the AI write produced.
    pub version: i64,
}

/// Answer to a revert. Re-read the page for its text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SentenceRevertResponse {
    pub page_id: String,
    /// The page's version after the revert; unchanged when nothing was put
    /// back.
    pub version: i64,
    /// Sentences put back as they were before that AI write.
    pub reverted: u32,
    /// Sentences that write changed but that changed again since, so they
    /// were left as they are.
    pub skipped: u32,
}

/// The page's newest AI write, for "updated N sentences" and its Undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastAiUpdate {
    /// The page version the AI write produced; what the revert takes.
    pub version: i64,
    /// Sentences that write added, changed or removed.
    pub changed_sentences: u32,
    /// Unix seconds.
    pub at: i64,
}

/// `POST /api/pages/{id}/sentences/recheck`: recheck one sentence only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SentenceRecheckRequest {
    pub sentence_hash: String,
}

/// Answer to a propose, reject or flag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SentenceProposalResponse {
    pub proposal: SentenceProposal,
}

/// Answer to an accept: the page version the accepted text landed in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SentenceAcceptResponse {
    pub proposal: SentenceProposal,
    pub version: i64,
}

/// Answer to a flag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SentenceFlagResponse {
    pub page_id: String,
    pub version: i64,
    pub sentence_hash: String,
    pub seq: i64,
}

/// Answer to a one-sentence recheck.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SentenceRecheckResponse {
    pub page_id: String,
    pub version: i64,
    pub sentence: PageSentence,
}

/// `actor.kind` of a history entry (spec section 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    Person,
    Ai,
    Original,
    Import,
    System,
}

/// `actor` of a history entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryActor {
    pub kind: ActorKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// `target` of a history entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryTarget {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
}

/// One history entry in the spec section 4 shape. `receipt` and `crc` are
/// added by the phase B writer; nothing in phase A produces them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub seq: i64,
    pub ts: i64,
    pub tx: String,
    pub actor: HistoryActor,
    pub op: String,
    pub target: HistoryTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest_after: Option<String>,
    #[serde(default)]
    pub data: serde_json::Value,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_entry_serializes_in_the_spec_shape() {
        let entry = HistoryEntry {
            seq: 7,
            ts: 1_700_000_000,
            tx: "tx_1".into(),
            actor: HistoryActor {
                kind: ActorKind::Ai,
                name: "distill".into(),
                agent: Some("claude-code".into()),
                session: None,
            },
            op: OP_SENTENCE_PROPOSE.into(),
            target: HistoryTarget {
                kind: "page".into(),
                id: "page_a".into(),
            },
            version: Some(3),
            why: None,
            text: Some("New text.".into()),
            digest_before: None,
            digest_after: None,
            data: serde_json::json!({"proposal_id": "sp_1"}),
        };
        let value = serde_json::to_value(&entry).unwrap();
        assert_eq!(value["target"]["type"], "page");
        assert_eq!(value["actor"]["kind"], "ai");
        assert_eq!(value["op"], "sentence.propose");
        assert!(value.get("why").is_none());
        let back: HistoryEntry = serde_json::from_value(value).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn ai_edit_mode_defaults_to_propose() {
        assert_eq!(AiEditMode::default(), AiEditMode::Propose);
        assert_eq!(serde_json::to_value(AiEditMode::Locked).unwrap(), "locked");
    }
}
