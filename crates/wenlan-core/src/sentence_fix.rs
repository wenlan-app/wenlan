// SPDX-License-Identifier: Apache-2.0
//! The AI's fix for a sentence the person flagged (UX-WHO-3: 「不對」, then
//! "ask the AI"). The AI's fix is still a proposal.
//!
//! One call to the everyday model asks for a corrected version of just that
//! sentence, grounded in the sources the sentence cites and in what the
//! person said was wrong (the flag's `why`); the paragraph around it is
//! context, not evidence. The answer is one of three:
//!
//! - `rewrite`: the corrected sentence, filed as a pending proposal that keeps
//!   the sentence's citation markers and list or heading prefix;
//! - `delete`: the sources contradict the sentence and nothing in them can
//!   replace it, filed as a proposal with empty text (the existing "propose
//!   deleting it" form);
//! - `none`: the sources do not show what is wrong or how to fix it. Nothing
//!   is filed and the flag stays as it is.
//!
//! The proposal goes through the same propose path as an agent's, so it lands
//! only when the person accepts it, on any page, locked or not. A protected
//! sentence (the person's, imported, or any on a locked page) may receive it
//! without a facts-changed reason only while the person's flag stands; see
//! `ProposalGrant::PersonFlagged`.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use regex::Regex;
use serde::Deserialize;
use wenlan_types::authorship::{
    ActorKind, HistoryActor, SentenceProposalStatus, SentenceProposeRequest,
};

use super::{
    flagged_since, load_page, occurrence_units, page_authorship, paragraph_at, proposals_from,
    propose_as, source_states, ProposalGrant,
};
use crate::authorship;
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::llm_provider::{extract_json, strip_think_tags, LlmProvider, LlmRequest};
use crate::refinery::{resolve_everyday, EverydaySource};

/// The proposal's `why` when the person gave none with the flag.
pub const DEFAULT_FIX_WHY: &str =
    "you marked this sentence as not right and asked the AI to fix it";

/// `actor.name` of the AI fix's proposal.
const FIX_ACTOR: &str = "wenlan";

/// At most this many characters of each cited source reach the model.
const SOURCE_CHARS: usize = 4_000;
/// At most this many characters of the paragraph around the sentence.
const CONTEXT_CHARS: usize = 1_500;

const SYSTEM_PROMPT: &str = "You fix one sentence of a note. The person marked it as not right. \
Use only facts stated in SOURCES or in PERSON'S NOTE. Never add a fact, number, name or date that \
is not in them. NEARBY TEXT is context only, not evidence. Keep the sentence's language and tone \
and change as little as needed. Answer with one JSON object and nothing else:\n\
{\"action\":\"rewrite\",\"text\":\"<the corrected sentence>\"} when SOURCES or PERSON'S NOTE show \
what the sentence should say;\n\
{\"action\":\"delete\"} when SOURCES contradict the sentence and nothing in them can replace it;\n\
{\"action\":\"none\"} when they do not show what is wrong or how to fix it.\n\
The corrected sentence is one sentence, with no citation markers like [1], no markdown and no \
line breaks.";

/// A list, quote or heading prefix the sentence keeps whatever the model says.
static RE_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:#{1,6}\s+|>\s*|[-*+]\s+(?:\[[ xX]\]\s+)?|\d+[.)]\s+)").unwrap()
});
static RE_MARKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\d+\]").unwrap());

/// What came of asking the AI to fix a flagged sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagFixOutcome {
    /// A pending proposal with the corrected sentence.
    Proposed(String),
    /// A pending proposal to delete the sentence (empty text).
    ProposedDeletion(String),
    /// The model found nothing in the sources to fix it with.
    NoFix,
    /// Nothing was asked or filed, and why (`sentence_changed`, `not_flagged`,
    /// `nothing_to_ground`, `model_unavailable`, `model_error`,
    /// `unusable_answer`, `already_proposed`).
    Skipped(&'static str),
}

/// The model the AI fix uses: the everyday (routine, cheap) route.
///
/// When the person pinned an everyday source, only that source serves, and a
/// pinned source that is not available means no fix (never another
/// provider). With no pin, the person's press of "ask the AI" is the consent,
/// as for any foreground call: the first available of the Anthropic routine
/// slot, the external OpenAI-compatible server, and the on-device model.
pub fn fix_provider<'a>(
    pin: Option<EverydaySource>,
    api_llm: Option<&'a Arc<dyn LlmProvider>>,
    external_llm: Option<&'a Arc<dyn LlmProvider>>,
    on_device: Option<&'a Arc<dyn LlmProvider>>,
) -> Option<&'a Arc<dyn LlmProvider>> {
    if pin.is_some() {
        return resolve_everyday(pin, api_llm, external_llm, on_device).llm;
    }
    [api_llm, external_llm, on_device]
        .into_iter()
        .flatten()
        .find(|llm| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| llm.is_available()))
                .unwrap_or(false)
        })
}

#[derive(Deserialize)]
struct FixAnswer {
    action: String,
    #[serde(default)]
    text: Option<String>,
}

fn clip(text: &str, max_chars: usize) -> &str {
    text.char_indices()
        .nth(max_chars)
        .map_or(text, |(at, _)| &text[..at])
}

/// The model's sentence, cleaned: markers, a repeated prefix and extra
/// whitespace removed. `None` when it is not one sentence on one line.
fn clean_rewrite(text: &str, prefix: &str) -> Option<String> {
    let text = text.trim();
    if text.contains('\n') {
        return None;
    }
    let text = RE_MARKER.replace_all(text, "");
    let text = text.trim();
    let text = if prefix.is_empty() {
        text
    } else {
        RE_PREFIX.find(text).map_or(text, |m| &text[m.end()..])
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (authorship::sentence_units(&text).len() == 1).then_some(text)
}

/// Ask `llm` for a fix of the flagged sentence `sentence_hash` and file it as
/// a pending proposal with `why` (the flag's reason, or
/// [`DEFAULT_FIX_WHY`]). The flag must still stand: a sentence that changed
/// since the flag, or was never flagged, gets nothing. A model that is
/// unavailable or fails leaves everything as it was.
pub async fn ask_ai_to_fix(
    db: &MemoryDB,
    llm: &dyn LlmProvider,
    page_id: &str,
    sentence_hash: &str,
    why: Option<&str>,
) -> Result<FlagFixOutcome, WenlanError> {
    let page = load_page(db, page_id).await?;
    let units = authorship::sentence_units(&page.content);
    let Some(idx) = units.iter().position(|u| u.hash == sentence_hash) else {
        return Ok(FlagFixOutcome::Skipped("sentence_changed"));
    };
    let unit = &units[idx];
    let rows = page_authorship(db, &page).await?;
    let entries = db.list_sentence_history("page", page_id).await?;
    let still_flagged = rows
        .get(idx)
        .is_some_and(|row| flagged_since(&entries, &unit.hash, row.since_version));
    if !still_flagged {
        return Ok(FlagFixOutcome::Skipped("not_flagged"));
    }

    // The sources this sentence cites, each once, in marker order.
    let markers = occurrence_units(&page.content, &units);
    let occurrences: HashSet<u32> = markers
        .iter()
        .enumerate()
        .filter(|(_, at)| **at == Some(idx))
        .map(|(k, _)| u32::try_from(k + 1).unwrap_or(u32::MAX))
        .collect();
    let cited: Vec<_> = page
        .citations
        .iter()
        .filter(|c| occurrences.contains(&c.occurrence) && !c.locator.is_empty())
        .cloned()
        .collect();
    let states = source_states(db, &cited).await?;
    let mut seen = HashSet::new();
    let sources: Vec<&str> = cited
        .iter()
        .filter(|c| seen.insert(c.locator.as_str()))
        .filter_map(|c| states.get(&c.locator)?.as_ref())
        .map(|(_, _, text)| text.trim())
        .filter(|text| !text.is_empty())
        .collect();
    let note = why.map(str::trim).filter(|w| !w.is_empty());
    if sources.is_empty() && note.is_none() {
        // Nothing to ground a fix on: the model would only be guessing.
        return Ok(FlagFixOutcome::Skipped("nothing_to_ground"));
    }
    if !llm.is_available() {
        return Ok(FlagFixOutcome::Skipped("model_unavailable"));
    }

    let prefix = RE_PREFIX.find(&unit.text).map_or("", |m| m.as_str());
    let sentence = crate::citations::strip_markers(&unit.text[prefix.len()..]);
    let context = crate::citations::strip_markers(paragraph_at(&page.content, unit.start));
    let mut prompt = format!(
        "SENTENCE:\n{sentence}\n\nNEARBY TEXT:\n{}\n\nPERSON'S NOTE:\n{}\n\nSOURCES:\n",
        clip(&context, CONTEXT_CHARS),
        note.unwrap_or("(none)"),
    );
    if sources.is_empty() {
        prompt.push_str("(none)\n");
    }
    for (k, text) in sources.iter().enumerate() {
        prompt.push_str(&format!("Source {}: {}\n", k + 1, clip(text, SOURCE_CHARS)));
    }
    let raw = match llm
        .generate(LlmRequest {
            system_prompt: Some(SYSTEM_PROMPT.to_string()),
            user_prompt: prompt,
            max_tokens: 400,
            temperature: 0.1,
            label: Some("sentence_fix".to_string()),
            timeout_secs: None,
        })
        .await
    {
        Ok(raw) => raw,
        Err(error) => {
            log::warn!("[sentence_fix] model failed for {page_id}/{sentence_hash}: {error}");
            return Ok(FlagFixOutcome::Skipped("model_error"));
        }
    };
    let answer = strip_think_tags(&raw);
    let Some(answer) =
        extract_json(&answer).and_then(|json| serde_json::from_str::<FixAnswer>(json).ok())
    else {
        log::debug!("[sentence_fix] unusable answer for {page_id}/{sentence_hash}");
        return Ok(FlagFixOutcome::Skipped("unusable_answer"));
    };
    let after = match answer.action.trim().to_ascii_lowercase().as_str() {
        "none" => return Ok(FlagFixOutcome::NoFix),
        "delete" => String::new(),
        "rewrite" => {
            let Some(text) = answer
                .text
                .as_deref()
                .and_then(|text| clean_rewrite(text, prefix))
            else {
                return Ok(FlagFixOutcome::Skipped("unusable_answer"));
            };
            if authorship::normalize_sentence(&text) == authorship::normalize_sentence(&sentence) {
                return Ok(FlagFixOutcome::NoFix);
            }
            // The fix keeps what the sentence cites, after its full stop as
            // the page writes them.
            let cites: String = RE_MARKER
                .find_iter(&unit.text)
                .map(|m| m.as_str())
                .collect();
            format!("{prefix}{text}{cites}")
        }
        _ => return Ok(FlagFixOutcome::Skipped("unusable_answer")),
    };

    let already = proposals_from(&entries, page_id).iter().any(|p| {
        p.status == SentenceProposalStatus::Pending
            && p.sentence_hash == unit.hash
            && p.after == after
    });
    if already {
        return Ok(FlagFixOutcome::Skipped("already_proposed"));
    }
    // No facts-changed reason: the person's standing flag is what lets the
    // fix reach a protected sentence. `expected_version` refuses it when the
    // page moved on while the model was thinking.
    let req = SentenceProposeRequest {
        sentence_hash: unit.hash.clone(),
        text: after.clone(),
        why: Some(note.unwrap_or(DEFAULT_FIX_WHY).to_string()),
        reason: None,
        expected_version: Some(page.version),
    };
    let actor = HistoryActor {
        kind: ActorKind::Ai,
        name: FIX_ACTOR.to_string(),
        agent: None,
        session: None,
    };
    let proposal = propose_as(db, page_id, &req, &actor, ProposalGrant::PersonFlagged).await?;
    Ok(if after.is_empty() {
        FlagFixOutcome::ProposedDeletion(proposal.id)
    } else {
        FlagFixOutcome::Proposed(proposal.id)
    })
}
