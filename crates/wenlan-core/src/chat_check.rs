// SPDX-License-Identifier: Apache-2.0
//! The in-chat check (UX-CHAT-1/2/6).
//!
//! Two passes, both daemon-side so every AI tool gets the same answer:
//!
//! - [`check_against_decisions`] (Stop hook, MCP `check_reply`): does the AI's latest
//!   reply go against something decided earlier? Each hit becomes a reminder
//!   with the decided statement, its date and an openable source, logged as a
//!   `chat.reminder` history entry. At most [`CHAT_REMINDER_DAILY_CAP`] per
//!   local day, counted from that log.
//! - [`decision_context`] (UserPromptSubmit hook): the past decisions a prompt
//!   touches, handed to the AI before it answers.
//!
//! The comparison is the decision comparison the background resolution
//! already uses (`contradiction::fields_may_contradict` on decision fields),
//! reached through `contradiction::statement_contradicts_decision`. It is
//! deterministic and needs no model, so it fits the one-second budget.

use crate::contradiction::{decision_terms, statement_contradicts_decision, DecisionConflict};
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::read_scope::ReadScope;
use chrono::{DateTime, Local, TimeZone, Utc};
use std::collections::HashSet;
use wenlan_types::chat_check::{
    memory_link, page_link, ChatCheckResponse, ChatCheckSkipped, ChatReminder, DecisionSource,
    PastDecision, SourcePageRef, CHAT_REMINDER_DAILY_CAP, CHAT_REMINDER_OP,
};
use wenlan_types::MemoryItem;

/// The work of one check must finish within this, or the route answers
/// empty (`skipped: timeout`). It is under one second so the answer, framing
/// included, reaches a hook that waits one second.
pub const CHECK_BUDGET: std::time::Duration = std::time::Duration::from_millis(800);
/// Decision memories read per check (newest first).
const DECISION_SCAN_LIMIT: usize = 300;
/// Characters of a reply that are read.
const REPLY_CHAR_LIMIT: usize = 20_000;
/// Decisions handed to the AI for one prompt.
const CONTEXT_LIMIT: usize = 3;
const DECIDED_CHAR_LIMIT: usize = 240;

/// Unix seconds of the start of the local day holding `now`.
pub fn local_day_start(now: DateTime<Local>) -> i64 {
    let midnight = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight exists");
    Local
        .from_local_datetime(&midnight)
        .earliest()
        .map(|t| t.timestamp())
        .unwrap_or_else(|| now.timestamp() - 86_400)
}

/// Sentences of a reply, outside fenced code, with list markers stripped.
pub fn split_sentences(text: &str) -> Vec<String> {
    let text: String = text.chars().take(REPLY_CHAR_LIMIT).collect();
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let line = trimmed
            .trim_start_matches(['-', '*', '+', '>', '#'])
            .trim_start();
        let mut current = String::new();
        for c in line.chars() {
            current.push(c);
            if matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | ';' | '；') {
                push_sentence(&mut out, &mut current);
            }
        }
        push_sentence(&mut out, &mut current);
    }
    out
}

fn push_sentence(out: &mut Vec<String>, current: &mut String) {
    let sentence = current.trim();
    let letters = sentence.chars().filter(|c| c.is_alphanumeric()).count();
    if letters >= 3 || (letters >= 2 && has_cjk(sentence)) {
        out.push(sentence.to_string());
    }
    current.clear();
}

fn has_cjk(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF))
}

fn cut(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The local date a decision was made: its structured `date` when that is a
/// `YYYY-MM-DD` date, else the day the memory was last written.
fn decided_on(memory: &MemoryItem) -> String {
    let explicit = memory
        .structured_fields
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|fields| {
            fields
                .get("date")
                .and_then(|d| d.as_str())
                .map(str::to_string)
        })
        .and_then(|date| {
            let head: String = date.chars().take(10).collect();
            chrono::NaiveDate::parse_from_str(&head, "%Y-%m-%d")
                .ok()
                .map(|d| d.format("%Y-%m-%d").to_string())
        });
    explicit.unwrap_or_else(|| {
        Local
            .timestamp_opt(memory.last_modified, 0)
            .single()
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "unknown".to_string())
    })
}

fn decided_text(memory: &MemoryItem) -> String {
    let text = if memory.content.trim().is_empty() {
        memory.title.as_str()
    } else {
        memory.content.as_str()
    };
    cut(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        DECIDED_CHAR_LIMIT,
    )
}

/// The openable source: the newest page in scope that cites the memory, else
/// the memory itself.
async fn decision_source(
    db: &MemoryDB,
    memory: &MemoryItem,
    scope: &ReadScope,
) -> Result<DecisionSource, WenlanError> {
    let mut pages: Vec<_> = db
        .get_pages_for_memory(&memory.source_id)
        .await?
        .into_iter()
        .filter(|page| page.status == "active" && scope.matches(page.space.as_deref()))
        .collect();
    pages.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
    let page = pages.into_iter().next().map(|page| SourcePageRef {
        page_id: page.id,
        version: page.version,
        title: page.title,
    });
    let link = page
        .as_ref()
        .map(|p| page_link(&p.page_id))
        .unwrap_or_else(|| memory_link(&memory.source_id));
    Ok(DecisionSource {
        memory_id: memory.source_id.clone(),
        memory_version: memory.version,
        page,
        link,
    })
}

/// The one line the person sees.
pub fn reminder_text(decided: &str, decided_on: &str, link: &str) -> String {
    if has_cjk(decided) {
        format!("之前的決定（{decided_on}）：{decided} {link}")
    } else {
        format!("Earlier decision ({decided_on}): {decided} {link}")
    }
}

async fn decisions_in_scope(
    db: &MemoryDB,
    scope: &ReadScope,
) -> Result<Vec<MemoryItem>, WenlanError> {
    db.list_memories_scoped(
        scope,
        Some("decision"),
        Some(true),
        None,
        DECISION_SCAN_LIMIT,
    )
    .await
}

/// A reminder found but not yet logged.
struct Found {
    memory: MemoryItem,
    sentence: String,
    conflict: DecisionConflict,
}

/// Check `reply` against the decisions in `scope` and log what is handed out.
/// `now` decides the local day for the cap.
pub async fn check_against_decisions(
    db: &MemoryDB,
    reply: &str,
    session: Option<&str>,
    scope: &ReadScope,
    now: DateTime<Local>,
) -> Result<ChatCheckResponse, WenlanError> {
    let sentences = split_sentences(reply);
    if sentences.is_empty() {
        return Ok(ChatCheckResponse {
            skipped: Some(ChatCheckSkipped::Empty),
            remaining_today: remaining(db, now).await?,
            ..Default::default()
        });
    }
    let day_start = local_day_start(now);
    let used = db.count_chat_reminders_since(day_start).await?;
    if used >= CHAT_REMINDER_DAILY_CAP {
        return Ok(ChatCheckResponse {
            skipped: Some(ChatCheckSkipped::DailyCap),
            ..Default::default()
        });
    }
    let budget = (CHAT_REMINDER_DAILY_CAP - used) as usize;
    let already = match session {
        Some(session) => {
            db.chat_reminder_targets_since(day_start, Some(session))
                .await?
        }
        None => HashSet::new(),
    };

    let mut found: Vec<Found> = Vec::new();
    for memory in decisions_in_scope(db, scope).await? {
        if found.len() >= budget {
            break;
        }
        if already.contains(&memory.source_id) {
            continue;
        }
        let terms = decision_terms(memory.structured_fields.as_deref(), &memory.content);
        if terms.chosen.is_empty() && terms.rejected.is_empty() {
            continue;
        }
        let hit = sentences.iter().find_map(|sentence| {
            statement_contradicts_decision(&terms, sentence).map(|c| (sentence.clone(), c))
        });
        if let Some((sentence, conflict)) = hit {
            found.push(Found {
                memory,
                sentence,
                conflict,
            });
        }
    }

    let mut reminders = Vec::new();
    for hit in found {
        let decided = decided_text(&hit.memory);
        let on = decided_on(&hit.memory);
        let source = decision_source(db, &hit.memory, scope).await?;
        let text = reminder_text(&decided, &on, &source.link);
        let reply_excerpt = cut(&hit.sentence, DECIDED_CHAR_LIMIT);
        let option = hit.conflict.option().to_string();
        let conflict = match hit.conflict {
            DecisionConflict::AdoptsRejected { .. } => "adopts_rejected",
            DecisionConflict::DropsChosen { .. } => "drops_chosen",
        };
        let data = serde_json::json!({
            "decided": decided,
            "decided_on": on,
            "reply_excerpt": reply_excerpt,
            "option": option,
            "conflict": conflict,
            "source": source,
        });
        let ts = now.timestamp();
        let target_id = hit.memory.source_id.clone();
        let version = hit.memory.version;
        let entry_text = text.clone();
        let (seq, _) = db
            .append_chat_reminder(ts, session, "memory", &target_id, |seq| {
                reminder_history_entry(ReminderEntry {
                    seq,
                    at: now.with_timezone(&Utc),
                    session,
                    target_id: &target_id,
                    version,
                    text: &entry_text,
                    data,
                })
            })
            .await?;
        reminders.push(ChatReminder {
            seq,
            text,
            decided,
            decided_on: on,
            reply_excerpt,
            option,
            source,
        });
    }
    let remaining_today = CHAT_REMINDER_DAILY_CAP.saturating_sub(used + reminders.len() as u32);
    Ok(ChatCheckResponse {
        reminders,
        skipped: None,
        remaining_today,
    })
}

async fn remaining(db: &MemoryDB, now: DateTime<Local>) -> Result<u32, WenlanError> {
    let used = db.count_chat_reminders_since(local_day_start(now)).await?;
    Ok(CHAT_REMINDER_DAILY_CAP.saturating_sub(used))
}

/// Inputs of one `chat.reminder` history entry.
pub struct ReminderEntry<'a> {
    pub seq: i64,
    pub at: DateTime<Utc>,
    pub session: Option<&'a str>,
    pub target_id: &'a str,
    pub version: i64,
    pub text: &'a str,
    pub data: serde_json::Value,
}

/// A `chat.reminder` entry in the history format of the migration spec
/// (section 4): `{seq, ts, tx, actor, op, target, version, why, text, data,
/// crc}`. `crc` is the CRC-32 of the entry serialized without it.
pub fn reminder_history_entry(entry: ReminderEntry<'_>) -> serde_json::Value {
    let agent = entry
        .session
        .and_then(|s| s.split_once(':'))
        .map(|(tool, _)| tool.to_string());
    let mut value = serde_json::json!({
        "seq": entry.seq,
        "ts": entry.at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "tx": format!("tx_{}", uuid::Uuid::new_v4().simple()),
        "actor": {
            "kind": "system",
            "name": "wenlan",
            "agent": agent,
            "session": entry.session,
        },
        "op": CHAT_REMINDER_OP,
        "target": {"type": "memory", "id": entry.target_id},
        "version": entry.version,
        "why": "the reply goes against an earlier decision",
        "text": entry.text,
        "data": entry.data,
    });
    let crc = crc32(value.to_string().as_bytes());
    value["crc"] = serde_json::Value::String(format!("{crc:08x}"));
    value
}

/// CRC-32 (IEEE 802.3), as the history format uses per entry.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "from", "into", "should", "would", "could",
    "what", "when", "where", "which", "how", "why", "are", "was", "were", "have", "has", "will",
    "can", "you", "your", "our", "use", "using", "let", "lets", "let's", "make", "about", "not",
    "but", "all", "any", "now", "then", "there", "here", "just", "also", "please", "want", "need",
];

fn tokens(text: &str) -> HashSet<String> {
    let lower = text.to_lowercase();
    let mut out = HashSet::new();
    let mut word = String::new();
    let mut cjk_run: Vec<char> = Vec::new();
    let flush_word = |word: &mut String, out: &mut HashSet<String>| {
        if word.chars().count() >= 3 && !STOPWORDS.contains(&word.as_str()) {
            out.insert(std::mem::take(word));
        } else {
            word.clear();
        }
    };
    let flush_cjk = |run: &mut Vec<char>, out: &mut HashSet<String>| {
        for pair in run.windows(2) {
            out.insert(pair.iter().collect());
        }
        run.clear();
    };
    for c in lower.chars() {
        if matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF) {
            flush_word(&mut word, &mut out);
            cjk_run.push(c);
        } else if c.is_alphanumeric() {
            flush_cjk(&mut cjk_run, &mut out);
            word.push(c);
        } else {
            flush_word(&mut word, &mut out);
            flush_cjk(&mut cjk_run, &mut out);
        }
    }
    flush_word(&mut word, &mut out);
    flush_cjk(&mut cjk_run, &mut out);
    out
}

/// The past decisions `prompt` touches, most relevant first.
pub async fn decision_context(
    db: &MemoryDB,
    prompt: &str,
    scope: &ReadScope,
) -> Result<Vec<PastDecision>, WenlanError> {
    let prompt_tokens = tokens(prompt);
    if prompt_tokens.is_empty() {
        return Ok(Vec::new());
    }
    let mut scored: Vec<(usize, MemoryItem)> = Vec::new();
    for memory in decisions_in_scope(db, scope).await? {
        let terms = decision_terms(memory.structured_fields.as_deref(), &memory.content);
        let mut decision_tokens = tokens(&memory.content);
        if let Some(context) = &terms.context {
            decision_tokens.extend(tokens(context));
        }
        let shared = decision_tokens.intersection(&prompt_tokens).count();
        let names_option = terms
            .chosen
            .iter()
            .chain(terms.rejected.iter())
            .any(|option| {
                let option_tokens = tokens(option);
                !option_tokens.is_empty() && option_tokens.is_subset(&prompt_tokens)
            });
        let score = shared + if names_option { 2 } else { 0 };
        if score >= 2 {
            scored.push((score, memory));
        }
    }
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.last_modified.cmp(&a.1.last_modified))
    });
    let mut out = Vec::new();
    for (_, memory) in scored.into_iter().take(CONTEXT_LIMIT) {
        out.push(PastDecision {
            decided: decided_text(&memory),
            decided_on: decided_on(&memory),
            source: decision_source(db, &memory, scope).await?,
        });
    }
    Ok(out)
}

/// The context block the UserPromptSubmit hook hands the AI.
pub fn context_block(decisions: &[PastDecision]) -> String {
    if decisions.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "Past decisions in Wenlan that bear on this (check before contradicting them):\n",
    );
    for decision in decisions {
        out.push_str(&format!(
            "- {}: {} ({})\n",
            decision.decided_on, decision.decided, decision.source.link
        ));
    }
    out
}

#[cfg(test)]
#[path = "chat_check_test.rs"]
mod tests;
