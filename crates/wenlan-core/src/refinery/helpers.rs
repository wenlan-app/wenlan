// SPDX-License-Identifier: Apache-2.0
//! Shared text-classifier helpers used by the refinery pipeline.
//!
//! These predicates detect low-quality or structurally-invalid page titles
//! (generic tokens, markup artifacts, UUIDs, code snippets, paths, etc.) so
//! the refinery can reject or sanitize them before storing.

/// Tokens considered generic stand-ins. A title made entirely of these is not
/// useful as a page title. Mostly English; a small set of CJK generics
/// included for the same reason. Curated to avoid false positives —
/// `concept`, `concepts`, `content`, `ideas` deliberately excluded because
/// they appear in legitimate titles too often.
const GENERIC_TOKENS: &[&str] = &[
    "general",
    "various",
    "miscellaneous",
    "topic",
    "topics",
    "notes",
    "things",
    "items",
    "stuff",
    "misc",
    "other",
    "unknown",
    "untitled",
    "random",
    "assorted",
    "cluster",
    "clusters",
    // CJK generics seen in real LLM output
    "杂项",
    "其他",
    "其它",
    "通用",
    "笔记",
    "主题",
];

/// Returns true when every word-token of the input (after splitting on
/// non-alphanumeric separators and lowercasing) is in GENERIC_TOKENS. Used to
/// reject LLM-produced titles like "General topic" or "Various Notes" or
/// "Misc-things" (hyphen treated as separator).
pub(crate) fn is_all_generic_tokens(s: &str) -> bool {
    let words: Vec<&str> = s
        .split(|c: char| !c.is_alphanumeric())
        .map(|w| w.trim())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return false;
    }
    words
        .iter()
        .all(|w| GENERIC_TOKENS.contains(&w.to_lowercase().as_str()))
}

/// Returns true when the title contains markdown formatting or document-content
/// punctuation that shouldn't appear in clean titles. Catches LLM hallucinations
/// like `**Roland** — 太正統，d-L 連接快` where the model emitted markdown-styled
/// document content instead of a title. Also catches wikilink brackets and
/// heading markers that leak in from training data of Markdown corpora.
pub(crate) fn looks_like_markup_styled(s: &str) -> bool {
    let trimmed = s.trim();
    // Markdown emphasis (bold/italic/strikethrough)
    trimmed.contains("**")
        || trimmed.contains("__")
        || trimmed.contains("~~")
        // Wikilink brackets
        || trimmed.contains("[[")
        || trimmed.contains("]]")
        // Em-dash separator (en-dash and ASCII hyphen are fine)
        || trimmed.contains('—')
        // Heading markers at start
        || trimmed.starts_with('#')
}

pub(crate) fn looks_like_uuid(s: &str) -> bool {
    // e.g. 5b064ab2-8919-48b2-8220-8f7680b426dd
    let trimmed = s.trim();
    trimmed.len() >= 32
        && trimmed.chars().filter(|c| *c == '-').count() >= 3
        && trimmed.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

pub(crate) fn looks_like_short_hash(s: &str) -> bool {
    // e.g. e554534 (commit SHA prefix) as sole title or lead token
    let first = s.split_whitespace().next().unwrap_or("");
    (7..=12).contains(&first.len())
        && first.chars().all(|c| c.is_ascii_hexdigit())
        && first.chars().any(|c| c.is_ascii_digit())
}

pub(crate) fn looks_like_code(s: &str) -> bool {
    let lowered = s.trim_start().to_lowercase();
    lowered.starts_with("const ")
        || lowered.starts_with("let ")
        || lowered.starts_with("var ")
        || lowered.starts_with("await ")
        || lowered.starts_with("function ")
        || lowered.starts_with("import ")
        || lowered.starts_with("fn ")
        || s.contains("=>")
        || s.contains("{ where:")
        || s.contains("findUnique")
}

pub(crate) fn looks_like_path(s: &str) -> bool {
    let trimmed = s.trim_start();
    (trimmed.starts_with('[')
        && (trimmed.contains(".md") || trimmed.contains("::") || has_dated_segment(trimmed)))
        || trimmed.starts_with('/')
        || trimmed.starts_with("~/")
        || trimmed.starts_with("\\\\")
        || starts_with_drive_letter(trimmed)
        || trimmed.contains("/Users/")
        || trimmed.contains("/home/")
        || has_dated_segment(trimmed)
}

/// A path segment that starts with a date such as `/2031-04-02`, for any year
/// from 2000 to 2099.
fn has_dated_segment(s: &str) -> bool {
    s.as_bytes().windows(6).any(|w| {
        w[0] == b'/'
            && w[1] == b'2'
            && w[2] == b'0'
            && w[3].is_ascii_digit()
            && w[4].is_ascii_digit()
            && w[5] == b'-'
    })
}

/// A Windows drive path such as `C:\` or `d:/`.
fn starts_with_drive_letter(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

pub(crate) fn looks_like_commit_message(s: &str) -> bool {
    let trimmed = s.trim_start();
    let lowered = trimmed.to_lowercase();
    let plain_prefixes = [
        "feat:",
        "fix:",
        "chore:",
        "docs:",
        "refactor:",
        "test:",
        "style:",
        "perf:",
        "ci:",
        "build:",
        "revert:",
    ];
    if plain_prefixes.iter().any(|p| lowered.starts_with(p)) {
        return true;
    }
    // Conventional commits with scope: feat(area): ...
    if let Some(open) = trimmed.find('(') {
        if let Some(colon_close) = trimmed[open..].find("):") {
            let _ = colon_close;
            let prefix_raw = trimmed[..open].to_lowercase();
            let prefix_clean = prefix_raw.trim_end_matches(':');
            if plain_prefixes
                .iter()
                .any(|p| prefix_clean == p.trim_end_matches(':'))
            {
                return true;
            }
        }
    }
    false
}

/// Strip a leading bracketed source-ID prefix like `[obs/unix/2026-03-17]`,
/// `[mem_abc123]`, `[5b064ab2-8919-48b2]` from content before title generation.
/// Only strips if the bracket content has no spaces and looks like a source token
/// (contains slash, underscore, double-colon, or is all hex/hyphens).
pub(crate) fn strip_source_prefix(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with('[') {
        return content;
    }
    if let Some(end) = trimmed.find(']') {
        let inside = &trimmed[1..end];
        if !inside.contains(' ')
            && (inside.contains('/')
                || inside.contains('_')
                || inside.contains("::")
                || inside.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
        {
            return trimmed[end + 1..].trim_start();
        }
    }
    content
}
