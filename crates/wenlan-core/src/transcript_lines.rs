// SPDX-License-Identifier: Apache-2.0
//! What was said in a conversation, read from the transcript its AI tool
//! keeps (UX-FIND-1, UX-FIND-2).
//!
//! Search shows the person's and the AI's own words, not a summary of them.
//! The transcript stays where the tool keeps it; this module only reads it.
//! Only the lines people read are kept: what the person typed and what the AI
//! wrote back. Tool calls, tool output, the AI's private reasoning and the
//! messages a tool injects on its own are left out.
//!
//! A parsed transcript is remembered until its file changes, so searching
//! again does not read the file again.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use serde::Deserialize;
use wenlan_types::conversations::{ConversationLine, ConversationSpeaker};

/// A line longer than this is a pasted file or log, not something said.
const MAX_JSONL_LINE_BYTES: usize = 4 * 1024 * 1024;
/// Text kept from one message.
const MAX_LINE_CHARS: usize = 20_000;
/// Transcripts remembered at once.
const CACHE_ENTRIES: usize = 256;

type Parsed = Arc<Vec<ConversationLine>>;

struct Cached {
    modified: Option<SystemTime>,
    len: u64,
    lines: Parsed,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Cached>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The conversation's lines, oldest first. `None` when the tool's transcript
/// format is not one Wenlan reads, or the file is gone.
pub fn conversation_lines(tool: &str, path: &Path) -> Option<Parsed> {
    if tool != "claude-code" {
        return None;
    }
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let (modified, len) = (meta.modified().ok(), meta.len());
    if let Ok(cache) = cache().lock() {
        if let Some(hit) = cache.get(path) {
            if hit.modified == modified && hit.len == len {
                return Some(hit.lines.clone());
            }
        }
    }
    let file = std::fs::File::open(path).ok()?;
    let lines: Parsed = Arc::new(parse_claude_code(std::io::BufReader::new(file)));
    if let Ok(mut cache) = cache().lock() {
        if cache.len() >= CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert(
            path.to_path_buf(),
            Cached {
                modified,
                len,
                lines: lines.clone(),
            },
        );
    }
    Some(lines)
}

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default, rename = "isMeta")]
    is_meta: bool,
    #[serde(default, rename = "isSidechain")]
    is_sidechain: bool,
    #[serde(default, rename = "isCompactSummary")]
    is_compact_summary: bool,
    timestamp: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    content: Option<Content>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Content {
    Text(String),
    Blocks(Vec<Block>),
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
}

/// Claude Code's JSONL transcript: one event per line.
pub fn parse_claude_code(reader: impl BufRead) -> Vec<ConversationLine> {
    let mut out = Vec::new();
    for raw in reader.split(b'\n') {
        let Ok(raw) = raw else { break };
        if raw.len() > MAX_JSONL_LINE_BYTES || !looks_like_message(&raw) {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<Entry>(&raw) else {
            continue;
        };
        if let Some(line) = said(entry) {
            out.push(line);
        }
    }
    out
}

/// Skips the events that cannot be a message (titles, file snapshots, tool
/// bookkeeping) without parsing them.
fn looks_like_message(raw: &[u8]) -> bool {
    const USER: &[u8] = br#""type":"user""#;
    const ASSISTANT: &[u8] = br#""type":"assistant""#;
    raw.windows(USER.len()).any(|w| w == USER)
        || raw.windows(ASSISTANT.len()).any(|w| w == ASSISTANT)
}

fn said(entry: Entry) -> Option<ConversationLine> {
    if entry.is_meta || entry.is_sidechain || entry.is_compact_summary {
        return None;
    }
    let speaker = match entry.kind.as_deref()? {
        "user" => ConversationSpeaker::You,
        "assistant" => ConversationSpeaker::Ai,
        _ => return None,
    };
    let texts: Vec<String> = match entry.message?.content? {
        Content::Text(text) => vec![text],
        Content::Blocks(blocks) => blocks
            .into_iter()
            .filter(|b| b.kind.as_deref() == Some("text"))
            .filter_map(|b| b.text)
            .collect(),
    };
    let text = texts
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty() && (speaker != ConversationSpeaker::You || !injected(t)))
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.is_empty() {
        return None;
    }
    let text = if text.chars().count() > MAX_LINE_CHARS {
        let mut cut: String = text.chars().take(MAX_LINE_CHARS - 1).collect();
        cut.push('…');
        cut
    } else {
        text
    };
    Some(ConversationLine {
        speaker,
        text,
        at: entry.timestamp.as_deref().and_then(unix_seconds),
    })
}

/// Text the tool put in the person's turn on its own: slash-command echoes,
/// command output, reminders, interruption notes.
fn injected(text: &str) -> bool {
    if text.starts_with("[Request interrupted") || text.starts_with("Caveat: ") {
        return true;
    }
    let Some(rest) = text.strip_prefix('<') else {
        return false;
    };
    let tag: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    !tag.is_empty() && rest[tag.len()..].starts_with(['>', ' ', '\n'])
}

fn unix_seconds(timestamp: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|t| t.timestamp())
}

/// The search words, lowercased. Whitespace separates them; text without
/// spaces (Chinese, Japanese) is one word.
pub fn search_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|t| !t.is_empty())
        .take(8)
        .collect()
}

/// Whether every search word is in `text`.
pub fn line_matches(text: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    terms.iter().all(|t| lower.contains(t.as_str()))
}

/// At most `max` characters of `text` around the first search word, with
/// "…" where text was cut.
pub fn excerpt(text: &str, terms: &[String], max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    // Lowercasing can change length for a few scripts; fall back to the start.
    let at = if lower.len() == chars.len() {
        terms
            .first()
            .and_then(|t| find_chars(&lower, &t.chars().collect::<Vec<_>>()))
            .unwrap_or(0)
    } else {
        0
    };
    let start = at.saturating_sub(max / 3).min(chars.len() - max);
    let end = (start + max).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

fn find_chars(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| haystack[i..i + needle.len()] == *needle)
}

/// The start of `text`, at most `max` characters.
pub fn head(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// The end of `text`, at most `max` characters.
pub fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let mut out = String::from("…");
    out.extend(text.chars().skip(count - max));
    out
}

#[cfg(test)]
#[path = "transcript_lines_test.rs"]
mod tests;
