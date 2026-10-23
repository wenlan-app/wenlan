// SPDX-License-Identifier: Apache-2.0
//! Automatic mode's share of code work: Wenlan's own model rewrites a note
//! whose code changed, the way an on-demand caller's main model does.
//!
//! A note is `code_changed` once a model judged a changed code basis `stale`
//! (the background recheck, or the caller's small model on demand). The
//! automatic run claims it through [`claim_as`] and lands the answer through
//! [`commit`], so the snapshot, the ownership rule (a note a person edited
//! becomes a revision card), the citation check and the settling of the code
//! bases are the on-demand path's own. Only the model differs.
//!
//! An answer that does not land (a citation refusal, a body the page format
//! refuses, the same text again) is remembered by what it was made from: the
//! note's version and the verdicts it acted on. The same inputs would fail the
//! same way, so the note waits until the note or its code moves, or the
//! daemon restarts, instead of costing a model call every turn.

use std::sync::Arc;
use std::time::Duration;

use wenlan_types::steep::{STEEP_ACTION_REWRITE, STEEP_ACTION_STILL_ACCURATE};

use super::{claim_as, commit, SteepClaim, SteepError, SteepHolder};
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::llm_provider::{LlmProvider, LlmRequest};
use crate::prompts::PromptRegistry;
use crate::read_scope::ReadScope;

/// How long one rewrite may take before the turn gives up on it.
const MODEL_TIMEOUT: Duration = Duration::from_secs(180);

/// The model's whole answer when the note still agrees with the code.
const STILL_ACCURATE_REPLY: &str = "STILL_ACCURATE";

/// A correction for changed code rarely removes most of a note; a rewrite
/// shorter than this share of it more likely dropped sentences that still
/// hold, so it is not saved.
const MIN_KEPT_SHARE: f64 = 0.5;

/// What one automatic code rewrite did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeRewriteOutcome {
    /// The note now says what the code says, and its code bases are settled.
    Written,
    /// A person edited the note, so the rewrite waits for them as a card.
    Card,
    /// The model read the code and the note still agrees with it.
    StillAccurate,
    /// The answer did not land; the note waits until it or its code moves.
    Failed,
    /// The note or its code moved during the rewrite; nothing was written,
    /// and the note is taken again once it is judged as it is now.
    Moved,
}

/// Rewrite at most one `code_changed` note with `llm`. `None` when no note
/// was waiting that this run may take (none judged `stale`, or each one held
/// by an on-demand caller, waiting on a card, or already failed as it is).
pub async fn rewrite_one_code_page(
    db: &MemoryDB,
    llm: &Arc<dyn LlmProvider>,
    prompts: &PromptRegistry,
    knowledge_path: Option<&std::path::Path>,
) -> Result<Option<CodeRewriteOutcome>, WenlanError> {
    for (page, stale, _) in db.list_steep_code_pages().await? {
        if stale == 0 {
            continue;
        }
        let claimed = match claim_as(
            db,
            &page.id,
            &ReadScope::Global,
            None,
            SteepHolder::Automatic,
        )
        .await
        {
            Ok(claimed) => claimed,
            Err(SteepError::Db(error)) => return Err(error),
            // Held, gone, or nothing judged `stale` any more.
            Err(_) => continue,
        };
        // A note that rests on other notes, or whose sources left this
        // Space, needs the user; a claim for it holds nothing.
        let Some(token) = claimed.token.clone() else {
            continue;
        };
        let attempt = attempt_key(db, &claimed, &token);
        if db.steep_leases().failed().contains(&attempt) {
            db.steep_leases().release(&page.id, &token);
            continue;
        }

        let outcome = match generate(llm, prompts, &claimed).await {
            Some(reply) => land(db, &claimed, &token, &reply, knowledge_path).await?,
            None => CodeRewriteOutcome::Failed,
        };
        db.steep_leases().release(&page.id, &token);
        if outcome == CodeRewriteOutcome::Failed {
            db.steep_leases().failed().insert(attempt);
        }
        log::info!("[steep-code] '{}': {outcome:?}", page.title);
        return Ok(Some(outcome));
    }
    Ok(None)
}

/// What the answer was made from: the note's version and the verdicts it
/// acted on (each verdict is keyed by the code it judged).
fn attempt_key(db: &MemoryDB, claimed: &SteepClaim, token: &str) -> String {
    let keys: Vec<String> = db
        .steep_leases()
        .find(token)
        .and_then(|lease| lease.snapshot)
        .map(|snapshot| snapshot.code.into_iter().map(|code| code.key).collect())
        .unwrap_or_default();
    format!(
        "{}@v{}:{}",
        claimed.page_id,
        claimed.version,
        keys.join(",")
    )
}

fn user_prompt(claimed: &SteepClaim) -> String {
    let mut prompt = format!(
        "Title: {}\n\nThe note as it reads:\n{}\n",
        claimed.title, claimed.current_content
    );
    if claimed.sources.is_empty() {
        prompt.push_str("\nThis note has no numbered sources: write no [n] markers.\n");
    } else {
        prompt.push_str("\nNumbered sources (cite only these numbers):\n");
        for source in &claimed.sources {
            prompt.push_str(&format!("[{}] {}\n", source.n, source.text));
        }
    }
    for code in &claimed.code {
        prompt.push_str(&format!(
            "\nCode the note relies on, {} lines {} to {}, as it reads now:\n{}\n",
            code.path, code.line_start, code.line_end, code.excerpt
        ));
        if !code.what_changed.is_empty() {
            prompt.push_str(&format!(
                "What the code means for the note (describes the change; do not copy its \
                 wording): {}\n",
                code.what_changed
            ));
        }
    }
    prompt
}

/// One model call; `None` for an error, a timeout or an empty reply.
async fn generate(
    llm: &Arc<dyn LlmProvider>,
    prompts: &PromptRegistry,
    claimed: &SteepClaim,
) -> Option<String> {
    let request = LlmRequest {
        system_prompt: Some(prompts.steep_code_rewrite.clone()),
        user_prompt: user_prompt(claimed),
        max_tokens: llm.recommended_max_output(),
        temperature: 0.0,
        label: Some("steep_code_rewrite".to_string()),
        timeout_secs: Some(MODEL_TIMEOUT.as_secs()),
    };
    match tokio::time::timeout(MODEL_TIMEOUT, llm.generate(request)).await {
        Ok(Ok(reply)) => Some(unfence(&reply)).filter(|reply| !reply.is_empty()),
        Ok(Err(error)) => {
            log::warn!("[steep-code] model call failed: {error}");
            None
        }
        Err(_) => {
            log::warn!("[steep-code] model call timed out");
            None
        }
    }
}

/// `body` without `[n]` citation markers (and the space before each).
fn drop_markers(body: &str) -> String {
    static MARKER: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r" ?\[\d+(?:\s*,\s*\d+)*\]").unwrap());
    MARKER.replace_all(body, "").into_owned()
}

/// The reply without a code fence wrapped around all of it. A note that opens
/// with a diagram block is the note, not a wrapper.
fn unfence(reply: &str) -> String {
    let trimmed = reply.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed.to_string();
    };
    if rest.starts_with("mermaid") {
        return trimmed.to_string();
    }
    let Some(body) = rest.strip_suffix("```") else {
        return trimmed.to_string();
    };
    // The opening fence's language tag, if any, ends at the first newline.
    body.split_once('\n')
        .map_or(body, |(_, body)| body)
        .trim()
        .to_string()
}

async fn land(
    db: &MemoryDB,
    claimed: &SteepClaim,
    token: &str,
    reply: &str,
    knowledge_path: Option<&std::path::Path>,
) -> Result<CodeRewriteOutcome, WenlanError> {
    let (action, content) = if reply == STILL_ACCURATE_REPLY {
        (STEEP_ACTION_STILL_ACCURATE, None)
    } else {
        let kept = reply.chars().count() as f64;
        let before = claimed.current_content.trim().chars().count() as f64;
        if kept < before * MIN_KEPT_SHARE {
            log::info!(
                "[steep-code] '{}' rewrite not saved: it drops most of the note",
                claimed.title
            );
            return Ok(CodeRewriteOutcome::Failed);
        }
        (STEEP_ACTION_REWRITE, Some(reply))
    };
    // With no sources there is no number a marker could name, so any the
    // model wrote anyway are dropped rather than refusing the whole rewrite.
    let unmarked;
    let content = match content {
        Some(body) if claimed.sources.is_empty() => {
            unmarked = drop_markers(body);
            Some(unmarked.as_str())
        }
        other => other,
    };
    match commit(db, token, action, content, &[], knowledge_path, None).await {
        Ok(done) => Ok(match done.outcome {
            "written" => CodeRewriteOutcome::Written,
            "review_card" => CodeRewriteOutcome::Card,
            "still_accurate" => CodeRewriteOutcome::StillAccurate,
            _ => CodeRewriteOutcome::Failed,
        }),
        Err(SteepError::Db(error)) => Err(error),
        Err(SteepError::Changed(_) | SteepError::TokenGone | SteepError::NotFound) => {
            Ok(CodeRewriteOutcome::Moved)
        }
        Err(error) => {
            log::info!(
                "[steep-code] '{}' rewrite not saved: {error}",
                claimed.title
            );
            Ok(CodeRewriteOutcome::Failed)
        }
    }
}

#[cfg(test)]
#[path = "steep_automatic_test.rs"]
mod steep_automatic_test;
