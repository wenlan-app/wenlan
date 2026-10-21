// SPDX-License-Identifier: Apache-2.0
//! Code anchors: remember which lines a note relied on and find them again
//! after the file changes.
//!
//! A note that cites `src/lib.rs` lines 10-20 should not go stale because
//! someone added a comment on line 1, nor should it silently keep pointing at
//! lines that now hold something else. [`anchor`] fingerprints the cited lines
//! together with up to [`CONTEXT_LINES`] lines on each side, and [`relocate`]
//! takes the file's current text plus that fingerprint and says where the
//! lines went and whether their content survived.
//!
//! Pure and deterministic: the caller reads the file, nothing here touches the
//! filesystem, a database, or the network.
//!
//! # Outcomes of [`relocate`]
//!
//! | Outcome     | Meaning                                                  | How it is found                                                          |
//! |-------------|----------------------------------------------------------|--------------------------------------------------------------------------|
//! | `Unchanged` | Same content at the hinted place.                        | Hint has the same line count and the same digest.                        |
//! | `Moved`     | Same content, now at a different place.                  | Unique window whose first/last line hashes and digest match; several matches are told apart by both context hashes. |
//! | `Changed`   | Content differs; the new span sits between the old fences.| Exactly one `(start, end)` pair where the preceding context ends and the following context begins. |
//! | `Unresolved`| Not found, or more than one candidate.                   | None of the above produced exactly one answer.                           |
//!
//! A `Changed` span whose digest equals the anchor's is reported as `Moved`:
//! the content is what it was, only the place differs.
//!
//! # Canonical line form
//!
//! Text is split into lines after turning `\r\n` and `\r` into `\n`. Each line
//! is then normalized for hashing: leading and trailing whitespace is trimmed
//! and every run of internal whitespace (spaces and tabs) collapses to one
//! space. No Unicode normalization is applied. A trailing newline does not
//! create an extra empty last line, and an empty (or whitespace-only) line
//! stays an empty line, so adding or removing a blank line is still a change.
//! Because of this, re-indenting, converting a file between LF and CRLF,
//! stripping trailing spaces or respacing inside a line never changes an
//! anchor. The rule is blind to whitespace inside string literals too (`"a  b"`
//! and `"a b"` hash alike). Normalization affects hashes only: the line numbers
//! and spans reported to callers are never touched.
//!
//! Each line is hashed as the SHA-256 hex of its normalized form; a run of lines
//! is hashed as the SHA-256 hex of its normalized lines joined with `\n`. A
//! context count of 0 means the selection touches the start (or end) of the
//! file, and `Changed` then fences against the file boundary instead of a hash.
//!
//! The relocation idea follows OpenWiki's repository evidence resolver, but the
//! fingerprint is stored as plain structured fields instead of an encoded
//! version string.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How many lines of context are fingerprinted on each side of a selection.
pub const CONTEXT_LINES: u32 = 3;

/// A 1-based, inclusive span of lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LineSpan {
    /// First line of the span (1-based).
    pub start: u32,
    /// Last line of the span (1-based, inclusive).
    pub end: u32,
}

impl LineSpan {
    fn line_count(&self) -> u32 {
        self.end - self.start + 1
    }
}

/// Fingerprint of a selected line span and the lines around it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeAnchor {
    /// SHA-256 hex of the selected lines in normalized form (leading and
    /// trailing whitespace trimmed, internal whitespace runs collapsed to one
    /// space), joined with `\n`. Whitespace-only edits leave it unchanged.
    pub digest: String,
    /// Number of lines selected.
    pub selected_line_count: u32,
    /// SHA-256 hex of the first selected line.
    pub first_line_hash: String,
    /// SHA-256 hex of the last selected line.
    pub last_line_hash: String,
    /// Lines of context before the selection, `0..=CONTEXT_LINES`. Fewer than
    /// `CONTEXT_LINES` only when the selection starts near the top of the file.
    pub preceding_context_line_count: u32,
    /// SHA-256 hex of the preceding context lines joined with `\n`.
    pub preceding_context_hash: String,
    /// Lines of context after the selection, `0..=CONTEXT_LINES`. Fewer than
    /// `CONTEXT_LINES` only when the selection ends near the bottom of the file.
    pub following_context_line_count: u32,
    /// SHA-256 hex of the following context lines joined with `\n`.
    pub following_context_hash: String,
}

/// Why an anchor could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AnchorError {
    /// The span selects no lines: `end < start`, or `start` is 0 (lines are
    /// 1-based).
    #[error("line span selects no lines")]
    EmptySpan,
    /// The span reaches past the last line of the text.
    #[error("line span is outside the file ({line_count} lines)")]
    SpanOutOfRange {
        /// Number of lines the text actually has.
        line_count: u32,
    },
}

/// Where a previously anchored span is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relocation {
    /// Same content at the hinted place.
    Unchanged {
        /// The span, equal to the hint.
        span: LineSpan,
    },
    /// Same content, found uniquely elsewhere.
    Moved {
        /// The span where the content is now.
        span: LineSpan,
    },
    /// Content differs; this is the unique span fenced by the context anchors.
    Changed {
        /// The span between the old preceding and following context.
        span: LineSpan,
    },
    /// Not found, or more than one candidate.
    Unresolved,
}

/// Fingerprint `span` of `text`.
pub fn anchor(text: &str, span: LineSpan) -> Result<CodeAnchor, AnchorError> {
    if span.start == 0 || span.end < span.start {
        return Err(AnchorError::EmptySpan);
    }
    let file = CanonicalFile::new(text);
    if span.end as usize > file.lines.len() {
        return Err(AnchorError::SpanOutOfRange {
            line_count: file.line_count(),
        });
    }
    let start = span.start as usize - 1;
    let end = span.end as usize;
    let context = CONTEXT_LINES as usize;
    let before_start = start.saturating_sub(context);
    let after_end = (end + context).min(file.lines.len());
    Ok(CodeAnchor {
        digest: file.range_hash(start, end),
        selected_line_count: span.line_count(),
        first_line_hash: file.line_hashes[start].clone(),
        last_line_hash: file.line_hashes[end - 1].clone(),
        preceding_context_line_count: (start - before_start) as u32,
        preceding_context_hash: file.range_hash(before_start, start),
        following_context_line_count: (after_end - end) as u32,
        following_context_hash: file.range_hash(end, after_end),
    })
}

/// Find where the lines described by `anchor` are in `text`.
///
/// `hint` is the span recorded last time. A hint that does not fit the text is
/// ignored.
pub fn relocate(text: &str, hint: Option<LineSpan>, anchor: &CodeAnchor) -> Relocation {
    let file = CanonicalFile::new(text);
    let hint = hint.and_then(|h| file.fit(h));
    let count = anchor.selected_line_count as usize;
    if count == 0 {
        return Relocation::Unresolved;
    }

    // 1. Same content at the hinted place.
    if let Some(h) = hint {
        if h.line_count() == anchor.selected_line_count
            && file.range_hash(h.start as usize - 1, h.end as usize) == anchor.digest
        {
            return Relocation::Unchanged { span: h };
        }
    }

    // 2. Same content somewhere else.
    let mut matches: Vec<usize> = Vec::new();
    if count <= file.lines.len() {
        for start in 0..=(file.lines.len() - count) {
            if file.line_hashes[start] == anchor.first_line_hash
                && file.line_hashes[start + count - 1] == anchor.last_line_hash
                && file.range_hash(start, start + count) == anchor.digest
            {
                matches.push(start);
            }
        }
    }
    if matches.len() > 1 {
        matches.retain(|&start| file.has_context(start, start + count, anchor));
    }
    if let [start] = matches[..] {
        return found_same_content(start, count, hint);
    }

    // 3. Content changed: fence the new span between the old context lines.
    let starts = file.boundaries(
        anchor.preceding_context_line_count,
        &anchor.preceding_context_hash,
        Side::Before,
    );
    let ends = file.boundaries(
        anchor.following_context_line_count,
        &anchor.following_context_hash,
        Side::After,
    );
    let mut pairs = starts
        .iter()
        .flat_map(|&s| ends.iter().map(move |&e| (s, e)))
        .filter(|&(s, e)| e > s);
    let (Some((start, end)), None) = (pairs.next(), pairs.next()) else {
        return Relocation::Unresolved;
    };
    if file.range_hash(start, end) == anchor.digest {
        return found_same_content(start, end - start, hint);
    }
    Relocation::Changed {
        span: span_of(start, end - start),
    }
}

fn found_same_content(start: usize, count: usize, hint: Option<LineSpan>) -> Relocation {
    let span = span_of(start, count);
    if hint == Some(span) {
        Relocation::Unchanged { span }
    } else {
        Relocation::Moved { span }
    }
}

/// Zero-based start index and line count to a 1-based inclusive span.
fn span_of(start: usize, count: usize) -> LineSpan {
    LineSpan {
        start: start as u32 + 1,
        end: (start + count) as u32,
    }
}

#[derive(Clone, Copy)]
enum Side {
    Before,
    After,
}

/// A text split into normalized lines, with each line hashed once.
struct CanonicalFile {
    lines: Vec<String>,
    line_hashes: Vec<String>,
}

impl CanonicalFile {
    fn new(text: &str) -> Self {
        let unix = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut lines: Vec<String> = unix.split('\n').map(normalize_line).collect();
        // A trailing newline ends the last line; it does not start another.
        // (Empty text splits into one empty piece, which is no lines at all.)
        if unix.is_empty() || unix.ends_with('\n') {
            lines.pop();
        }
        let line_hashes = lines.iter().map(|line| sha256_hex(line)).collect();
        Self { lines, line_hashes }
    }

    fn line_count(&self) -> u32 {
        self.lines.len() as u32
    }

    /// The span if it lies inside the file, else `None`.
    fn fit(&self, span: LineSpan) -> Option<LineSpan> {
        (span.start >= 1 && span.end >= span.start && span.end as usize <= self.lines.len())
            .then_some(span)
    }

    /// Hash of lines `start..end` (zero-based, end exclusive) joined with `\n`.
    fn range_hash(&self, start: usize, end: usize) -> String {
        sha256_hex(&self.lines[start..end].join("\n"))
    }

    /// Whether both context windows around `start..end` match the anchor.
    fn has_context(&self, start: usize, end: usize, anchor: &CodeAnchor) -> bool {
        let before = anchor.preceding_context_line_count as usize;
        let after = anchor.following_context_line_count as usize;
        let before_ok = if before == 0 {
            start == 0
        } else {
            start >= before
                && self.range_hash(start - before, start) == anchor.preceding_context_hash
        };
        let after_ok = if after == 0 {
            end == self.lines.len()
        } else {
            end + after <= self.lines.len()
                && self.range_hash(end, end + after) == anchor.following_context_hash
        };
        before_ok && after_ok
    }

    /// Zero-based selection boundaries next to a context window: the index just
    /// after it (`Before`) or at its first line (`After`). Count 0 fences
    /// against the file edge.
    fn boundaries(&self, context_count: u32, context_hash: &str, side: Side) -> Vec<usize> {
        let n = context_count as usize;
        if n == 0 {
            return vec![match side {
                Side::Before => 0,
                Side::After => self.lines.len(),
            }];
        }
        if n > self.lines.len() {
            return Vec::new();
        }
        (0..=(self.lines.len() - n))
            .filter(|&at| self.range_hash(at, at + n) == context_hash)
            .map(|at| match side {
                Side::Before => at + n,
                Side::After => at,
            })
            .collect()
    }
}

/// A line as hashed: trimmed, with each run of internal whitespace collapsed to
/// a single space. Whitespace-only lines become empty.
fn normalize_line(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sha256_hex(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
#[path = "code_anchor_test.rs"]
mod code_anchor_test;
