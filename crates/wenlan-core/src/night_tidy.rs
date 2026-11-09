// SPDX-License-Identifier: Apache-2.0
//! What a night tidies among the memories an AI wrote (docs/ux/spec.md,
//! UX-GRAPH-7): merge two that say the same, let a newer one take the place
//! of one it says no longer holds, and bring one whose planned date has
//! passed up to date. Pure: the prompt, and the checks on the model's answer.
//!
//! Only memories an AI wrote are offered (`authorship::memory_author`), and
//! only within one Space. Nothing is deleted: what a change takes the place
//! of is hidden and comes back on Undo.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use crate::conversation_intake::plain_words;

/// Most memories of a thing a night looks at for tidying.
pub const MAX_TOUCHED: usize = 8;
/// Most neighbours offered with each one.
pub const MAX_NEIGHBOURS: usize = 5;
/// How alike a neighbour must be to be offered. A starting value; the dry
/// run on the owner's database copy reports what it touches.
pub const MIN_SIMILARITY: f32 = 0.85;
/// Longest text a merge or update may write, in characters.
pub const MAX_TIDY_CHARS: usize = 400;

/// A memory's date has passed once the whole day is over. `event_date` is
/// the day's first second in UTC, so a night early on that day, in any
/// timezone, does not yet count it as past.
pub fn date_passed(event_date: i64, now: i64) -> bool {
    event_date + 86_400 <= now
}

/// A memory is a plan whose day has passed when, as it was written, its day
/// had not begun yet, and now that day is over. "Merged on 11/6" written on
/// 11/7 says what happened, not what was planned: it is not one (the
/// 11-08 dry run on the owner's copy found most passed dates were such facts).
pub fn plan_passed(event_date: i64, created_at: i64, now: i64) -> bool {
    created_at < event_date && date_passed(event_date, now)
}

/// A memory offered to the night, as the prompt shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct TidyItem {
    pub id: String,
    pub text: String,
    /// `None` when unfiled.
    pub space: Option<String>,
    pub created_at: i64,
    /// The date it is about (`memories.event_date`), Unix seconds.
    pub event_date: Option<i64>,
    /// A decision rather than a fact.
    pub decided: bool,
    /// The conversation it came from and the line there, carried to what a
    /// merge or update writes so it keeps its source.
    pub session_id: Option<String>,
    pub line: Option<String>,
}

/// One change the model asked for and the checks let through. Indices are
/// into the items given to [`parse_tidy`].
#[derive(Debug, Clone, PartialEq)]
pub enum TidyChange {
    /// Two memories say the same: `text` says what both say.
    Merge { items: [usize; 2], text: String },
    /// `keep` is newer and says `drop` no longer holds.
    Replace { keep: usize, drop: usize },
    /// `item`'s planned date has passed: `text` says it in the past tense.
    Update { item: usize, text: String },
}

/// The prompt for one thing's night. `groups` are indices into `items`: each
/// group is a memory written since the last night and the ones most like it.
pub fn tidy_prompt(items: &[TidyItem], groups: &[Vec<usize>], now: i64) -> String {
    let date = say_date;
    let mut list = String::new();
    for (g, group) in groups.iter().enumerate() {
        list.push_str(&format!("Group {}:\n", g + 1));
        for &i in group {
            let item = &items[i];
            let about = item
                .event_date
                .map(|at| format!(", about {}", date(at)))
                .unwrap_or_default();
            list.push_str(&format!(
                "M{i} (written {}{about}): {}\n",
                date(item.created_at),
                item.text
            ));
        }
    }
    format!(
        "Memories an AI wrote for a person, in groups of ones that are alike. Today is {}.\n\n\
{list}\n\
Tidy them only where you are sure:\n\
- \"merge\": two memories say the same thing. Write one text that says what both say, in \
their language. Add nothing that neither says.\n\
- \"replace\": a newer memory says an older one no longer holds. \"keep\" is the newer, \
\"drop\" the older.\n\
- \"update\": a memory about a date that has passed. Say in the past tense what was planned, \
like \"The launch was set for Nov 3.\" Never say what happened.\n\
Leave everything else as it is: most groups need nothing. Use each memory at most once. \
No PR or issue numbers, hashes, paths or file names.\n\
Answer with JSON only: {{\"changes\": [{{\"do\": \"merge\", \"memories\": [\"M0\", \"M1\"], \
\"text\": string}}, {{\"do\": \"replace\", \"keep\": \"M2\", \"drop\": \"M3\"}}, \
{{\"do\": \"update\", \"memory\": \"M4\", \"text\": string}}]}}",
        date(now)
    )
}

static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("number regex"));

/// Every number in `text` is in one of `sources`: a merge or update may say
/// a thing again, never a new count, amount or date.
/// Numbers compare by value, so "03" in a source allows "3".
fn numbers_from(text: &str, sources: &[&str]) -> bool {
    let value = |m: regex::Match<'_>| m.as_str().trim_start_matches('0').to_string();
    let known: HashSet<String> = sources
        .iter()
        .flat_map(|s| NUMBER.find_iter(s).map(value))
        .collect();
    NUMBER.find_iter(text).all(|m| known.contains(&value(m)))
}

/// A date as the prompt and an update's text say it.
fn say_date(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|d| d.format("%b %-d, %Y").to_string())
        .unwrap_or_default()
}

fn index(id: &str, len: usize) -> Option<usize> {
    id.trim()
        .strip_prefix('M')
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| *n < len)
}

/// The changes in a model's answer that pass the checks. A change that names
/// a memory not offered, or one already used, or two memories in different
/// Spaces, is dropped; so is a merge or update whose text is empty, too long,
/// not in plain words or brings in a number the memories do not have, a
/// replace whose `keep` is not the newer, and an update of a memory whose
/// date has not passed. `None` when the answer is not JSON.
pub fn parse_tidy(text: &str, items: &[TidyItem], now: i64) -> Option<Vec<TidyChange>> {
    #[derive(Deserialize)]
    struct Change {
        #[serde(default, rename = "do")]
        action: String,
        #[serde(default)]
        memories: Vec<String>,
        #[serde(default)]
        keep: Option<String>,
        #[serde(default)]
        drop: Option<String>,
        #[serde(default)]
        memory: Option<String>,
        #[serde(default)]
        text: String,
    }
    #[derive(Deserialize)]
    struct Answer {
        #[serde(default)]
        changes: Vec<Change>,
    }
    let json = {
        let text = match text.rfind("</think>") {
            Some(end) => &text[end + "</think>".len()..],
            None => text,
        };
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        (end > start).then(|| &text[start..=end])?
    };
    let answer: Answer = serde_json::from_str(json).ok()?;
    let mut used = HashSet::new();
    let mut out = Vec::new();
    let new_text = |text: &str, sources: &[&str]| {
        let text = text.trim();
        (!text.is_empty()
            && text.chars().count() <= MAX_TIDY_CHARS
            && plain_words(text)
            && numbers_from(text, sources))
        .then(|| text.to_string())
    };
    for change in answer.changes {
        let found = match change.action.trim() {
            "merge" => {
                let ids: Vec<usize> = change
                    .memories
                    .iter()
                    .filter_map(|id| index(id, items.len()))
                    .collect();
                let [a, b] = ids[..] else { continue };
                if a == b || items[a].space != items[b].space {
                    continue;
                }
                let Some(text) = new_text(&change.text, &[&items[a].text, &items[b].text]) else {
                    continue;
                };
                TidyChange::Merge {
                    items: [a, b],
                    text,
                }
            }
            "replace" => {
                let (Some(keep), Some(drop)) = (
                    change.keep.as_deref().and_then(|id| index(id, items.len())),
                    change.drop.as_deref().and_then(|id| index(id, items.len())),
                ) else {
                    continue;
                };
                if keep == drop
                    || items[keep].space != items[drop].space
                    || items[keep].created_at <= items[drop].created_at
                {
                    continue;
                }
                TidyChange::Replace { keep, drop }
            }
            "update" => {
                let Some(item) = change
                    .memory
                    .as_deref()
                    .and_then(|id| index(id, items.len()))
                else {
                    continue;
                };
                let Some(at) = items[item]
                    .event_date
                    .filter(|at| plan_passed(*at, items[item].created_at, now))
                else {
                    continue;
                };
                let date = say_date(at);
                let Some(text) = new_text(&change.text, &[&items[item].text, &date]) else {
                    continue;
                };
                TidyChange::Update { item, text }
            }
            _ => continue,
        };
        let names: Vec<usize> = match &found {
            TidyChange::Merge { items, .. } => items.to_vec(),
            TidyChange::Replace { keep, drop } => vec![*keep, *drop],
            TidyChange::Update { item, .. } => vec![*item],
        };
        if names.iter().any(|i| used.contains(i)) {
            continue;
        }
        used.extend(names);
        out.push(found);
    }
    Some(out)
}

#[cfg(test)]
#[path = "night_tidy_test.rs"]
mod tests;
