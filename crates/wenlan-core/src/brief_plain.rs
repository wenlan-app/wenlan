// SPDX-License-Identifier: Apache-2.0
//! A Space Brief said for people. The Brief is the note one AI session leaves
//! the next, so its items carry PR numbers, paths and long detail. A Space's
//! page shows it on top (docs/ux/spec.md, UX-SPACE-2): the night writes one
//! plain line for where the work stands and one for each active item, in the
//! person's language, and closes an item a newer note in the Space says is
//! done. The Brief text itself stays as the AI wrote it. Pure: the prompt and
//! the checks on the model's answer.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use wenlan_types::Brief;

use crate::conversation_intake::plain_words;

/// Longest plain line, in characters.
pub const MAX_LINE_CHARS: usize = 80;
/// Shortest quote that may close an item.
pub const MIN_QUOTE_CHARS: usize = 8;
/// Most evidence lines offered.
pub const MAX_EVIDENCE: usize = 40;
/// Longest evidence line, in characters.
pub const MAX_EVIDENCE_CHARS: usize = 300;
/// Longest Brief item text offered, in characters.
pub const MAX_ITEM_CHARS: usize = 600;

/// The plain lines of one Space's Brief, kept beside it in app metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BriefPlain {
    /// The Brief version these lines were written for.
    pub brief_version: i64,
    /// The language they are in, as `conversation_intake::language_name`.
    pub language: String,
    /// Where the work stands, in a sentence or two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// One line per active item, by item id.
    #[serde(default)]
    pub lines: BTreeMap<String, String>,
    /// When the night wrote them, Unix seconds.
    pub written_at: i64,
}

/// The app-metadata key for one Space's plain lines.
pub fn metadata_key(space_id: &str) -> String {
    format!("brief_plain_v1:{space_id}")
}

/// Lines written for another version or language are written again.
pub fn is_current(plain: Option<&BriefPlain>, brief: &Brief, language: &str) -> bool {
    plain.is_some_and(|p| p.brief_version == brief.version && p.language == language)
}

/// An item the answer closes, with the words in a newer note that say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub item_id: String,
    pub quote: String,
}

/// What a checked answer says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlainAnswer {
    pub summary: Option<String>,
    pub lines: BTreeMap<String, String>,
    pub done: Vec<Done>,
}

fn clip(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        text
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

/// The prompt for one Space. `evidence` is what was written in the Space
/// since its oldest active item, newest first.
pub fn plain_prompt(brief: &Brief, evidence: &[String], language: &str) -> String {
    let mut items = String::new();
    for (i, item) in brief.active.iter().enumerate() {
        items.push_str(&format!("I{i}: {}\n", clip(&item.text, MAX_ITEM_CHARS)));
    }
    let mut notes = String::new();
    for line in evidence.iter().take(MAX_EVIDENCE) {
        notes.push_str(&format!("- {}\n", clip(line, MAX_EVIDENCE_CHARS)));
    }
    if notes.is_empty() {
        notes.push_str("(none)\n");
    }
    format!(
        "A project's handoff note, written by one AI session for the next. The person who owns \
the project will read it on the project's page.\n\n\
Where the work stands:\n{}\n\n\
What is in progress:\n{items}\n\
Newer notes from the same project, newest first:\n{notes}\n\
Write for the person, in {language}:\n\
- \"summary\": where the work stands, in one or two short sentences.\n\
- \"items\": for each item still in progress, one short line saying what it is, like a to-do \
a person would write. At most {MAX_LINE_CHARS} characters.\n\
- \"done\": an item a newer note clearly says is finished, merged, shipped or dropped. Give \
the words of that note that say so, copied exactly. Leave out anything you are not sure of.\n\
Plain words only: no PR or issue numbers, version numbers, hashes, paths, file names, branch names or code.\n\
Answer with JSON only: {{\"summary\": string, \"items\": [{{\"item\": \"I0\", \"line\": \
string}}], \"done\": [{{\"item\": \"I1\", \"quote\": string}}]}}",
        clip(&brief.last_session_summary, MAX_ITEM_CHARS)
    )
}

fn index(id: &str, len: usize) -> Option<usize> {
    id.trim()
        .strip_prefix('I')
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| *n < len)
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<String>().to_lowercase()
}

fn plain_line(text: &str, max: usize) -> Option<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty() && text.chars().count() <= max && plain_words(&text)).then_some(text)
}

/// The answer's lines and closings that pass the checks: a line must be
/// short and in plain words, and an item is closed only when its quote is
/// found, word for word, in one of the evidence lines. `None` when the
/// answer is not JSON.
pub fn parse_plain(text: &str, brief: &Brief, evidence: &[String]) -> Option<PlainAnswer> {
    #[derive(Deserialize)]
    struct Line {
        #[serde(default)]
        item: String,
        #[serde(default)]
        line: String,
    }
    #[derive(Deserialize)]
    struct Closed {
        #[serde(default)]
        item: String,
        #[serde(default)]
        quote: String,
    }
    #[derive(Deserialize)]
    struct Answer {
        #[serde(default)]
        summary: String,
        #[serde(default)]
        items: Vec<Line>,
        #[serde(default)]
        done: Vec<Closed>,
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
    let len = brief.active.len();
    let offered: Vec<String> = evidence
        .iter()
        .take(MAX_EVIDENCE)
        .map(|line| squash(&clip(line, MAX_EVIDENCE_CHARS)))
        .collect();
    let mut done = Vec::new();
    let mut closed = HashSet::new();
    for item in answer.done {
        let Some(i) = index(&item.item, len) else {
            continue;
        };
        let quote = squash(item.quote.trim_end_matches('…'));
        if quote.chars().count() < MIN_QUOTE_CHARS
            || !offered.iter().any(|line| line.contains(&quote))
            || !closed.insert(i)
        {
            continue;
        }
        done.push(Done {
            item_id: brief.active[i].id.clone(),
            quote: item.quote.trim().to_string(),
        });
    }
    let mut lines = BTreeMap::new();
    for line in answer.items {
        let Some(i) = index(&line.item, len) else {
            continue;
        };
        if closed.contains(&i) {
            continue;
        }
        if let Some(text) = plain_line(&line.line, MAX_LINE_CHARS) {
            lines.entry(brief.active[i].id.clone()).or_insert(text);
        }
    }
    Some(PlainAnswer {
        summary: plain_line(&answer.summary, MAX_LINE_CHARS * 3),
        lines,
        done,
    })
}

#[cfg(test)]
#[path = "brief_plain_test.rs"]
mod tests;
