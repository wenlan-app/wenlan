// SPDX-License-Identifier: Apache-2.0
//! Who wrote each sentence of a page, derived from page history (pure).
//!
//! Nothing here is stored as truth. The truth is `page_history` (every
//! version's text and its `edited_by`) plus the `sentence.accept` entries in
//! `sentence_history`; [`derive_authorship`] replays them into the rows of the
//! spec's `authorship(page_id, version, idx, hash, author, since_version,
//! confidence)` cache, with `origin` as the separate evidentiary column
//! (storage migration spec 2026-10-31, section 4).
//!
//! Rules, in the order the derivation applies them:
//!
//! - A page body splits into sentence units: every line, then
//!   [`crate::faithfulness::sentence_spans`] inside the line (the sentence
//!   boundary the citation checker uses), refined so a common abbreviation's
//!   full stop (`a.m.`) does not end a sentence and Chinese or Japanese
//!   `。！？` does.
//! - Each version is aligned with the one before it by LCS over normalized
//!   units (citation markers removed, whitespace collapsed), so re-wrapping
//!   or re-citing a sentence keeps its author. A unit that reappears with the
//!   same text elsewhere was moved and keeps its author too.
//! - A new or changed unit takes the author of the version that wrote it.
//!   A person's one-character edit to an AI sentence makes it the person's,
//!   but its `origin` stays AI, so it keeps the citation requirement.
//! - Versions whose writer is unknown (the `migration_87` seed of every page
//!   that predates history, or a writer string this code does not know) give
//!   [`SentenceAuthor::Unknown`] with uncertain confidence. Unknown sentences
//!   are protected like the person's whenever the page is human-owned or
//!   review-locked ([`is_protected`]).
//! - Mechanical rewrites (a link rewrite after a rename, an archive) keep the
//!   author of the sentence they touched.
//! - Imported text from another tool is [`SentenceAuthor::Original`]: never
//!   the person's, and never rewritten by a machine.

use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};
use wenlan_types::authorship::{AuthorshipConfidence, SentenceAuthor, SentenceAuthorship};

/// One sentence unit of a page body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentenceUnit {
    /// Byte range of the trimmed unit in the body.
    pub start: usize,
    pub end: usize,
    /// The unit as written, trimmed.
    pub text: String,
    /// The comparison form: markers removed, whitespace collapsed.
    pub norm: String,
    pub hash: String,
    /// The unit is the only unit on its line.
    pub whole_line: bool,
    /// A blank line precedes the unit's line.
    pub blank_before: bool,
}

/// The comparison form of a sentence.
pub fn normalize_sentence(text: &str) -> String {
    crate::citations::strip_markers(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Hash of a normalized sentence; how proposals, flags and rechecks address
/// one sentence.
pub fn sentence_hash(norm: &str) -> String {
    let digest = Sha256::digest(norm.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Split a page body into sentence units, in body order.
pub fn sentence_units(body: &str) -> Vec<SentenceUnit> {
    let mut units = Vec::new();
    let mut offset = 0;
    let mut previous_blank = false;
    for raw_line in body.split_inclusive('\n') {
        let line = raw_line.trim_end_matches(['\n', '\r']);
        let line_start = offset;
        offset += raw_line.len();
        if line.trim().is_empty() {
            previous_blank = true;
            continue;
        }
        let mut line_units = Vec::new();
        for (start, end) in line_unit_ranges(line) {
            let slice = &line[start..end];
            let trimmed = slice.trim();
            if !trimmed.chars().any(char::is_alphanumeric) {
                continue;
            }
            let lead = slice.len() - slice.trim_start().len();
            let unit_start = line_start + start + lead;
            let norm = normalize_sentence(trimmed);
            if norm.is_empty() {
                continue;
            }
            line_units.push(SentenceUnit {
                start: unit_start,
                end: unit_start + trimmed.len(),
                text: trimmed.to_string(),
                hash: sentence_hash(&norm),
                norm,
                whole_line: false,
                blank_before: previous_blank,
            });
        }
        let whole_line = line_units.len() == 1;
        for mut unit in line_units {
            unit.whole_line = whole_line;
            units.push(unit);
        }
        previous_blank = false;
    }
    units
}

/// Byte ranges of one line's sentences, each running from where its sentence
/// starts to where the next one starts (so it keeps its own terminal
/// punctuation and the citation markers after it).
///
/// The boundary is [`crate::faithfulness::sentence_spans`], run on the line
/// with its `[N]` markers removed, exactly as the citation checker does: a
/// marker sits between a sentence's full stop and the space after it, which
/// would otherwise hide the boundary.
fn line_unit_ranges(line: &str) -> Vec<(usize, usize)> {
    let markers = crate::citations::marker_spans(line);
    let mut bare = String::with_capacity(line.len());
    // `raw_at[b]` is the byte of `line` that byte `b` of `bare` came from.
    let mut raw_at = Vec::with_capacity(line.len() + 1);
    let mut next_marker = markers.iter().peekable();
    let mut i = 0;
    while i < line.len() {
        if let Some(&&(start, end)) = next_marker.peek() {
            if i == start {
                i = end;
                next_marker.next();
                continue;
            }
        }
        let ch = line[i..].chars().next().expect("in bounds");
        for k in 0..ch.len_utf8() {
            raw_at.push(i + k);
        }
        bare.push(ch);
        i += ch.len_utf8();
    }
    raw_at.push(line.len());
    let starts: Vec<usize> = sentence_starts(&bare)
        .into_iter()
        .map(|start| raw_at[start])
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(k, &start)| (start, starts.get(k + 1).copied().unwrap_or(line.len())))
        .filter(|(start, end)| start < end)
        .collect()
}

/// Where each sentence of `bare` (one line, markers removed) starts.
///
/// The base boundary is [`crate::faithfulness::sentence_spans`]. Two
/// refinements apply to sentence units only, because the citation checker
/// and claim identity pin that boundary's offsets:
///
/// - A full stop that ends a common abbreviation (`a.m.`, `e.g.`, `Dr.`)
///   does not end the sentence.
/// - Chinese and Japanese end sentences with `。！？` and need no space
///   after them, so each one (plus any closing quote or bracket after it)
///   ends a sentence too.
fn sentence_starts(bare: &str) -> Vec<usize> {
    let mut starts: Vec<usize> = crate::faithfulness::sentence_spans(bare)
        .into_iter()
        .map(|(start, _)| start)
        .filter(|&start| start == 0 || !ends_with_abbreviation(&bare[..start]))
        .collect();
    starts.extend(cjk_sentence_starts(bare));
    starts.sort_unstable();
    starts.dedup();
    starts
}

/// Words whose full stop does not end a sentence, lowercase, without the
/// final `.`.
const ABBREVIATIONS: &[&str] = &[
    "a.m", "p.m", "e.g", "i.e", "vs", "mr", "mrs", "ms", "dr", "prof", "st", "jr", "sr", "fig",
    "approx", "u.s", "u.k",
];

/// `before` (the text up to a sentence start) ends with an abbreviation and
/// its full stop, then whitespace.
fn ends_with_abbreviation(before: &str) -> bool {
    let trimmed = before.trim_end();
    let Some(word) = trimmed.strip_suffix('.') else {
        return false;
    };
    let token = word
        .rsplit(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or("")
        .to_lowercase();
    ABBREVIATIONS.contains(&token.as_str())
}

/// Sentence starts after `。！？` (and any closing quotes or brackets right
/// after them) that the ASCII boundary does not see.
fn cjk_sentence_starts(bare: &str) -> Vec<usize> {
    const ENDS: &[char] = &['。', '！', '？'];
    const CLOSERS: &[char] = &['」', '』', '”', '’', '）', ')', '】', '"', '\''];
    let mut starts = Vec::new();
    let mut chars = bare.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if !ENDS.contains(&ch) {
            continue;
        }
        while let Some(&(_, next)) = chars.peek() {
            if ENDS.contains(&next) || CLOSERS.contains(&next) || next.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        if let Some(&(start, _)) = chars.peek() {
            starts.push(start);
        }
    }
    starts
}

/// One step of an alignment between an old and a new unit list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignOp {
    Equal(usize, usize),
    /// The old unit at this position was replaced by the new one.
    Replace(usize, usize),
    Delete(usize),
    Insert(usize),
}

/// Above this many cells the middle section is matched greedily instead of by
/// a full LCS table.
const LCS_CELL_LIMIT: usize = 4_000_000;

/// Align `old` with `new` by longest common subsequence. In each unmatched run
/// between two matches, a unit that is a touch-up of an old one (word overlap
/// at least [`EDIT_SIMILARITY`]) is a replacement; the rest are deletions and
/// insertions.
pub fn align(old: &[&str], new: &[&str]) -> Vec<AlignOp> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let old_mid = &old[prefix..old.len() - suffix];
    let new_mid = &new[prefix..new.len() - suffix];

    let mut matches: Vec<(usize, usize)> = (0..prefix).map(|i| (i, i)).collect();
    for (i, j) in lcs_pairs(old_mid, new_mid) {
        matches.push((prefix + i, prefix + j));
    }
    for k in 0..suffix {
        matches.push((old.len() - suffix + k, new.len() - suffix + k));
    }

    let mut ops = Vec::with_capacity(old.len().max(new.len()));
    let (mut oi, mut ni) = (0, 0);
    for (o, n) in matches {
        push_gap(&mut ops, old, new, oi..o, ni..n);
        ops.push(AlignOp::Equal(o, n));
        oi = o + 1;
        ni = n + 1;
    }
    push_gap(&mut ops, old, new, oi..old.len(), ni..new.len());
    ops
}

/// The unmatched run between two matches. An old unit pairs, in order, with
/// the first remaining new unit similar to it (an edit of it); the others are
/// deletions, emitted before the insertions they sit next to, so whatever
/// they are put back next to keeps its old position.
fn push_gap(
    ops: &mut Vec<AlignOp>,
    old: &[&str],
    new: &[&str],
    o: std::ops::Range<usize>,
    n: std::ops::Range<usize>,
) {
    let mut next_new = n.start;
    let mut next_old = o.start;
    for old_index in o.clone() {
        let Some(found) = (next_new..n.end).find(|&k| similar(old[old_index], new[k])) else {
            continue;
        };
        for deleted in next_old..old_index {
            ops.push(AlignOp::Delete(deleted));
        }
        for inserted in next_new..found {
            ops.push(AlignOp::Insert(inserted));
        }
        ops.push(AlignOp::Replace(old_index, found));
        next_old = old_index + 1;
        next_new = found + 1;
    }
    for deleted in next_old..o.end {
        ops.push(AlignOp::Delete(deleted));
    }
    for inserted in next_new..n.end {
        ops.push(AlignOp::Insert(inserted));
    }
}

fn lcs_pairs(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    if old.is_empty() || new.is_empty() {
        return Vec::new();
    }
    if old.len().saturating_mul(new.len()) > LCS_CELL_LIMIT {
        // Greedy in-order matching keeps the result monotonic and bounded.
        let mut pairs = Vec::new();
        let mut from = 0;
        for (i, unit) in old.iter().enumerate() {
            if let Some(found) = new[from..].iter().position(|n| n == unit) {
                pairs.push((i, from + found));
                from += found + 1;
            }
        }
        return pairs;
    }
    let (n, m) = (old.len(), new.len());
    let mut table = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * (m + 1) + j] = if old[i] == new[j] {
                table[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                table[(i + 1) * (m + 1) + j].max(table[i * (m + 1) + j + 1])
            };
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if old[i] == new[j] {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if table[(i + 1) * (m + 1) + j] >= table[i * (m + 1) + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// Who a page version's writer is, for authorship.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionAuthor {
    Person,
    Ai,
    Unknown,
    Original,
    /// A mechanical rewrite: the sentences it touched keep their author.
    System,
}

/// Classify a `page_history.edited_by` value. `creation_kind` decides who
/// wrote a page's first version (`create`); the caller passes something
/// other than `authored` for an authored page no person has touched (see
/// `sentences::birth_kind`).
pub fn version_author(edited_by: &str, creation_kind: &str) -> VersionAuthor {
    match edited_by {
        // A person in the app or the vault, a card or proposal the person
        // accepted, a version the person restored, a draft the person
        // published.
        "manual_edit" | "fs_edit" | "revision_accept" | "restore" | "publish" => {
            VersionAuthor::Person
        }
        "create" => match creation_kind {
            "authored" => VersionAuthor::Person,
            "original" => VersionAuthor::Original,
            _ => VersionAuthor::Ai,
        },
        "original" | "chat_import" => VersionAuthor::Original,
        "distill" | "re_distill" | "agent_refresh" | "page_growth" | "refinery_merge"
        | "citation_backfill" | "page_merge" => VersionAuthor::Ai,
        "link_rename" | "title_rename" | "archive" => VersionAuthor::System,
        // `migration_87` (the seed of every page that predates history),
        // `migration_84`, and any writer this code does not know.
        _ => VersionAuthor::Unknown,
    }
}

/// May a machine change this sentence without the person's say? `guarded` is
/// true when the page is human-owned or review-locked.
pub fn is_protected(author: SentenceAuthor, guarded: bool) -> bool {
    match author {
        SentenceAuthor::Person | SentenceAuthor::Original => true,
        SentenceAuthor::Unknown => guarded,
        SentenceAuthor::Ai => false,
    }
}

/// One page version as the derivation reads it, oldest first.
#[derive(Debug, Clone, Copy)]
pub struct VersionText<'a> {
    pub version: i64,
    pub edited_by: &'a str,
    pub content: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Attribution {
    author: SentenceAuthor,
    origin: SentenceAuthor,
    since: i64,
    confidence: AuthorshipConfidence,
    accepted: bool,
}

/// Word overlap at or above which a replaced sentence counts as an edit of the
/// sentence it replaced (and so keeps that sentence's origin).
const EDIT_SIMILARITY: f64 = 0.5;

fn words(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut word = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() && !is_cjk(c) {
            word.extend(c.to_lowercase());
            continue;
        }
        if !word.is_empty() {
            out.insert(std::mem::take(&mut word));
        }
        if is_cjk(c) {
            out.insert(c.to_string());
        }
    }
    if !word.is_empty() {
        out.insert(word);
    }
    out
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

fn similar(a: &str, b: &str) -> bool {
    let (a, b) = (words(a), words(b));
    let all = a.union(&b).count();
    all > 0 && a.intersection(&b).count() as f64 / all as f64 >= EDIT_SIMILARITY
}

/// How many sentence units `after` added, changed or removed against
/// `before`.
pub fn changed_sentence_count(before: &str, after: &str) -> u32 {
    let old = sentence_units(before);
    let new = sentence_units(after);
    let old_norms: Vec<&str> = old.iter().map(|u| u.norm.as_str()).collect();
    let new_norms: Vec<&str> = new.iter().map(|u| u.norm.as_str()).collect();
    let changed = align(&old_norms, &new_norms)
        .into_iter()
        .filter(|op| !matches!(op, AlignOp::Equal(..)))
        .count();
    u32::try_from(changed).unwrap_or(u32::MAX)
}

/// Replay `versions` (oldest first) into the authorship of the last one.
/// `accepted` maps a version to the sentence hashes that version accepted
/// from a proposal; `restored` to the hashes an undo of an AI write put back
/// in that version, which keep the attribution they last had. `truncated`
/// says the oldest version given is not the page's first: its sentences are
/// then of unknown origin.
pub fn derive_authorship(
    page_id: &str,
    creation_kind: &str,
    versions: &[VersionText<'_>],
    accepted: &HashMap<i64, HashSet<String>>,
    restored: &HashMap<i64, HashSet<String>>,
    truncated: bool,
) -> Vec<SentenceAuthorship> {
    let mut previous: Vec<(SentenceUnit, Attribution)> = Vec::new();
    // The attribution each sentence text last had, for an undo's put-back.
    let mut last_seen: HashMap<String, Attribution> = HashMap::new();
    let mut last_version = 0;
    for (position, version) in versions.iter().enumerate() {
        last_version = version.version;
        let writer = if position == 0 && truncated {
            VersionAuthor::Unknown
        } else {
            version_author(version.edited_by, creation_kind)
        };
        let units = sentence_units(version.content);
        let old_norms: Vec<&str> = previous.iter().map(|(u, _)| u.norm.as_str()).collect();
        let new_norms: Vec<&str> = units.iter().map(|u| u.norm.as_str()).collect();
        let ops = align(&old_norms, &new_norms);

        let mut removed: HashMap<&str, Attribution> = HashMap::new();
        for op in &ops {
            if let AlignOp::Delete(o) | AlignOp::Replace(o, _) = *op {
                removed.entry(old_norms[o]).or_insert(previous[o].1);
            }
        }
        let accepted_here = accepted.get(&version.version);
        let restored_here = restored.get(&version.version);
        let mut next: Vec<Option<Attribution>> = vec![None; units.len()];
        for op in &ops {
            let (n, replaced) = match *op {
                AlignOp::Equal(o, n) => {
                    next[n] = Some(previous[o].1);
                    continue;
                }
                AlignOp::Delete(_) => continue,
                AlignOp::Insert(n) => (n, None),
                AlignOp::Replace(o, n) => (n, Some(o)),
            };
            if let Some(moved) = removed.get(new_norms[n]) {
                next[n] = Some(*moved);
                continue;
            }
            let edited = replaced
                .filter(|o| similar(old_norms[*o], new_norms[n]))
                .map(|o| previous[o].1);
            next[n] = Some(attribute(
                writer,
                version.version,
                edited,
                replaced.map(|o| previous[o].1),
            ));
        }
        let mut current = Vec::with_capacity(units.len());
        for (unit, attribution) in units.into_iter().zip(next) {
            let mut attribution = attribution.unwrap_or(Attribution {
                author: SentenceAuthor::Unknown,
                origin: SentenceAuthor::Unknown,
                since: version.version,
                confidence: AuthorshipConfidence::Uncertain,
                accepted: false,
            });
            if restored_here.is_some_and(|hashes| hashes.contains(&unit.hash)) {
                if let Some(before) = last_seen.get(&unit.norm) {
                    attribution = *before;
                }
            }
            if accepted_here.is_some_and(|hashes| hashes.contains(&unit.hash)) {
                attribution = Attribution {
                    author: SentenceAuthor::Person,
                    origin: SentenceAuthor::Ai,
                    since: version.version,
                    confidence: AuthorshipConfidence::Certain,
                    accepted: true,
                };
            }
            current.push((unit, attribution));
        }
        for (unit, attribution) in &current {
            last_seen.insert(unit.norm.clone(), *attribution);
        }
        previous = current;
    }
    previous
        .into_iter()
        .enumerate()
        .map(|(idx, (unit, a))| SentenceAuthorship {
            page_id: page_id.to_string(),
            version: last_version,
            idx: u32::try_from(idx).unwrap_or(u32::MAX),
            hash: unit.hash,
            author: a.author,
            since_version: a.since,
            confidence: a.confidence,
            origin: a.origin,
            accepted: a.accepted,
        })
        .collect()
}

/// The attribution of a unit `writer` added or changed at `version`. `edited`
/// is the attribution of the sentence it is a touch-up of, `replaced` the one
/// of whatever stood in its place.
fn attribute(
    writer: VersionAuthor,
    version: i64,
    edited: Option<Attribution>,
    replaced: Option<Attribution>,
) -> Attribution {
    let fresh = |author: SentenceAuthor, confidence| Attribution {
        author,
        origin: author,
        since: version,
        confidence,
        accepted: false,
    };
    match writer {
        VersionAuthor::System => match replaced {
            Some(previous) => Attribution {
                since: version,
                ..previous
            },
            None => fresh(SentenceAuthor::Unknown, AuthorshipConfidence::Uncertain),
        },
        VersionAuthor::Person => match edited {
            // A touch-up: the sentence is the person's now, but what it says
            // came from whoever wrote it first.
            Some(previous) => Attribution {
                author: SentenceAuthor::Person,
                origin: previous.origin,
                since: version,
                confidence: AuthorshipConfidence::Certain,
                accepted: false,
            },
            None => fresh(SentenceAuthor::Person, AuthorshipConfidence::Certain),
        },
        VersionAuthor::Ai => fresh(SentenceAuthor::Ai, AuthorshipConfidence::Certain),
        VersionAuthor::Original => fresh(SentenceAuthor::Original, AuthorshipConfidence::Certain),
        VersionAuthor::Unknown => fresh(SentenceAuthor::Unknown, AuthorshipConfidence::Uncertain),
    }
}

/// A change a machine write wanted to make to a protected sentence, staged
/// instead of written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedProposal {
    pub sentence_hash: String,
    pub before: String,
    /// Empty when the write deleted the sentence.
    pub after: String,
    /// Why the facts behind the sentence changed. The planner leaves it
    /// empty; `sentences::sentence_gate` fills it and drops every proposal
    /// that has none, so a rewording alone is never proposed.
    pub reason: Option<wenlan_types::authorship::WeakReason>,
}

/// What lands of a machine write once the protected sentences are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatePlan {
    /// The machine's body with every protected sentence it changed or dropped
    /// put back as it was.
    pub content: String,
    pub proposals: Vec<PlannedProposal>,
    /// Where each `[N]` marker of `content` came from, in body order: the
    /// machine's body (its markers outside the spans the gate replaced) or a
    /// protected sentence put back with its markers from the current body.
    pub markers: Vec<MarkerOrigin>,
}

/// The citation occurrence (1-based) one marker of a gate plan's body had
/// before the plan, in the body it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerOrigin {
    /// Occurrence in the machine's body.
    New(u32),
    /// Occurrence in the current body: a protected sentence put back keeps
    /// its markers, so the person's citations stay on it.
    Current(u32),
}

/// 1-based occurrences of the markers inside `start..end` of `body`.
fn occurrences_within(markers: &[(usize, usize)], start: usize, end: usize) -> Vec<u32> {
    markers
        .iter()
        .enumerate()
        .filter(|(_, (from, _))| *from >= start && *from < end)
        .map(|(k, _)| u32::try_from(k + 1).unwrap_or(u32::MAX))
        .collect()
}

/// Plan a machine write of `new` over `current` whose units at the indices in
/// `protected` may not change. `None` when the write leaves every protected
/// sentence as it is: it lands unchanged.
pub fn plan_sentence_gate(
    current: &str,
    new: &str,
    protected: &HashSet<usize>,
) -> Option<GatePlan> {
    if protected.is_empty() {
        return None;
    }
    let old_units = sentence_units(current);
    let new_units = sentence_units(new);
    let current_markers = crate::citations::marker_spans(current);
    let old_norms: Vec<&str> = old_units.iter().map(|u| u.norm.as_str()).collect();
    let new_norms: Vec<&str> = new_units.iter().map(|u| u.norm.as_str()).collect();
    let still_there: HashSet<&str> = new_norms.iter().copied().collect();
    let ops = pair_protected_deletions(align(&old_norms, &new_norms), |o| {
        protected.contains(&o) && !still_there.contains(old_norms[o])
    });

    // Replacements of a span of `new`, and insertions at a byte position of
    // `new`, both applied at the end from the back. Each carries the current
    // body's marker occurrences in the text it puts back.
    let mut replacements: Vec<(usize, usize, String, Vec<u32>)> = Vec::new();
    let mut insertions: Vec<(usize, String, Vec<u32>)> = Vec::new();
    let mut proposals = Vec::new();
    let mut anchor: Option<usize> = None;
    for op in &ops {
        match *op {
            AlignOp::Equal(_, n) | AlignOp::Insert(n) => anchor = Some(n),
            AlignOp::Replace(o, n) => {
                if protected.contains(&o) && !still_there.contains(old_norms[o]) {
                    let unit = &old_units[o];
                    replacements.push((
                        new_units[n].start,
                        new_units[n].end,
                        unit.text.clone(),
                        occurrences_within(&current_markers, unit.start, unit.end),
                    ));
                    proposals.push(PlannedProposal {
                        sentence_hash: unit.hash.clone(),
                        before: unit.text.clone(),
                        after: crate::citations::strip_markers(&new_units[n].text),
                        reason: None,
                    });
                }
                anchor = Some(n);
            }
            AlignOp::Delete(o) => {
                if !protected.contains(&o) || still_there.contains(old_norms[o]) {
                    continue;
                }
                let unit = &old_units[o];
                let text = &unit.text;
                let separator = if unit.blank_before { "\n\n" } else { "\n" };
                let insertion = match (anchor, unit.whole_line) {
                    (Some(n), true) => {
                        let end = new_units[n].end;
                        let line_end = new[end..].find('\n').map_or(new.len(), |i| end + i);
                        (line_end, format!("{separator}{text}"))
                    }
                    (Some(n), false) => (new_units[n].end, format!(" {text}")),
                    (None, true) => (0, format!("{text}{separator}")),
                    (None, false) => (0, format!("{text} ")),
                };
                insertions.push((
                    insertion.0,
                    insertion.1,
                    occurrences_within(&current_markers, unit.start, unit.end),
                ));
                proposals.push(PlannedProposal {
                    sentence_hash: unit.hash.clone(),
                    before: unit.text.clone(),
                    after: String::new(),
                    reason: None,
                });
            }
        }
    }
    if proposals.is_empty() {
        return None;
    }

    // Apply from the back so earlier offsets stay valid. Insertions at the
    // same position keep their order.
    let mut edits: Vec<(usize, usize, String, usize, Vec<u32>)> = replacements
        .into_iter()
        .enumerate()
        .map(|(i, (from, to, text, occ))| (from, to, text, i, occ))
        .collect();
    let base = edits.len();
    edits.extend(
        insertions
            .into_iter()
            .enumerate()
            .map(|(i, (at, text, occ))| (at, at, text, base + i, occ)),
    );

    // The markers of the landing body in order: the machine's markers that
    // no replacement covers, and each put-back sentence's own markers at the
    // place it goes (before a machine marker at the same offset, as the text
    // is inserted in front of it; edits at one offset in their order).
    let mut placed: Vec<(usize, u8, usize, MarkerOrigin)> = Vec::new();
    for (k, (start, _)) in crate::citations::marker_spans(new).iter().enumerate() {
        if !edits
            .iter()
            .any(|(from, to, ..)| from != to && start >= from && start < to)
        {
            let occurrence = u32::try_from(k + 1).unwrap_or(u32::MAX);
            placed.push((*start, 1, 0, MarkerOrigin::New(occurrence)));
        }
    }
    for (from, _, _, order, occurrences) in &edits {
        for (j, occurrence) in occurrences.iter().enumerate() {
            placed.push((
                *from,
                0,
                order * 1024 + j,
                MarkerOrigin::Current(*occurrence),
            ));
        }
    }
    placed.sort_by_key(|(at, kind, order, _)| (*at, *kind, *order));
    let markers = placed.into_iter().map(|(.., origin)| origin).collect();

    edits.sort_by(|a, b| b.0.cmp(&a.0).then(b.3.cmp(&a.3)));
    let mut content = new.to_string();
    for (from, to, text, ..) in edits {
        content.replace_range(from..to, &text);
    }
    Some(GatePlan {
        content,
        proposals,
        markers,
    })
}

/// Within each run of deletions and insertions, pair a deleted protected unit
/// with an inserted one in order, as a replacement: the machine wrote that
/// text in place of the person's sentence, so it becomes the proposal for that
/// sentence instead of landing beside it.
fn pair_protected_deletions(ops: Vec<AlignOp>, protected: impl Fn(usize) -> bool) -> Vec<AlignOp> {
    let mut out = Vec::with_capacity(ops.len());
    let mut deleted: Vec<usize> = Vec::new();
    let mut inserted: Vec<usize> = Vec::new();
    let flush = |out: &mut Vec<AlignOp>, deleted: &mut Vec<usize>, inserted: &mut Vec<usize>| {
        let mut free = inserted.iter().copied().peekable();
        let mut paired = Vec::new();
        for &o in deleted.iter() {
            match (protected(o), free.peek().copied()) {
                (true, Some(n)) => {
                    free.next();
                    paired.push(AlignOp::Replace(o, n));
                }
                _ => paired.push(AlignOp::Delete(o)),
            }
        }
        out.extend(paired);
        out.extend(free.map(AlignOp::Insert));
        deleted.clear();
        inserted.clear();
    };
    for op in ops {
        match op {
            AlignOp::Delete(o) => deleted.push(o),
            AlignOp::Insert(n) => inserted.push(n),
            other => {
                flush(&mut out, &mut deleted, &mut inserted);
                out.push(other);
            }
        }
    }
    flush(&mut out, &mut deleted, &mut inserted);
    out
}

/// The citation records of a gate plan's landing body: for each marker, in
/// body order, the machine's record for it (from `new_citations_json`) or the
/// current page's record for a marker on a put-back sentence (from
/// `current`), renumbered to its place in the landing body. A marker with no
/// record stays unmapped. `None` when the machine sent no citation map and
/// no put-back marker has a record, which stores the page as never
/// annotated, as before; unparseable machine JSON counts as no machine
/// records, so the person's citations are kept regardless.
pub fn merge_gate_citations(
    new_citations_json: Option<&str>,
    current: &[wenlan_types::pages::PageCitation],
    markers: &[MarkerOrigin],
) -> Option<String> {
    let new_records: Vec<wenlan_types::pages::PageCitation> = new_citations_json
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    let by_new: HashMap<u32, &wenlan_types::pages::PageCitation> =
        new_records.iter().map(|c| (c.occurrence, c)).collect();
    let by_current: HashMap<u32, &wenlan_types::pages::PageCitation> =
        current.iter().map(|c| (c.occurrence, c)).collect();
    let merged: Vec<_> = markers
        .iter()
        .enumerate()
        .filter_map(|(i, origin)| {
            let record = match origin {
                MarkerOrigin::New(k) => by_new.get(k),
                MarkerOrigin::Current(k) => by_current.get(k),
            }?;
            let mut record = (*record).clone();
            record.occurrence = u32::try_from(i + 1).unwrap_or(u32::MAX);
            Some(record)
        })
        .collect();
    if merged.is_empty() && new_citations_json.is_none() {
        return None;
    }
    serde_json::to_string(&merged).ok()
}

#[cfg(test)]
#[path = "authorship_test.rs"]
mod tests;
