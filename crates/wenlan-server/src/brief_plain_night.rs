// SPDX-License-Identifier: Apache-2.0
//! The night's pass over Space Briefs (docs/ux/spec.md, UX-SPACE-2): for a
//! Space whose Brief changed since its plain lines were written, ask the
//! model for one plain line on where the work stands and one per active item,
//! and close an item a newer note in the Space says is done. Closing goes
//! through the Brief's own update, so it is in the Brief's history like any
//! AI tool's handoff. The Brief text itself is never rewritten.

use std::sync::Arc;

use wenlan_core::brief_plain::{self, BriefPlain};
use wenlan_core::db::MemoryDB;
use wenlan_core::llm_provider::{LlmProvider, LlmRequest};
use wenlan_core::read_scope::ReadScope;
use wenlan_types::{Brief, BriefMutation, BriefUpdateRequest};

/// Most Spaces written in one look; the rest wait for the next.
const SPACES_PER_LOOK: usize = 3;
/// How far back newer notes are looked at.
const EVIDENCE_WINDOW_SECS: i64 = 60 * 24 * 60 * 60;
/// Who closes an item, in the Brief's history.
pub const CALLER: &str = "wenlan-night";

/// The plain lines kept for a Space, if any.
pub async fn load(db: &MemoryDB, space_id: &str) -> Option<BriefPlain> {
    let raw = db
        .get_app_metadata(&brief_plain::metadata_key(space_id))
        .await
        .ok()
        .flatten()?;
    serde_json::from_str(&raw).ok()
}

async fn save(db: &MemoryDB, space_id: &str, plain: &BriefPlain) -> Result<(), String> {
    let raw = serde_json::to_string(plain).map_err(|e| e.to_string())?;
    db.set_app_metadata(&brief_plain::metadata_key(space_id), &raw)
        .await
        .map_err(|e| e.to_string())
}

/// What was written in the Space since its oldest active item, newest first:
/// memories, and the lines of things still going on there.
async fn evidence(db: &MemoryDB, brief: &Brief, now: i64) -> Vec<String> {
    let oldest = brief
        .active
        .iter()
        .map(|item| item.added_at)
        .min()
        .unwrap_or(now)
        .max(now - EVIDENCE_WINDOW_SECS);
    let scope = ReadScope::Space(brief.space.clone());
    let mut lines: Vec<(i64, String)> = Vec::new();
    if let Ok(memories) = db
        .list_recent_memories_scoped(
            brief_plain::MAX_EVIDENCE as i64,
            Some(oldest * 1000),
            &scope,
        )
        .await
    {
        for memory in memories {
            let text = match memory.snippet.as_deref().map(str::trim) {
                Some(snippet) if !snippet.is_empty() => format!("{}: {snippet}", memory.title),
                _ => memory.title.clone(),
            };
            lines.push(((memory.timestamp_ms / 1000) as i64, text));
        }
    }
    if let Ok(things) = db.list_things(&scope, None).await {
        for thing in things {
            // A quiet thing may be a Brief item itself: it is no evidence.
            if thing.quiet || thing.last_activity_at < oldest {
                continue;
            }
            if let Some(line) = thing.status_line.filter(|line| !line.trim().is_empty()) {
                let title = thing.title.unwrap_or_default();
                lines.push((thing.last_activity_at, format!("{title}: {line}")));
            }
        }
    }
    lines.sort_by_key(|line| std::cmp::Reverse(line.0));
    lines.into_iter().map(|(_, text)| text).collect()
}

/// Write the plain lines for one Space's Brief, closing what a newer note
/// says is done. Returns how many items were closed.
pub async fn write_space(
    db: &MemoryDB,
    model: &Arc<dyn LlmProvider>,
    brief: Brief,
    space_id: &str,
    language: &str,
    now: i64,
) -> Result<usize, String> {
    let evidence = evidence(db, &brief, now).await;
    let request = LlmRequest {
        system_prompt: Some(
            "You rewrite a project's handoff note in plain words for its owner. Answer with JSON only."
                .into(),
        ),
        user_prompt: brief_plain::plain_prompt(&brief, &evidence, language),
        max_tokens: 2_048,
        temperature: 0.0,
        label: Some("brief_plain".into()),
        timeout_secs: Some(300),
    };
    let answer = model.generate(request).await.map_err(|e| e.to_string())?;
    let parsed = brief_plain::parse_plain(&answer, &brief, &evidence)
        .ok_or_else(|| "the answer was not JSON".to_string())?;
    let mut version = brief.version;
    let closed = parsed.done.len();
    if closed > 0 {
        let mutations = parsed
            .done
            .iter()
            .filter_map(|done| {
                let item = brief.active.iter().find(|item| item.id == done.item_id)?;
                Some(BriefMutation::Complete {
                    item_id: item.id.clone(),
                    expected_version: item.version,
                })
            })
            .collect();
        let receipt = db
            .apply_brief_update(&BriefUpdateRequest {
                space: brief.space.clone(),
                caller_id: CALLER.into(),
                operation_id: format!("brief-plain-{}-{}", space_id, brief.version),
                summary: None,
                mutations,
            })
            .await
            .map_err(|e| e.to_string())?;
        version = receipt.brief_version;
    }
    save(
        db,
        space_id,
        &BriefPlain {
            brief_version: version,
            language: language.to_string(),
            summary: parsed.summary,
            lines: parsed.lines,
            written_at: now,
        },
    )
    .await?;
    Ok(closed)
}

/// One look's pass: up to [`SPACES_PER_LOOK`] kept Spaces whose Brief has no
/// current plain lines. Returns the Spaces written and the items closed.
pub async fn plain_once(
    db: &MemoryDB,
    model: &Arc<dyn LlmProvider>,
    language: &str,
    now: i64,
) -> Result<(usize, usize), String> {
    let spaces = db.list_spaces().await.map_err(|e| e.to_string())?;
    let mut written = 0;
    let mut closed = 0;
    for space in spaces.into_iter().filter(|space| !space.suggested) {
        if written >= SPACES_PER_LOOK {
            break;
        }
        let Ok(Some(brief)) = db.get_brief_by_space_name(&space.name).await else {
            continue;
        };
        if brief.active.is_empty() && brief.last_session_summary.trim().is_empty() {
            continue;
        }
        if brief_plain::is_current(load(db, &space.id).await.as_ref(), &brief, language) {
            continue;
        }
        match write_space(db, model, brief, &space.id, language, now).await {
            Ok(n) => {
                written += 1;
                closed += n;
            }
            Err(error) => {
                tracing::warn!("[brief] plain lines for {}: {error}", space.name);
                return Err(error);
            }
        }
    }
    Ok((written, closed))
}
