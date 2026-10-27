use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::extract::ExtractedEntity;
use crate::sources::{compute_effective_confidence, RawDocument};

/// Pre-compiled regexes for prefix stripping (compiled once).
/// Matches: [2025-06-15] [type] - ... or [2025-06-15] - ...
static DATE_PREFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(\d{4}-\d{2}-\d{2})\]\s*(?:\[(\w+)\]\s*)?-\s*").unwrap());
/// Matches: [unknown] [type] - ... or [unknown] - ...
static UNKNOWN_PREFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[unknown\]\s*(?:\[(\w+)\]\s*)?-\s*").unwrap());
static NUMBERED_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+\.\s+").unwrap());
/// Standalone type tag at start of line: [type] - ...
static TYPE_PREFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(\w+)\]\s*-\s*").unwrap());

/// Maximum raw input size in bytes (512 KB).
pub const MAX_INPUT_BYTES: usize = 512_000;

/// Maximum number of parsed memories allowed per import.
pub const MAX_MEMORIES: usize = 500;

/// Minimum character count for a memory to be kept (filters noise).
const MIN_CONTENT_CHARS: usize = 20;

/// ChatGPT memory export section headers — not actual memories.
/// Claude and ChatGPT memory export section headers — not actual memories.
const SECTION_HEADERS: &[&str] = &[
    // Claude section headers
    "earlier context",
    "recent months",
    "brief history",
    "top of mind",
    "personal context",
    "work context",
    "communication style",
    "communication & interaction style",
    "communication & interaction",
    "communication & interaction preferences",
    "behavioral patterns",
    "general context",
    "professional context",
    "learning & interests",
    "preferences & style",
    "key relationships",
    "background",
    // ChatGPT section headers
    "about me",
    "helpful response style",
    "what i do",
    "my preferences",
];

/// Valid memory type tags that can appear in import format.
/// Includes legacy types (correction, custom, recap) for backward compat — they map to "fact".
const VALID_TYPES: &[&str] = &[
    "identity",
    "preference",
    "decision",
    "lesson",
    "gotcha",
    "fact",
    "correction",
    "custom",
    "recap", // legacy — accepted but stored as "fact"
];

/// A single memory extracted from raw import text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedMemory {
    pub content: String,
    /// Unix timestamp extracted from date prefix, if present.
    pub extracted_date: Option<i64>,
    /// Memory type extracted from `[type]` tag, if present.
    pub memory_type: Option<String>,
}

/// Validate that raw input does not exceed the size limit.
pub fn validate_input(input: &str) -> Result<(), WenlanError> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(WenlanError::Generic(format!(
            "Input size ({} bytes) exceeds maximum allowed ({} bytes)",
            input.len(),
            MAX_INPUT_BYTES
        )));
    }
    Ok(())
}

/// Validate that the parsed memory count does not exceed the limit.
pub fn validate_parsed_count(count: usize) -> Result<(), WenlanError> {
    if count > MAX_MEMORIES {
        return Err(WenlanError::Generic(format!(
            "Parsed {} memories, which exceeds the maximum of {}",
            count, MAX_MEMORIES
        )));
    }
    Ok(())
}

/// Longest caller-supplied batch id the importer will mint source ids from.
const MAX_BATCH_ID_LEN: usize = 64;

/// Validate a caller-supplied import batch id.
///
/// The batch id is embedded in every source id the import mints
/// (`import_{batch}_{chunk}_{i}`), and batch membership is recovered by
/// stripping the trailing numeric segments back off. An id containing `_`
/// makes that recovery ambiguous: batch `x` would claim the memories of an
/// un-chunked import whose batch id is `x_0`, reporting another import's rows
/// as its own with the wrong chunk count and wrong phase totals. The desktop
/// app sends `crypto.randomUUID()`, which is already safe; this closes the
/// door for MCP and CLI callers, who can send anything.
pub fn validate_batch_id(batch_id: &str) -> Result<(), WenlanError> {
    if batch_id.is_empty() || batch_id.len() > MAX_BATCH_ID_LEN {
        return Err(WenlanError::Generic(format!(
            "Import batch id must be 1..={MAX_BATCH_ID_LEN} characters"
        )));
    }
    if !batch_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
    {
        return Err(WenlanError::Generic(
            "Import batch id may contain only letters, digits, '-' and '.'".to_string(),
        ));
    }
    Ok(())
}

/// Returns true if a line is a visual separator (e.g. `---`, `===`, `***`).
fn is_separator(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return true;
    }
    // A separator is a line composed entirely of repeated `-`, `=`, or `*`
    // with at least 3 characters.
    if trimmed.chars().count() >= 3 {
        let first = trimmed.chars().next().unwrap();
        if matches!(first, '-' | '=' | '*') && trimmed.chars().all(|c| c == first) {
            return true;
        }
    }
    false
}

/// Detect whether paragraph mode should be used.
///
/// Paragraph mode is active when:
/// 1. The text contains at least one `\n\n` boundary, AND
/// 2. The majority of double-newline-separated blocks contain internal
///    single newlines (i.e., they are multi-line paragraphs, not just
///    single lines separated by blank lines).
fn is_paragraph_mode(text: &str) -> bool {
    if !text.contains("\n\n") {
        return false;
    }
    let blocks: Vec<&str> = text.split("\n\n").collect();
    if blocks.len() < 2 {
        return false;
    }
    let multi_line_blocks = blocks
        .iter()
        .filter(|b| {
            let trimmed = b.trim();
            !trimmed.is_empty() && trimmed.contains('\n')
        })
        .count();
    // "most blocks span multiple lines" — majority threshold
    let non_empty_blocks = blocks.iter().filter(|b| !b.trim().is_empty()).count();
    non_empty_blocks > 0 && multi_line_blocks * 2 >= non_empty_blocks
}

/// Strip common list/date prefixes from a line, returning the cleaned
/// content, an optional extracted date, an optional memory type, and
/// whether the line carried an explicit export marker (a date prefix with a
/// real calendar date, an `[unknown]` prefix, or a standalone `[type]` tag
/// naming a valid type).
/// A marked line is exempt from the `MIN_CONTENT_CHARS` floor in
/// `parse_memories_counted` -- it's an explicit assertion, not free-form
/// noise, even when short.
fn strip_prefix(line: &str) -> (String, Option<i64>, Option<String>, bool) {
    let trimmed = line.trim();

    // Date prefix with optional type tag: [2025-06-15] [type] - ... or [2025-06-15] - ...
    if let Some(caps) = DATE_PREFIX_RE.captures(trimmed) {
        let date_str = &caps[1];
        let rest = &trimmed[caps.get(0).unwrap().end()..];
        let timestamp = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|dt| dt.and_utc().timestamp());
        let mem_type = caps
            .get(2)
            .map(|m| m.as_str().to_lowercase())
            .filter(|t| VALID_TYPES.contains(&t.as_str()));
        // A malformed date is not a marker, but an independently valid
        // type tag still identifies an explicit assertion.
        let marked = timestamp.is_some() || mem_type.is_some();
        return (rest.trim().to_string(), timestamp, mem_type, marked);
    }

    // [unknown] with optional type tag: [unknown] [type] - ...
    if let Some(caps) = UNKNOWN_PREFIX_RE.captures(trimmed) {
        let rest = &trimmed[caps.get(0).unwrap().end()..];
        let mem_type = caps
            .get(1)
            .map(|m| m.as_str().to_lowercase())
            .filter(|t| VALID_TYPES.contains(&t.as_str()));
        return (rest.trim().to_string(), None, mem_type, true);
    }

    // Standalone type tag: [type] - ...
    if let Some(caps) = TYPE_PREFIX_RE.captures(trimmed) {
        let tag = caps[1].to_lowercase();
        if VALID_TYPES.contains(&tag.as_str()) {
            let rest = &trimmed[caps.get(0).unwrap().end()..];
            return (rest.trim().to_string(), None, Some(tag), true);
        }
    }

    // Numbered list: 1. , 2. , etc.
    if let Some(m) = NUMBERED_PREFIX_RE.find(trimmed) {
        return (trimmed[m.end()..].trim().to_string(), None, None, false);
    }

    // Bullet prefixes: - , * , bullet char
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return (rest.trim().to_string(), None, None, false);
    }
    if let Some(rest) = trimmed.strip_prefix("* ") {
        return (rest.trim().to_string(), None, None, false);
    }
    if let Some(rest) = trimmed.strip_prefix('\u{2022}') {
        return (rest.trim().to_string(), None, None, false);
    }

    (trimmed.to_string(), None, None, false)
}

/// Returns true if a trimmed line opens or closes a fenced code block
/// (e.g. `` ``` `` or `` ```text ``). Export pastes sometimes wrap sample
/// output in fences; the marker itself is never a memory.
fn is_code_fence(line: &str) -> bool {
    line.starts_with("```")
}

/// Parse raw text into a list of individual memories, keeping every entry
/// [`parse_memories`] does. See it for the supported formats.
///
/// Returns the kept memories alongside a count of entries dropped during
/// parsing: too-short unmarked lines, empty-after-strip lines, known
/// section headers, and in-batch exact duplicates. Blank lines, separators
/// (`---`, `===`, `***`), and code-fence lines are not counted -- they are
/// formatting, not dropped content.
///
/// A line carrying an explicit export marker (a real calendar date prefix,
/// `[unknown]`, or a valid `[type]` tag) is exempt from the
/// `MIN_CONTENT_CHARS` floor: it's an explicit assertion, not free-form
/// noise, even when short. The floor still applies to everything else, so
/// bare short lines, bracketed words that aren't a recognized type (e.g.
/// `[notatype] - x`), and date-shaped prefixes that don't parse (e.g.
/// `[2025-13-99] - x`) keep being filtered and counted.
///
/// CRLF line endings are normalized to LF before mode detection, so a
/// `\r\n\r\n`-separated paste parses exactly like its `\n\n` equivalent.
pub fn parse_memories_counted(raw_text: &str) -> (Vec<ParsedMemory>, usize) {
    // Normalize CRLF to LF first: paragraph detection and the paragraph
    // split look for "\n\n", which a "\r\n\r\n" paste never contains. Line
    // mode is unaffected, since `str::lines()` already strips a trailing \r.
    let normalized: Cow<'_, str> = if raw_text.contains("\r\n") {
        Cow::Owned(raw_text.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(raw_text)
    };
    let raw_text: &str = &normalized;
    let paragraph_mode = is_paragraph_mode(raw_text);

    let raw_blocks: Vec<String> = if paragraph_mode {
        // Split on double newlines; collapse internal newlines to spaces.
        // Fence lines are dropped here, per physical line, before the join --
        // a whole-block fence check after collapsing would treat a real
        // paragraph that merely contains a fence line as one and discard its
        // other content along with it.
        raw_text
            .split("\n\n")
            .map(|block| {
                block
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty() && !is_code_fence(l))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    } else {
        // One entry per line.
        raw_text.lines().map(|l| l.to_string()).collect()
    };

    let mut seen = HashSet::new();
    let mut result = Vec::new();
    let mut dropped = 0usize;

    for block in &raw_blocks {
        let trimmed = block.trim();

        // Skip empty and separator lines without counting them as drops.
        if is_separator(trimmed) {
            continue;
        }

        // Skip code-fence lines without counting them as drops. Paragraph
        // mode already filtered these out per-line above; this also covers
        // line mode, where each block is one physical line.
        if is_code_fence(trimmed) {
            continue;
        }

        let (content, extracted_date, memory_type, marked) = strip_prefix(trimmed);

        // Empty content after stripping (e.g. "[fact] - " with nothing
        // after the dash): drop and count, marked or not.
        if content.is_empty() {
            dropped += 1;
            continue;
        }

        // Skip entries shorter than MIN_CONTENT_CHARS, unless the line
        // carried an explicit export marker.
        if !marked && content.chars().count() < MIN_CONTENT_CHARS {
            dropped += 1;
            continue;
        }

        // Skip known section headers (e.g. "Earlier context", "Top of mind")
        if SECTION_HEADERS.contains(&content.to_lowercase().as_str()) {
            dropped += 1;
            continue;
        }

        // Deduplicate exact matches
        if !seen.insert(content.clone()) {
            dropped += 1;
            continue;
        }

        result.push(ParsedMemory {
            content,
            extracted_date,
            memory_type,
        });
    }

    (result, dropped)
}

/// Parse raw text into a list of individual memories.
///
/// Supports multiple formats:
/// - Plain lines (one memory per line)
/// - Paragraph mode (double-newline separated blocks)
/// - ChatGPT-style date prefixes: `[2025-06-15] - ...`
/// - `[unknown] - ...` prefixes
/// - Bullet lists (`- `, `* `, `\u{2022} `)
/// - Numbered lists (`1. `, `2. `, etc.)
///
/// Skips empty lines, separators (`---`, `===`, `***`), code fences, and
/// unmarked entries shorter than 20 characters (filters section headers and
/// free-form noise; a line with an explicit export marker is exempt -- see
/// [`parse_memories_counted`]). Deduplicates exact matches within the batch.
pub fn parse_memories(raw_text: &str) -> Vec<ParsedMemory> {
    parse_memories_counted(raw_text).0
}

/// Result of an import operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub imported: usize,
    pub skipped: usize,
    pub breakdown: HashMap<String, usize>,
    pub entities_created: usize,
    pub observations_added: usize,
    pub relations_created: usize,
    pub batch_id: String,
}

/// Find indices of memories that already exist in the DB by exact content match.
/// Uses content-based matching instead of vector similarity because contextual
/// enrichment in upsert_documents changes the stored embedding prefix.
pub async fn find_duplicates(db: &MemoryDB, memories: &[ParsedMemory]) -> HashSet<usize> {
    let mut duplicates = HashSet::new();
    for (i, memory) in memories.iter().enumerate() {
        if let Ok(true) = db.has_memory_content(&memory.content).await {
            duplicates.insert(i);
        }
    }
    duplicates
}

/// Returns (entity_id, was_newly_created).
/// Bulk-import variant: an in-batch cache in front of the canonical
/// `MemoryDB::resolve_or_create_entity` resolve-then-write cascade (M3 PR-1
/// stage d), skipping the single-entity post-write enrichment ring (verify,
/// activity log, refinery enqueue) that path doesn't need at bulk volume. Use
/// `post_write::create_entity` for single-entity writes that should fire that
/// ring. NOTE (behavior change from the pre-collapse version): resolution-step
/// errors now propagate instead of being swallowed-and-fallen-through, so a
/// transient DB error skips this entity for the round (existing call site
/// `if let Ok((_id, true)) = ...` already discards `Err` uniformly) rather
/// than risking a duplicate create -- matching `post_write::create_entity`'s
/// existing stricter semantics now that both share one cascade.
pub(crate) async fn resolve_entity_bulk(
    db: &MemoryDB,
    entity_cache: &mut HashMap<String, String>,
    entity: &ExtractedEntity,
    source: &str,
) -> Result<(String, bool), WenlanError> {
    let name_lower = entity.name.to_lowercase();

    // Step 0: In-batch cache (case-insensitive) -- bulk-specific, stays here.
    if let Some(id) = entity_cache.get(&name_lower) {
        return Ok((id.clone(), false));
    }

    let (id, created) = db
        .resolve_or_create_entity(&entity.name, &entity.entity_type, None, Some(source), None)
        .await?;
    entity_cache.insert(name_lower, id.clone());
    Ok((id, created))
}

/// Import memories into one daemon-resolved Space. Stable Space identity is
/// carried through the async embedding work and re-resolved in the DB write
/// transaction, matching the top-level store path.
pub async fn import_memories_no_llm_in_space(
    db: &MemoryDB,
    raw_text: &str,
    source: &str,
    _label: Option<&str>,
    confidence_cfg: &crate::tuning::ConfidenceConfig,
    write_space: &crate::space_context::ResolvedWriteSpace,
) -> Result<ImportResult, WenlanError> {
    import_memories_no_llm_inner(
        db,
        raw_text,
        source,
        confidence_cfg,
        Some(write_space),
        None,
        None,
    )
    .await
}

/// [`import_memories_no_llm_in_space`], plus the chunked-import identity: when
/// the caller supplies a batch id it is used verbatim so every chunk of one
/// user-facing import aggregates under one batch; otherwise one is minted as
/// before. `chunk_index` names this chunk's segment in the minted source ids
/// and is what keeps per-chunk `i` counters from colliding.
#[allow(clippy::too_many_arguments)]
pub async fn import_memories_no_llm_in_batch(
    db: &MemoryDB,
    raw_text: &str,
    source: &str,
    _label: Option<&str>,
    confidence_cfg: &crate::tuning::ConfidenceConfig,
    write_space: &crate::space_context::ResolvedWriteSpace,
    batch_id: Option<&str>,
    chunk_index: Option<u32>,
) -> Result<ImportResult, WenlanError> {
    import_memories_no_llm_inner(
        db,
        raw_text,
        source,
        confidence_cfg,
        Some(write_space),
        batch_id,
        chunk_index,
    )
    .await
}

async fn import_memories_no_llm_inner(
    db: &MemoryDB,
    raw_text: &str,
    source: &str,
    confidence_cfg: &crate::tuning::ConfidenceConfig,
    write_space: Option<&crate::space_context::ResolvedWriteSpace>,
    batch_id: Option<&str>,
    chunk_index: Option<u32>,
) -> Result<ImportResult, WenlanError> {
    validate_input(raw_text)?;
    if let Some(id) = batch_id {
        validate_batch_id(id)?;
    }
    let (memories, parse_dropped) = parse_memories_counted(raw_text);
    validate_parsed_count(memories.len())?;

    let duplicates = find_duplicates(db, &memories).await;

    let minted_batch_id;
    let batch_id = match batch_id {
        Some(id) => id,
        None => {
            minted_batch_id = uuid::Uuid::new_v4().to_string();
            minted_batch_id.as_str()
        }
    };
    let now = chrono::Utc::now().timestamp();
    let mut imported = 0usize;
    // Starts at the parse-time drop count (too-short, empty, headers,
    // in-batch duplicates); DB duplicates are added to it below.
    let mut skipped = parse_dropped;
    let mut breakdown: HashMap<String, usize> = HashMap::new();

    // Collect all non-duplicate docs for a single batch upsert
    let mut docs: Vec<RawDocument> = Vec::new();

    for (i, memory) in memories.iter().enumerate() {
        if duplicates.contains(&i) {
            skipped += 1;
            continue;
        }

        // memory_type starts as None (unclassified) — refinery reclassifies in background
        let confidence = compute_effective_confidence(None, None, "review", None, confidence_cfg);

        let title: String = memory.content.chars().take(80).collect();
        // Chunked imports restart `i` at 0 per chunk, so the chunk index is
        // part of the id whenever the caller supplies one; both forms match
        // an `import_{batch}_%` prefix query.
        let source_id = match chunk_index {
            Some(chunk) => format!("import_{}_{}_{}", batch_id, chunk, i),
            None => format!("import_{}_{}", batch_id, i),
        };

        docs.push(RawDocument {
            source: "memory".to_string(),
            source_id,
            title,
            summary: None,
            content: memory.content.clone(),
            url: None,
            last_modified: memory.extracted_date.unwrap_or(now),
            metadata: HashMap::from([
                ("import_source".to_string(), source.to_string()),
                ("import_batch".to_string(), batch_id.to_string()),
            ]),
            memory_type: None,
            space: write_space.and_then(|resolved| resolved.space_name.clone()),
            source_agent: Some(source.to_string()),
            confidence: Some(confidence),
            confirmed: None,
            supersedes: None,
            pending_revision: false,
            ..Default::default()
        });

        imported += 1;
        *breakdown.entry("unclassified".to_string()).or_insert(0) += 1;
    }

    // Single batch upsert — one embedding generation pass for all docs
    if !docs.is_empty() {
        let docs = docs
            .into_iter()
            .map(|doc| (doc, write_space.cloned()))
            .collect();
        db.upsert_documents_with_enrichment_origin_and_write_spaces(
            docs,
            crate::db::EnrichmentOrigin {
                memory_type_explicit: false,
                structured_fields_explicit: false,
                space_rejected: false,
            },
        )
        .await?;
    }

    Ok(ImportResult {
        imported,
        skipped,
        breakdown,
        entities_created: 0,
        observations_added: 0,
        relations_created: 0,
        batch_id: batch_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_memories tests ───────────────────────────────────────────

    #[test]
    fn test_parse_single_line_memories() {
        let input = "User is a software engineer\nPrefers dark mode in editors\nLikes Rust programming language";
        let result = parse_memories(input);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].content, "User is a software engineer");
        assert_eq!(result[1].content, "Prefers dark mode in editors");
        assert_eq!(result[2].content, "Likes Rust programming language");
        assert!(result[0].extracted_date.is_none());
    }

    #[test]
    fn test_parse_chatgpt_date_prefix() {
        let input =
            "[2025-06-15] - User works at a fintech startup\n[2025-01-20] - Working on Q2 launch";
        let result = parse_memories(input);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "User works at a fintech startup");
        assert!(result[0].extracted_date.is_some());
        assert_eq!(result[1].content, "Working on Q2 launch");
    }

    #[test]
    fn test_parse_unknown_date_prefix() {
        let input = "[unknown] - Prefers dark mode in editors";
        let result = parse_memories(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "Prefers dark mode in editors");
        assert!(result[0].extracted_date.is_none());
    }

    #[test]
    fn test_parse_bulleted_list() {
        let input = "- User is a software engineer\n\u{2022} Prefers dark mode in editors\n* Likes Rust programming language";
        let result = parse_memories(input);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].content, "User is a software engineer");
        assert_eq!(result[1].content, "Prefers dark mode in editors");
        assert_eq!(result[2].content, "Likes Rust programming language");
    }

    #[test]
    fn test_parse_numbered_list() {
        let input = "1. User is a software engineer\n2. Prefers dark mode in editors\n3. Likes Rust programming language";
        let result = parse_memories(input);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].content, "User is a software engineer");
    }

    #[test]
    fn test_parse_type_tags() {
        let input = "[2025-03-01] [identity] - Lives in San Francisco\n[unknown] [preference] - Prefers concise responses always\n[2025-06-15] [decision] - Chose Rust over Electron\n[unknown] [correction] - Never use emojis in code";
        let result = parse_memories(input);
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].memory_type.as_deref(), Some("identity"));
        assert_eq!(result[1].memory_type.as_deref(), Some("preference"));
        assert_eq!(result[2].memory_type.as_deref(), Some("decision"));
        assert_eq!(result[3].memory_type.as_deref(), Some("correction"));
        assert_eq!(result[0].content, "Lives in San Francisco");
        assert!(result[0].extracted_date.is_some());
        assert!(result[1].extracted_date.is_none());
    }

    #[test]
    fn test_parse_without_type_tags_defaults_none() {
        let input = "[2025-01-01] - User is a backend engineer\nPrefers dark mode in all editors";
        let result = parse_memories(input);
        assert_eq!(result.len(), 2);
        assert!(result[0].memory_type.is_none());
        assert!(result[1].memory_type.is_none());
    }

    #[test]
    fn test_skip_empty_and_separators() {
        let input = "User is a software engineer\n\n---\n\n===\n***\nPrefers dark mode in editors";
        let result = parse_memories(input);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_skip_short_lines() {
        // Short words and section-header-style lines (< 20 chars) are filtered
        let input = "User is a software engineer\nhi\nok\nPersonal context\nTop of mind\nRecent months\nPrefers dark mode in editors";
        let result = parse_memories(input);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_skip_chatgpt_section_headers() {
        let input = "Earlier context\n[2025-01-01] - User is a backend engineer\nTop of mind\nWork context\n[2025-06-01] - Prefers dark mode in editors\nPersonal context";
        let result = parse_memories(input);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "User is a backend engineer");
        assert_eq!(result[1].content, "Prefers dark mode in editors");
    }

    #[test]
    fn test_deduplicate_exact_matches() {
        let input = "User is a software engineer\nUser is a software engineer\nPrefers dark mode in editors";
        let result = parse_memories(input);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_paragraph_mode() {
        let input = "User is a software engineer\nwho works at a fintech startup\n\nPrefers dark mode\nin all applications";
        let result = parse_memories(input);
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0].content,
            "User is a software engineer who works at a fintech startup"
        );
        assert_eq!(result[1].content, "Prefers dark mode in all applications");
    }

    // ── short tagged line exemption (paste-short-lines) ─────────────────

    #[test]
    fn parse_keeps_short_marked_lines() {
        let input = "[fact] - Uses Rust daily\n[identity] - 住在台北\n[2025-01-20] - Likes tea\n[unknown] [preference] - Dark mode";
        let result = parse_memories(input);
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].content, "Uses Rust daily");
        assert_eq!(result[0].memory_type.as_deref(), Some("fact"));
        assert_eq!(result[1].content, "住在台北");
        assert_eq!(result[1].memory_type.as_deref(), Some("identity"));
        assert_eq!(result[2].content, "Likes tea");
        assert!(result[2].extracted_date.is_some());
        assert_eq!(result[3].content, "Dark mode");
        assert_eq!(result[3].memory_type.as_deref(), Some("preference"));
    }

    #[test]
    fn parse_drops_short_lines_with_invalid_markers() {
        // A bracketed word that isn't in VALID_TYPES doesn't exempt the line
        // from the length floor: it's ordinary short noise, not an explicit
        // export marker.
        let input = "[notatype] - x\n[fact] - Uses Rust daily";
        let result = parse_memories(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "Uses Rust daily");
    }

    #[test]
    fn parse_drops_empty_tagged_content() {
        // A tag with nothing after the dash must not become an empty-content
        // memory, marked or not.
        let input = "[fact] - \n[fact] - Uses Rust daily";
        let result = parse_memories(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "Uses Rust daily");
    }

    #[test]
    fn parse_respects_exact_length_boundary() {
        // Unmarked lines: 19 chars is still filtered, 20 is the kept floor.
        // Unchanged by the marked-line exemption; guards against an
        // off-by-one regression in the surrounding refactor.
        let short = "a".repeat(19);
        let exact = "a".repeat(20);
        let input = format!("{short}\n{exact}");
        let result = parse_memories(&input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, exact);
    }

    #[test]
    fn parse_skips_code_fence_lines() {
        // A fence line long enough to clear the length floor on its own must
        // still be dropped as a fence, not kept as a memory.
        let input = "```a-long-enough-fence-marker-line\nUser is a software engineer";
        let result = parse_memories(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "User is a software engineer");
    }

    #[test]
    fn paragraph_mode_fence_line_does_not_discard_paragraph_payload() {
        // Paragraph mode collapses a block's internal lines into one memory
        // before the main loop's per-block filters run (importer.rs:256-267).
        // A fence line embedded in a real paragraph must be dropped at the
        // per-line stage, before the join -- not by a whole-block
        // "starts with a fence" check in the main loop, which would throw
        // away the real payload collapsed alongside it.
        let input = "```\nUser is a software engineer who ships Rust\nand reviews pull requests daily\n\nPrefers dark mode\nin every editor";
        let result = parse_memories(input);
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0].content,
            "User is a software engineer who ships Rust and reviews pull requests daily"
        );
        assert_eq!(result[1].content, "Prefers dark mode in every editor");
    }

    #[test]
    fn parse_keeps_short_marked_lines_with_crlf() {
        // CRLF export text: str::lines() already strips the trailing \r, so
        // a short marked line surrounded by CRLF terminators is still kept,
        // with no stray \r left in the content.
        let input = "[fact] - Uses Rust daily\r\n[identity] - 住在台北\r\nhi\r\n";
        let result = parse_memories(input);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "Uses Rust daily");
        assert_eq!(result[1].content, "住在台北");
    }

    #[test]
    fn parse_counts_dropped_lines() {
        // Not "\n\n" anywhere in this input, so it stays in line mode: each
        // physical line is judged on its own (paragraph mode would collapse
        // several of these into one block instead).
        let input = "User is a software engineer\nhi\nTop of mind\nUser is a software engineer\n---\n```\n```text";
        let (result, dropped) = parse_memories_counted(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "User is a software engineer");
        // hi (too short), "Top of mind" (header), and the repeat (duplicate)
        // are dropped and counted; the separator and both fence lines are
        // formatting and are not.
        assert_eq!(dropped, 3);
    }

    #[test]
    fn parse_crlf_paragraph_mode_matches_lf() {
        // Every block spans two lines, so the LF form parses in paragraph
        // mode. The CRLF form has no "\n\n" in it until CRLF is normalized;
        // without that it falls back to line mode and splits each paragraph
        // into short fragments, keeping and dropping different content.
        let lf = "[fact] - Uses Rust\nand Go\n\n[identity] - 住在\n台北\n\nhi\nthere\n\n[notatype] - x\ny\n\nUser is a software engineer\nwho ships Rust";
        let crlf = lf.replace('\n', "\r\n");
        assert!(crlf.contains("\r\n\r\n"));

        let summarize = |memories: &[ParsedMemory]| {
            memories
                .iter()
                .map(|m| (m.content.clone(), m.extracted_date, m.memory_type.clone()))
                .collect::<Vec<_>>()
        };

        let (lf_memories, lf_dropped) = parse_memories_counted(lf);
        let (crlf_memories, crlf_dropped) = parse_memories_counted(&crlf);

        // The LF baseline itself: short marked paragraphs kept, the short
        // unmarked and invalid-marker paragraphs dropped and counted.
        let contents: Vec<&str> = lf_memories.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(
            contents,
            vec![
                "Uses Rust and Go",
                "住在 台北",
                "User is a software engineer who ships Rust"
            ]
        );
        assert_eq!(lf_dropped, 2);

        assert_eq!(summarize(&crlf_memories), summarize(&lf_memories));
        assert_eq!(crlf_dropped, lf_dropped);
    }

    #[test]
    fn parse_invalid_calendar_date_prefix_is_not_a_marker() {
        // `[2025-13-99]` matches the date-prefix shape but is not a real
        // date, so it doesn't exempt the line from the length floor: the
        // short line is dropped and counted. A real date still exempts one.
        let input = "[2025-13-99] - x\n[2025-01-20] - Likes tea";
        let (result, dropped) = parse_memories_counted(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "Likes tea");
        assert!(result[0].extracted_date.is_some());
        assert_eq!(dropped, 1);
    }

    #[test]
    fn parse_valid_type_survives_an_invalid_date_prefix() {
        let (result, dropped) =
            parse_memories_counted("[2025-13-99] [fact] - Likes tea\n[2025-13-99] [notatype] - x");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "Likes tea");
        assert_eq!(result[0].memory_type.as_deref(), Some("fact"));
        assert_eq!(result[0].extracted_date, None);
        assert_eq!(dropped, 1);
    }

    // ── validation tests ───────────────────────────────────────────────

    #[test]
    fn test_validate_input_size_ok() {
        let input = "a".repeat(100);
        assert!(validate_input(&input).is_ok());
    }

    #[test]
    fn test_validate_input_too_large() {
        let input = "a".repeat(MAX_INPUT_BYTES + 1);
        let err = validate_input(&input).unwrap_err();
        assert!(err.to_string().contains("exceeds"));
    }

    #[test]
    fn test_validate_too_many_memories() {
        let input = (0..501)
            .map(|i| format!("This is a test memory number {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed = parse_memories(&input);
        assert!(parsed.len() > MAX_MEMORIES);
        let err = validate_parsed_count(parsed.len()).unwrap_err();
        assert!(err.to_string().contains("500"));
    }

    // ── Task 5: Vector dedup tests ────────────────────────────────────

    #[tokio::test]
    async fn test_find_duplicates_empty_db() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let memories = vec![ParsedMemory {
            content: "User is an engineer".to_string(),
            extracted_date: None,
            memory_type: None,
        }];
        let dupes = find_duplicates(&db, &memories).await;
        assert!(dupes.is_empty());
    }

    // ── Task 6: Entity resolution tests ───────────────────────────────

    #[tokio::test]
    async fn test_resolve_or_create_entity_new() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let mut cache = HashMap::new();
        let entity = crate::extract::ExtractedEntity {
            name: "Alice".to_string(),
            entity_type: "person".to_string(),
        };
        let (id, created) = resolve_entity_bulk(&db, &mut cache, &entity, "chatgpt")
            .await
            .unwrap();
        assert!(!id.is_empty());
        assert!(created);
        assert_eq!(cache.get("alice"), Some(&id));
    }

    #[tokio::test]
    async fn test_resolve_or_create_entity_cached() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let mut cache = HashMap::new();
        cache.insert("alice".to_string(), "existing_id".to_string());
        let entity = crate::extract::ExtractedEntity {
            name: "Alice".to_string(),
            entity_type: "person".to_string(),
        };
        let (id, created) = resolve_entity_bulk(&db, &mut cache, &entity, "chatgpt")
            .await
            .unwrap();
        assert_eq!(id, "existing_id");
        assert!(!created);
    }

    // ── Task 4: Alias-based entity resolution tests ───────────────────

    #[tokio::test]
    async fn test_resolve_or_create_entity_alias_resolution() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let mut cache = std::collections::HashMap::new();

        let entity = crate::extract::ExtractedEntity {
            name: "Alice Chen".to_string(),
            entity_type: "person".to_string(),
        };

        // First call: creates entity + alias
        let (id1, created1) = resolve_entity_bulk(&db, &mut cache, &entity, "test")
            .await
            .unwrap();
        assert!(created1);

        // Clear cache to force alias lookup
        cache.clear();

        // Second call with same case: should resolve via alias, not create
        let (id2, created2) = resolve_entity_bulk(&db, &mut cache, &entity, "test")
            .await
            .unwrap();
        assert!(!created2);
        assert_eq!(id1, id2);

        // Clear cache again
        cache.clear();

        // Third call with different case: should also resolve via alias
        let entity_lower = crate::extract::ExtractedEntity {
            name: "alice chen".to_string(),
            entity_type: "person".to_string(),
        };
        let (id3, created3) = resolve_entity_bulk(&db, &mut cache, &entity_lower, "test")
            .await
            .unwrap();
        assert!(!created3);
        assert_eq!(id1, id3);
    }

    // ── Import batch status tests ─────────────────────────────────────

    async fn seed_import_memory(
        db: &MemoryDB,
        source_id: &str,
        source_agent: &str,
        space: Option<&str>,
    ) {
        let session = db.test_primary_session().await;
        session
            .execute(
                "INSERT INTO memories (id, content, source, source_id, title, chunk_index, \
                 last_modified, chunk_type, source_agent, space, created_at) \
                 VALUES (?1, ?2, 'memory', ?3, ?4, 0, 1700000000, 'prose', ?5, ?6, 1700000000)",
                libsql::params![
                    format!("mem_{source_id}"),
                    format!("seeded content for {source_id} that is long enough"),
                    source_id.to_string(),
                    format!("title {source_id}"),
                    source_agent.to_string(),
                    space.unwrap_or(crate::db::UNFILED_SPACE_ID).to_string(),
                ],
            )
            .await
            .expect("seed import memory");
    }

    #[tokio::test]
    async fn batch_status_waits_for_bounded_page_turn_and_recovers_after_expiry() {
        use wenlan_types::import::{ImportPhase, ImportPhaseState};
        let (db, _dir) = crate::db::tests::test_db().await;
        seed_import_memory(&db, "import_prioritytest_0", "other", None).await;
        let now = chrono::Utc::now().timestamp();
        // Seeded memory dates deliberately remain in 2023: processing tracks
        // this import, never the historical date in the note.
        db.set_app_metadata(
            "import_batch_priority_v1:prioritytest",
            &(now + 600).to_string(),
        )
        .await
        .unwrap();
        for step in [
            "entity_extract",
            "entity_link",
            "title_enrich",
            "page_growth",
        ] {
            db.record_enrichment_step("import_prioritytest_0", step, "ok", None)
                .await
                .unwrap();
        }
        db.set_app_metadata("import_priority_until_v1", &(now + 600).to_string())
            .await
            .unwrap();
        let queued = db
            .import_batch_status("prioritytest")
            .await
            .unwrap()
            .unwrap();
        assert!(
            !queued.complete,
            "do not stop polling before page formation is attempted"
        );
        assert!(db
            .active_import_batches(20)
            .await
            .unwrap()
            .iter()
            .any(|batch| batch.batch_id == "prioritytest"));
        assert_eq!(
            queued
                .phases
                .iter()
                .find(|p| p.phase == ImportPhase::Distill)
                .unwrap()
                .state,
            ImportPhaseState::Running
        );
        db.set_app_metadata("import_priority_until_v1", &(now - 1).to_string())
            .await
            .unwrap();
        let expired = db
            .import_batch_status("prioritytest")
            .await
            .unwrap()
            .unwrap();
        assert!(
            expired.complete,
            "an interrupted process must not leave an immortal running state"
        );
        assert_eq!(expired.pages_distilled, 0);
        assert_eq!(
            expired
                .phases
                .iter()
                .find(|p| p.phase == ImportPhase::Distill)
                .unwrap()
                .state,
            ImportPhaseState::Pending
        );
        db.persist_import_batch_priority("prioritytest", now - 1)
            .await
            .unwrap();
        let complete_status = db
            .import_batch_status("prioritytest")
            .await
            .unwrap()
            .unwrap();
        assert!(complete_status.complete);
        assert!(
            !db.active_import_batches(20)
                .await
                .unwrap()
                .iter()
                .any(|batch| batch.batch_id == "prioritytest"),
            "completed batches remain filtered even while their marker is recent"
        );
    }

    #[tokio::test]
    async fn active_import_batches_keeps_historical_stalled_batch_after_deadline() {
        let (db, _dir) = crate::db::tests::test_db().await;
        seed_import_memory(&db, "import_historical_stalled_0", "other", None).await;
        let now = chrono::Utc::now().timestamp();
        db.persist_import_batch_priority("historical_stalled", now - 1)
            .await
            .unwrap();
        seed_import_memory(&db, "import_too_old_0", "other", None).await;
        db.set_app_metadata(
            "import_batch_priority_v1:expired_marker",
            &(now - 24 * 60 * 60 - 1).to_string(),
        )
        .await
        .unwrap();
        db.set_app_metadata(
            "import_batch_priority_v1:too_old",
            &(now - 24 * 60 * 60 - 1).to_string(),
        )
        .await
        .unwrap();
        let before_prune = db.active_import_batches(20).await.unwrap();
        assert!(
            !before_prune.iter().any(|batch| batch.batch_id == "too_old"),
            "batches older than the 24-hour recency floor stay excluded"
        );
        db.persist_import_batch_priority("historical_stalled", now - 1)
            .await
            .unwrap();

        let active = db.active_import_batches(20).await.unwrap();
        let stalled = active
            .iter()
            .find(|batch| batch.batch_id == "historical_stalled")
            .expect("a stalled historical batch remains discoverable for 24 hours");
        assert!(!stalled.complete);
        assert_eq!(
            db.get_app_metadata("import_batch_priority_v1:expired_marker")
                .await
                .unwrap(),
            None,
            "the next import prunes only stale batch priority markers"
        );
        assert_eq!(
            db.get_app_metadata("import_batch_priority_v1:too_old")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn legacy_overview_archive_preserves_edits_and_original_content() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let placeholder = crate::synthesis::overview::OVERVIEW_PLACEHOLDER_CONTENT;
        {
            let conn = db.test_primary_session().await;
            conn.execute("INSERT INTO pages (id,title,content,status,version,source_revision,creation_kind,kind,created_at,last_compiled,last_modified) VALUES ('old_overview','Overview',?1,'active',1,0,'research','overview','now','now','now')", libsql::params![placeholder]).await.unwrap();
        }
        assert!(!db
            .archive_legacy_overview_placeholder("old_overview", 2, 0, placeholder)
            .await
            .unwrap());
        assert!(!db
            .archive_legacy_overview_placeholder("old_overview", 1, 1, placeholder)
            .await
            .unwrap());
        {
            let conn = db.test_primary_session().await;
            conn.execute("UPDATE pages SET user_edited=1 WHERE id='old_overview'", ())
                .await
                .unwrap();
        }
        assert!(!db
            .archive_legacy_overview_placeholder("old_overview", 1, 0, placeholder)
            .await
            .unwrap());
        {
            let conn = db.test_primary_session().await;
            conn.execute("UPDATE pages SET user_edited=0 WHERE id='old_overview'", ())
                .await
                .unwrap();
        }
        assert!(db
            .archive_legacy_overview_placeholder("old_overview", 1, 0, placeholder)
            .await
            .unwrap());
        let page = db.get_page("old_overview").await.unwrap().unwrap();
        assert_eq!(page.status, "archived");
        assert_eq!(
            page.content, placeholder,
            "archive retains the original instead of deleting it"
        );
    }

    #[tokio::test]
    async fn batch_status_reports_phase_done_failed_and_state() {
        use wenlan_types::import::{ImportPhase, ImportPhaseState};

        let (db, _dir) = crate::db::tests::test_db().await;
        seed_import_memory(&db, "import_phasebatch_0", "chatgpt", None).await;
        seed_import_memory(&db, "import_phasebatch_1", "chatgpt", None).await;

        for (sid, step, status) in [
            ("import_phasebatch_0", "entity_extract", "ok"),
            ("import_phasebatch_0", "entity_link", "ok"),
            ("import_phasebatch_0", "title_enrich", "skipped"),
            ("import_phasebatch_0", "page_growth", "failed"),
            ("import_phasebatch_1", "entity_extract", "ok"),
            ("import_phasebatch_1", "page_growth", "failed"),
        ] {
            db.record_enrichment_step(sid, step, status, None)
                .await
                .unwrap();
        }

        let status = db
            .import_batch_status("phasebatch")
            .await
            .unwrap()
            .expect("batch exists");
        assert_eq!(status.memories_imported, 2);
        assert_eq!(status.chunks_received, 1);
        assert_eq!(status.memories_skipped, 0);
        assert_eq!(status.source, "chatgpt");
        assert_eq!(status.started_at, 1_700_000_000);
        assert!(!status.complete);

        let phase = |p: ImportPhase| {
            status
                .phases
                .iter()
                .find(|s| s.phase == p)
                .expect("phase present")
        };
        // Detect: 2 ok + 1 ok of 4 rows; one row missing.
        let detect = phase(ImportPhase::Detect);
        assert_eq!((detect.done, detect.total, detect.failed), (3, 4, 0));
        assert_eq!(detect.state, ImportPhaseState::Running);
        // Enrich: the skipped title row counts as done.
        let enrich = phase(ImportPhase::Enrich);
        assert_eq!((enrich.done, enrich.total, enrich.failed), (1, 2, 0));
        assert_eq!(enrich.state, ImportPhaseState::Running);
        // Link: every row settled, failures present.
        let link = phase(ImportPhase::Link);
        assert_eq!((link.done, link.total, link.failed), (0, 2, 2));
        assert_eq!(link.state, ImportPhaseState::Failed);
        // Distill: no citations yet, still running with the rest unsettled.
        let distill = phase(ImportPhase::Distill);
        assert_eq!((distill.done, distill.total), (0, 0));
        assert_eq!(distill.state, ImportPhaseState::Running);
    }

    #[tokio::test]
    async fn batch_status_splits_entities_by_lifecycle() {
        let (db, _dir) = crate::db::tests::test_db().await;
        seed_import_memory(&db, "import_entbatch_0", "claude", None).await;
        seed_import_memory(&db, "import_entbatch_1", "claude", None).await;

        let detected = db
            .store_entity("Batch Detected", "person", None, None, None)
            .await
            .unwrap();
        let established = db
            .store_entity("Batch Established", "person", None, None, None)
            .await
            .unwrap();
        db.confirm_entity(&established, true).await.unwrap();
        db.link_memory_entities("import_entbatch_0", &[detected.as_str()])
            .await
            .unwrap();
        db.link_memory_entities("import_entbatch_1", &[established.as_str()])
            .await
            .unwrap();

        let status = db
            .import_batch_status("entbatch")
            .await
            .unwrap()
            .expect("batch exists");
        assert_eq!(status.entities_detected, 1);
        assert_eq!(status.entities_established, 1);
    }

    #[tokio::test]
    async fn batch_status_counts_distilled_pages_citing_members() {
        let (db, _dir) = crate::db::tests::test_db().await;
        seed_import_memory(&db, "import_distbatch_0", "other", None).await;
        seed_import_memory(&db, "import_distbatch_1", "other", None).await;

        // A citation edge is only writable when both endpoints exist and sit
        // in the edge's own space; `edges_space_fence` aborts on a missing
        // endpoint the same way it aborts on a mismatched one. So the citing
        // pages, and the one memory outside this batch, all have to be real
        // rows in the unfiled space the memories above were seeded into.
        let space = crate::db::UNFILED_SPACE_ID;
        seed_import_memory(&db, "import_otherbatch_0", "other", Some(space)).await;
        let session = db.test_primary_session().await;
        for page in ["page_dist_1", "page_dist_2"] {
            session
                .execute(
                    "INSERT INTO pages (id, title, content, source_memory_ids, version, \
                     status, created_at, last_compiled, last_modified, workspace, \
                     creation_kind, review_status) \
                     VALUES (?1, ?1, 'body', '[]', 1, 'active', 'now', 'now', 'now', ?2, \
                     'distilled', 'confirmed')",
                    libsql::params![page, space],
                )
                .await
                .expect("seed distilled page");
        }
        session
            .execute(
                &format!(
                    "INSERT INTO edges (edge_id, src_id, src_kind, dst_id, dst_kind, \
                     edge_type, lineage, grounded, space, created_at, valid_until) VALUES \
                     ('edge-dist-1', 'page_dist_1', 'page', 'import_distbatch_0', 'memory', \
                      'cites', 'evidence', 1, '{space}', 1700000001, NULL), \
                     ('edge-dist-2', 'page_dist_1', 'page', 'import_distbatch_1', 'memory', \
                      'cites', 'evidence', 1, '{space}', 1700000002, NULL), \
                     ('edge-dist-3', 'page_dist_2', 'page', 'import_otherbatch_0', 'memory', \
                      'cites', 'evidence', 1, '{space}', 1700000003, NULL)"
                ),
                (),
            )
            .await
            .expect("seed cites edges");
        drop(session);

        let status = db
            .import_batch_status("distbatch")
            .await
            .unwrap()
            .expect("batch exists");
        // One page cites two member memories; the non-member citation is out.
        assert_eq!(status.pages_distilled, 1);
        let distill = status
            .phases
            .iter()
            .find(|s| s.phase == wenlan_types::import::ImportPhase::Distill)
            .expect("distill phase");
        assert_eq!((distill.done, distill.total), (1, 0));
    }

    #[tokio::test]
    async fn abandoned_step_is_terminal_so_the_batch_settles() {
        let (db, _dir) = crate::db::tests::test_db().await;
        seed_import_memory(&db, "import_stuckbatch_0", "other", None).await;
        // What the entity lane writes once a memory has burned every attempt.
        // The retry selector never picks an abandoned row up again, so if the
        // rollup treats it as neither done nor failed the phase is one row
        // short of its total for good: Detect stays Running, the batch never
        // reports complete, and every poller keeps asking forever.
        for (step, status) in [
            ("entity_extract", "abandoned"),
            ("entity_link", "ok"),
            ("title_enrich", "ok"),
            ("page_growth", "ok"),
        ] {
            db.record_enrichment_step("import_stuckbatch_0", step, status, None)
                .await
                .expect("record step");
        }

        let status = db
            .import_batch_status("stuckbatch")
            .await
            .unwrap()
            .expect("batch exists");
        let detect = status
            .phases
            .iter()
            .find(|s| s.phase == wenlan_types::import::ImportPhase::Detect)
            .expect("detect phase");
        assert_eq!((detect.done, detect.failed, detect.total), (1, 1, 2));
        assert_eq!(
            detect.state,
            wenlan_types::import::ImportPhaseState::Failed,
            "one abandoned step of two fails the phase"
        );
        assert!(
            status.complete,
            "an abandoned step is terminal, so the batch settles instead of polling forever"
        );
    }

    #[tokio::test]
    async fn batch_status_keeps_case_variant_batch_ids_apart() {
        let (db, _dir) = crate::db::tests::test_db().await;
        seed_import_memory(&db, "import_CaseBatch_0", "other", None).await;
        seed_import_memory(&db, "import_casebatch_0", "other", None).await;
        // SQLite's default LIKE is case-insensitive, so the old pattern match
        // read both rows into whichever batch was asked for.
        let upper = db
            .import_batch_status("CaseBatch")
            .await
            .unwrap()
            .expect("batch exists");
        assert_eq!(upper.memories_imported, 1);
    }

    #[test]
    fn batch_id_that_could_claim_another_batch_is_rejected() {
        assert!(validate_batch_id("plain-batch").is_ok());
        assert!(validate_batch_id(&uuid::Uuid::new_v4().to_string()).is_ok());
        // `x` would otherwise match `import_x_0_7`, the ids an un-chunked
        // import with batch id `x_0` mints.
        assert!(validate_batch_id("x_0").is_err());
        assert!(validate_batch_id("").is_err());
        assert!(validate_batch_id(&"a".repeat(MAX_BATCH_ID_LEN + 1)).is_err());
    }

    #[tokio::test]
    async fn batch_status_unknown_id_returns_none() {
        let (db, _dir) = crate::db::tests::test_db().await;
        assert!(db
            .import_batch_status("missing-batch")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn two_chunks_sharing_batch_id_form_one_batch() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let cfg = crate::tuning::ConfidenceConfig::default();
        let space = crate::space_context::ResolvedWriteSpace::uncategorized();
        for (chunk, content) in [
            (
                0u32,
                "- core chunk zero alpha memory content\n- core chunk zero beta memory content",
            ),
            (
                1u32,
                "- core chunk one gamma memory content\n- core chunk one delta memory content",
            ),
        ] {
            let result = import_memories_no_llm_in_batch(
                &db,
                content,
                "other",
                None,
                &cfg,
                &space,
                Some("core-batch"),
                Some(chunk),
            )
            .await
            .unwrap();
            assert_eq!(result.batch_id, "core-batch");
            assert_eq!(result.imported, 2);
            assert_eq!(result.skipped, 0);
        }

        let status = db
            .import_batch_status("core-batch")
            .await
            .unwrap()
            .expect("batch exists");
        assert_eq!(status.memories_imported, 4);
        assert_eq!(status.chunks_received, 2);
        assert_eq!(status.source, "other");
    }

    #[tokio::test]
    async fn unchunked_import_keeps_legacy_source_id_shape() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let cfg = crate::tuning::ConfidenceConfig::default();
        let space = crate::space_context::ResolvedWriteSpace::uncategorized();
        let result = import_memories_no_llm_in_space(
            &db,
            "- legacy shape alpha memory content here\n- legacy shape beta memory content here",
            "chatgpt",
            None,
            &cfg,
            &space,
        )
        .await
        .unwrap();
        assert_eq!(result.imported, 2);

        let status = db
            .import_batch_status(&result.batch_id)
            .await
            .unwrap()
            .expect("batch exists");
        assert_eq!(status.memories_imported, 2);
        assert_eq!(status.chunks_received, 1);
        assert_eq!(status.source, "chatgpt");
    }

    #[tokio::test]
    async fn import_all_noise_input_reports_full_skipped_count() {
        // Every line here is parse-time noise (too short, a section header):
        // nothing lands in the DB, so this must not read as "nothing skipped".
        let (db, _dir) = crate::db::tests::test_db().await;
        let cfg = crate::tuning::ConfidenceConfig::default();
        let space = crate::space_context::ResolvedWriteSpace::uncategorized();
        let result = import_memories_no_llm_in_space(
            &db,
            "hi\nok\nTop of mind",
            "chatgpt",
            None,
            &cfg,
            &space,
        )
        .await
        .unwrap();
        assert_eq!(result.imported, 0);
        assert_eq!(result.skipped, 3);
    }

    #[tokio::test]
    async fn import_skipped_counts_parse_drops_and_db_duplicates() {
        // Cross-chunk duplicates: the second chunk's content already sits in
        // the DB from the first, so it must be counted alongside this
        // chunk's own parse-time drop.
        let (db, _dir) = crate::db::tests::test_db().await;
        let cfg = crate::tuning::ConfidenceConfig::default();
        let space = crate::space_context::ResolvedWriteSpace::uncategorized();
        let content = "- first durable import memory line\nhi\n[fact] - Uses Rust";

        let first = import_memories_no_llm_in_batch(
            &db,
            content,
            "other",
            None,
            &cfg,
            &space,
            Some("skip-count-batch"),
            Some(0),
        )
        .await
        .unwrap();
        assert_eq!(first.imported, 2);
        assert_eq!(first.skipped, 1);

        let second = import_memories_no_llm_in_batch(
            &db,
            content,
            "other",
            None,
            &cfg,
            &space,
            Some("skip-count-batch"),
            Some(1),
        )
        .await
        .unwrap();
        assert_eq!(second.imported, 0);
        assert_eq!(second.skipped, 3);
    }
}
