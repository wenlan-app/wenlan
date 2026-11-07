// SPDX-License-Identifier: Apache-2.0
//! Which lines of a conversation a memory came from (UX-WHO-6).
//!
//! A memory an AI tool saved during a conversation was said, in other words,
//! in the turns just before it was saved. This picks those lines: the ones
//! said at or a moment after the save that share the most words with the
//! memory. Words are compared the way the rest of Wenlan compares them
//! ([`crate::post_ingest::overlap_terms`]: Latin words and pairs of CJK
//! characters). A line that shares too little is never picked, so when
//! nothing comes close the answer is empty rather than a guess.
//!
//! This only finds lines that say the same words. It does not check that
//! they back the memory up; the source check does that.

use wenlan_types::conversations::ConversationLine;

/// Lines picked, at most.
pub const MAX_SAID_LINES: usize = 3;
/// A line said this long after the memory was saved still counts: Claude
/// Code writes each part of a reply as its own timestamped entry, so the
/// AI's own wording of the memory can land a few seconds after the save.
pub const SAVE_SLACK_SECS: i64 = 60;
/// The least a line must share with the memory to be picked: shared words
/// over the square root of both word counts, so a long reply that mentions
/// everything does not win by length alone.
pub const MIN_SAID_SCORE: f64 = 0.2;

/// How much `line` says the same words as `memory` (0 to 1).
pub fn said_score(
    memory: &std::collections::HashSet<String>,
    line: &std::collections::HashSet<String>,
) -> f64 {
    if memory.is_empty() || line.is_empty() {
        return 0.0;
    }
    let shared = memory.intersection(line).count() as f64;
    shared / ((memory.len() * line.len()) as f64).sqrt()
}

/// The indexes of the lines `memory` came from, oldest first, at most
/// [`MAX_SAID_LINES`]. `saved_at` is when the memory was saved (Unix
/// seconds); lines said later than [`SAVE_SLACK_SECS`] after it, or with no
/// time, are not considered.
pub fn said_lines(memory: &str, saved_at: i64, lines: &[ConversationLine]) -> Vec<usize> {
    let terms = crate::post_ingest::overlap_terms(memory);
    if terms.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, f64)> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.at.is_some_and(|at| at <= saved_at + SAVE_SLACK_SECS))
        .map(|(i, line)| {
            let score = said_score(&terms, &crate::post_ingest::overlap_terms(&line.text));
            (i, score)
        })
        .filter(|(_, score)| *score >= MIN_SAID_SCORE)
        .collect();
    // Best first; a tie goes to the later line, nearer the save.
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
    let mut picked: Vec<usize> = scored
        .into_iter()
        .take(MAX_SAID_LINES)
        .map(|(i, _)| i)
        .collect();
    picked.sort_unstable();
    picked
}

#[cfg(test)]
#[path = "said_lines_test.rs"]
mod tests;
