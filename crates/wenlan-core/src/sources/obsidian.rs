// SPDX-License-Identifier: Apache-2.0

use regex::Regex;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::chunker::ChunkingEngine;
use crate::sources::{MemoryType, RawDocument};

// ---------------------------------------------------------------------------
// Pre-compiled regexes (compiled once, reused across all calls)
// ---------------------------------------------------------------------------

static WIKILINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(!?)\[\[([^\]|#]+)(?:#([^\]|]+))?(?:\|([^\]]+))?\]\]").unwrap());

static INLINE_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[ \t])#([a-zA-Z][a-zA-Z0-9_/\-]*)").unwrap());

static DATE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap());

static WIKILINK_DETECT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[").unwrap());

// ---------------------------------------------------------------------------
// Frontmatter
// ---------------------------------------------------------------------------

/// Parsed YAML frontmatter from an Obsidian note.
#[derive(Debug, Clone, Default)]
pub struct NoteFrontmatter {
    pub fields: HashMap<String, serde_yaml::Value>,
}

impl NoteFrontmatter {
    /// Extract tags from the `tags` field (list of strings).
    pub fn tags(&self) -> Vec<String> {
        self.fields
            .get("tags")
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|item| item.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Extract aliases from the `aliases` field (list of strings).
    pub fn aliases(&self) -> Vec<String> {
        self.fields
            .get("aliases")
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|item| item.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get a string value by key.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(|v| v.as_str())
    }

    /// Whether the key is present at all, whatever its YAML type. Used for the
    /// self-recapture check, where a hand-mangled `origin_id: 12` must still
    /// count as "this file is our own projection".
    pub fn has(&self, key: &str) -> bool {
        self.fields.contains_key(key)
    }
}

/// Split content into its raw YAML frontmatter block and the body after it.
/// Returns `None` when there is no frontmatter: content does not start with
/// `---`, or there is no closing `\n---`. This is exactly the delimiter logic
/// `extract_frontmatter` uses to find the YAML block; the two cannot drift
/// because `extract_frontmatter` calls this function.
pub(crate) fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    // Frontmatter must start at the very beginning of the file
    let after_open = content.strip_prefix("---")?;

    // Find the closing "\n---" delimiter
    let pos = after_open.find("\n---")?;
    let yaml_str = &after_open[..pos];
    // "\n---" is exactly 4 ASCII bytes -- safe to index at find() boundary
    let body_after = &after_open[pos + 4..];
    let body = body_after.trim_start_matches('\n');
    Some((yaml_str, body))
}

/// Parse YAML frontmatter delimited by `---`. Returns the parsed frontmatter
/// and a reference to the body text after the closing `---`.
pub fn extract_frontmatter(content: &str) -> (NoteFrontmatter, &str) {
    match split_frontmatter(content) {
        Some((yaml_str, body)) => {
            let fields: HashMap<String, serde_yaml::Value> =
                serde_yaml::from_str(yaml_str).unwrap_or_default();
            (NoteFrontmatter { fields }, body)
        }
        None => (NoteFrontmatter::default(), content),
    }
}

// ---------------------------------------------------------------------------
// Code block ranges
// ---------------------------------------------------------------------------

/// Find byte ranges of fenced code blocks (``` or ~~~) so they can be skipped
/// during wikilink/tag extraction.
pub fn code_block_ranges(content: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut offset = 0;
    let mut in_block = false;
    let mut block_start = 0;
    let mut fence_char: u8 = 0;

    for line in content.split('\n') {
        let trimmed = line.trim_start();
        if !in_block {
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_block = true;
                block_start = offset;
                fence_char = trimmed.as_bytes()[0];
            }
        } else {
            // Closing fence must use the same character
            let closing = if fence_char == b'`' { "```" } else { "~~~" };
            if trimmed.starts_with(closing) {
                let block_end = offset + line.len();
                ranges.push(block_start..block_end);
                in_block = false;
            }
        }
        // +1 for the '\n' delimiter (split consumes it)
        offset += line.len() + 1;
    }
    // Unclosed code block extends to end
    if in_block {
        ranges.push(block_start..content.len());
    }
    ranges
}

/// Check whether a byte offset falls inside any code block range.
fn in_code_block(offset: usize, ranges: &[Range<usize>]) -> bool {
    ranges.iter().any(|r| r.contains(&offset))
}

// ---------------------------------------------------------------------------
// Wikilinks
// ---------------------------------------------------------------------------

/// A parsed `[[wikilink]]` from an Obsidian note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wikilink {
    pub target: String,
    pub heading: Option<String>,
    pub display: Option<String>,
    pub is_embed: bool,
}

/// A parsed `[[wikilink]]` carrying the target capture's byte range in the
/// source content. The M6 orphan-wikilink signal (`m6::signals`) uses the
/// range to test whether an occurrence falls inside a claim revision's
/// anchored span (`docs/plans/2026-08-03-m6-pr-a-followup-2-scope.md` §2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikilinkWithSpan {
    pub target: String,
    pub target_range: Range<usize>,
    pub heading: Option<String>,
    pub display: Option<String>,
    pub is_embed: bool,
}

/// Extract all `[[wikilinks]]` from content, skipping those inside code
/// blocks, carrying each target's byte range in `content`. The one regex and
/// code-block filter here also back [`extract_wikilinks`], so the write path
/// and M6 read the same occurrences.
pub fn extract_wikilinks_with_spans(content: &str) -> Vec<WikilinkWithSpan> {
    let code_ranges = code_block_ranges(content);

    WIKILINK_RE
        .captures_iter(content)
        .filter(|cap| {
            let m = cap.get(0).unwrap();
            !in_code_block(m.start(), &code_ranges)
        })
        .map(|cap| {
            let target = cap.get(2).unwrap();
            WikilinkWithSpan {
                is_embed: &cap[1] == "!",
                target: target.as_str().to_string(),
                target_range: target.start()..target.end(),
                heading: cap.get(3).map(|m| m.as_str().to_string()),
                display: cap.get(4).map(|m| m.as_str().to_string()),
            }
        })
        .collect()
}

/// Extract all `[[wikilinks]]` from content, skipping those inside code blocks.
pub fn extract_wikilinks(content: &str) -> Vec<Wikilink> {
    extract_wikilinks_with_spans(content)
        .into_iter()
        .map(|w| Wikilink {
            target: w.target,
            heading: w.heading,
            display: w.display,
            is_embed: w.is_embed,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Inline tags
// ---------------------------------------------------------------------------

/// Extract inline `#tag` references, skipping code blocks.
/// Supports nested tags like `#rust/async`.
pub fn extract_inline_tags(content: &str) -> Vec<String> {
    let code_ranges = code_block_ranges(content);

    INLINE_TAG_RE
        .captures_iter(content)
        .filter(|cap| {
            let m = cap.get(0).unwrap();
            !in_code_block(m.start(), &code_ranges)
        })
        .map(|cap| cap[1].to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Vault detection & scanning
// ---------------------------------------------------------------------------

/// Check if a directory is an Obsidian vault (contains `.obsidian/` directory).
pub fn is_obsidian_vault(path: &Path) -> bool {
    path.join(".obsidian").is_dir()
}

/// Classification of an Obsidian note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteType {
    Standard,
    Daily,
    Moc,
}

/// Detect the type of an Obsidian note based on path and content.
///
/// - **Daily**: path contains `daily/` or `journal/`, or filename matches `YYYY-MM-DD`
/// - **MOC** (Map of Content): at least three link lines, and more than half
///   of the non-empty, non-heading lines are link lines: a `[[wikilink]]`
///   with almost nothing else (see `is_link_line`). A short note whose
///   sentences mention other notes is written content, not an index.
/// - **Standard**: everything else
pub fn detect_note_type(path: &Path, content: &str) -> NoteType {
    // Check path components for daily/journal directories
    let path_str = path.to_string_lossy();
    for component in path.components() {
        let name = component.as_os_str().to_string_lossy();
        if name == "daily" || name == "journal" {
            return NoteType::Daily;
        }
    }

    // Check filename for YYYY-MM-DD pattern
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if DATE_RE.is_match(stem) {
            return NoteType::Daily;
        }
    }

    // Ignore path_str lint — it was used for the component check above
    let _ = path_str;

    // Check for MOC: mostly bare links, enough of them to be an index
    let body_lines: Vec<&str> = content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let link_lines = body_lines.iter().filter(|l| is_link_line(l)).count();
    if link_lines >= MOC_MIN_LINK_LINES && link_lines * 2 > body_lines.len() {
        return NoteType::Moc;
    }

    NoteType::Standard
}

/// Fewer link lines than this never make a note an index.
const MOC_MIN_LINK_LINES: usize = 3;

/// A line that is a `[[wikilink]]` with at most a couple of words around it,
/// like `- [[Note A]]` or `* [[Note B]] (draft)`. A sentence that mentions a
/// note, `Plan the beds with [[Maren]] next spring.`, is not one.
fn is_link_line(line: &str) -> bool {
    if !WIKILINK_DETECT_RE.is_match(line) {
        return false;
    }
    let rest = WIKILINK_RE.replace_all(line, " ");
    let mut words = 0usize;
    let mut cjk = 0usize;
    for token in rest.split_whitespace() {
        let mut latin = false;
        for c in token.chars() {
            if is_cjk(c) {
                cjk += 1;
            } else if c.is_alphanumeric() {
                latin = true;
            }
        }
        if latin {
            words += 1;
        }
    }
    // Two CJK characters read about as much as one word.
    words + cjk / 2 < 3
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

/// Why a note produced nothing to store, for the sync reply and the log.
/// Meaningful only for a note `note_to_documents` turned into nothing.
pub fn empty_note_reason(path: &Path, content: &str) -> &'static str {
    if detect_note_type(path, content) == NoteType::Moc {
        return SKIP_REASON_LINK_INDEX;
    }
    let (frontmatter, _) = extract_frontmatter(content);
    if frontmatter.has(PROJECTED_PAGE_FRONTMATTER_KEY) || has_projection_marker_line(content) {
        return SKIP_REASON_WENLAN_PAGE;
    }
    SKIP_REASON_EMPTY
}

/// A note that is a list of links to other notes (an index, or map of
/// content): the notes it links to are read instead.
pub const SKIP_REASON_LINK_INDEX: &str = "link_index";
/// One of Wenlan's own pages, written back into the folder.
pub const SKIP_REASON_WENLAN_PAGE: &str = "wenlan_page";
/// Nothing but frontmatter or whitespace.
pub const SKIP_REASON_EMPTY: &str = "empty";

/// Directories to skip when scanning a vault.
const SKIP_DIRS: &[&str] = &[".obsidian", ".trash", ".git", "templates"];

/// Frontmatter key that marks a `.md` file as one of Wenlan's own projected
/// pages (written by `export::knowledge::KnowledgeWriter::write_page`). Any
/// file carrying it is skipped on ingest — see `note_to_documents`.
pub(crate) const PROJECTED_PAGE_FRONTMATTER_KEY: &str = "origin_id";

/// Second, parser-independent look for the projection marker.
///
/// `extract_frontmatter` runs the block through `serde_yaml` and falls back to
/// an EMPTY map on any parse error, so ONE malformed line anywhere in the
/// frontmatter — a stray tab, an unquoted `: ` in a title — makes
/// `NoteFrontmatter::has` answer "no `origin_id` here" for a file that plainly
/// has one. That failure is fail-OPEN in the direction that matters: the
/// projection gets ingested. Meanwhile `export::knowledge::read_origin_id`
/// never parses YAML at all — it scans the block for a line starting with
/// `origin_id:` — so the writer would still claim the same file. This mirrors
/// that line-prefix scan so the two sides cannot disagree about what is ours.
///
/// Deliberately broader than `read_origin_id` in one place: that function
/// discards an empty value, this one treats the key's mere presence as enough.
/// A page whose `origin_id` got blanked is still our output, and the cost of
/// being wrong here is one skipped note, not a poisoned graph.
pub(crate) fn has_projection_marker_line(content: &str) -> bool {
    let mut lines = content.lines();
    if lines.next() != Some("---") {
        return false;
    }
    lines
        .take_while(|line| *line != "---")
        .any(|line| line.starts_with(&format!("{PROJECTED_PAGE_FRONTMATTER_KEY}:")))
}

/// Check if a path should be skipped (any component matches a skip directory).
pub fn should_skip(path: &Path) -> bool {
    path.components().any(|c| {
        let name = c.as_os_str().to_string_lossy();
        SKIP_DIRS.contains(&name.as_ref())
    })
}

/// Recursively scan an Obsidian vault for `.md` files, skipping excluded directories.
pub fn scan_vault(root: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    scan_dir_recursive(root, root, &mut results);
    results
}

fn scan_dir_recursive(root: &Path, dir: &Path, results: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // Compute relative path for skip checking
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if should_skip(relative) {
            continue;
        }

        if path.is_dir() {
            scan_dir_recursive(root, &path, results);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            results.push(path);
        }
    }
}

/// Short-circuit check: returns `true` as soon as any `.md` file is found in
/// `root` or any (non-skipped) subdirectory. Used at source registration time
/// so we don't need to walk the whole vault (which can be slow on large
/// knowledge bases) just to validate "has at least one markdown file".
pub fn has_any_markdown(root: &Path) -> bool {
    has_any_markdown_recursive(root, root)
}

fn has_any_markdown_recursive(root: &Path, dir: &Path) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if should_skip(relative) {
            continue;
        }

        if path.is_dir() {
            if has_any_markdown_recursive(root, &path) {
                return true;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Note → RawDocument conversion
// ---------------------------------------------------------------------------

/// Convert an Obsidian note into one or more `RawDocument` structs, using the
/// `ChunkingEngine` for header-based splitting. MOC notes are skipped (they are
/// structural, not content).
///
/// TODO(schema): The memory row schema overloads `source` (kind of entry) and
/// `source_agent` (who created it). We set `source = "memory"` so these rows
/// appear in the main Memory view (list_memories filters WHERE source = 'memory'),
/// and `source_agent = "obsidian"` to preserve origin for UI differentiation.
/// A future refactor should rename: source → kind, source_agent → source,
/// source_id → external_id. Until then, new source connectors should follow
/// this pattern: `source = "memory"` + distinctive `source_agent`.
///
/// A connector's distinctive string is no longer enough on its own: a
/// document-ingest connector must also add that string to
/// `origin::DOCUMENT_INGEST_SOURCE_AGENTS`, which is what makes its documents
/// groundable and simultaneously bars a wire request from claiming the value.
/// The old "invent a new string" guidance silently broke the grounding gate
/// for every connector after `folder`, including this one.
pub fn note_to_documents(
    source_id: &str,
    path: &Path,
    content: &str,
    mtime: i64,
) -> Vec<RawDocument> {
    // MOCs are structural index pages — skip them
    if detect_note_type(path, content) == NoteType::Moc {
        return Vec::new();
    }

    let (frontmatter, body) = extract_frontmatter(content);

    // Self-recapture exclusion (close plan Part A item 1, rider). A `.md` file
    // carrying `origin_id` frontmatter is one of OUR projected pages —
    // `KnowledgeWriter::write_page` stamps it, and `read_origin_id` reads it
    // back to re-attribute files. Re-ingesting one would turn the system's own
    // distilled prose into a "document ingest", which the grounding gate
    // promotes: the graph would end up voting for its own output.
    //
    // `is_reserved_ingest_root` only refuses the pages directory as a source
    // ROOT. It cannot see a projection nested inside a watched vault (the
    // common setup when `knowledge_path` points into the vault), a parent
    // directory registered as a source, or a page file someone copied
    // elsewhere. The frontmatter travels with the file, so checking it here
    // closes all of those — and it sits on the shared path, so the folder
    // watcher's markdown branch (`directory::file_to_documents`) is covered by
    // the same three lines.
    // Two detections, because the YAML parse is fail-open on malformed
    // frontmatter — see `has_projection_marker_line`.
    if frontmatter.has(PROJECTED_PAGE_FRONTMATTER_KEY) || has_projection_marker_line(content) {
        // Say so. A silent skip is indistinguishable from a broken watcher to
        // the user whose note never showed up, and this is the one skip that
        // can fire on a file they wrote by hand (they pasted our frontmatter,
        // or their vault holds a copied projection).
        log::info!(
            "[ingest] skipping {} — carries `{PROJECTED_PAGE_FRONTMATTER_KEY}` frontmatter, \
             so it is one of Wenlan's own projected pages, not a source document",
            path.display()
        );
        return Vec::new();
    }

    // Map frontmatter to memory fields
    let space = frontmatter.tags().into_iter().next();
    let memory_type = frontmatter
        .get_str("type")
        .and_then(|t| t.parse::<MemoryType>().ok())
        .map(|mt| mt.to_string());

    let mut metadata = HashMap::new();
    let tags = frontmatter.tags();
    if !tags.is_empty() {
        metadata.insert("tags".to_string(), tags.join(","));
    }
    let aliases = frontmatter.aliases();
    if !aliases.is_empty() {
        metadata.insert("aliases".to_string(), aliases.join(","));
    }
    metadata.insert("extension".to_string(), "md".to_string());

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();

    note_chunks_to_documents(NoteChunks {
        source_id,
        path,
        title: stem,
        body,
        metadata,
        space,
        memory_type,
        mtime,
        source_agent: "obsidian",
    })
}

/// A note ready to chunk: what [`note_to_documents`] and the Logseq reader
/// (`sources::logseq`) have parsed out of one file.
pub(crate) struct NoteChunks<'a> {
    pub source_id: &'a str,
    pub path: &'a Path,
    pub title: String,
    pub body: &'a str,
    pub metadata: HashMap<String, String>,
    pub space: Option<String>,
    pub memory_type: Option<String>,
    pub mtime: i64,
    /// Must be listed in `origin::DOCUMENT_INGEST_SOURCE_AGENTS`.
    pub source_agent: &'static str,
}

/// Chunk a note's body and build one `RawDocument` per chunk, with ids
/// `<source_id>::<path>::<n>`.
pub(crate) fn note_chunks_to_documents(note: NoteChunks<'_>) -> Vec<RawDocument> {
    let NoteChunks {
        source_id,
        path,
        title: stem,
        body,
        metadata,
        space,
        memory_type,
        mtime,
        source_agent,
    } = note;
    let path_str = path.to_string_lossy();

    // Chunk the body using the engine
    let engine = ChunkingEngine::new();
    let chunks = engine.chunk(body, &stem, &path_str, &metadata);

    // If body is empty or chunker produced nothing, create a single document
    if chunks.is_empty() {
        let source_id_full = format!("{}::{}::0", source_id, path_str);
        return vec![RawDocument {
            source: "memory".to_string(),
            source_id: source_id_full,
            title: stem,
            content: body.to_string(),
            last_modified: mtime,
            metadata,
            space,
            memory_type,
            source_agent: Some(source_agent.to_string()),
            ..Default::default()
        }];
    }

    // Build a RawDocument per chunk. Note: noise/quality filtering happens in
    // the server's vault sync via the shared `QualityGate`; this function
    // stays pure and just maps chunks to documents.
    chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| {
            let title = if chunks.len() == 1 {
                stem.clone()
            } else {
                match &chunk.semantic_unit {
                    Some(unit) => format!("{} ({})", stem, unit),
                    None => format!("{} (chunk {})", stem, i),
                }
            };
            let source_id_full = format!("{}::{}::{}", source_id, path_str, i);
            RawDocument {
                source: "memory".to_string(),
                source_id: source_id_full,
                title,
                content: chunk.content.clone(),
                last_modified: mtime,
                metadata: metadata.clone(),
                space: space.clone(),
                memory_type: memory_type.clone(),
                source_agent: Some(source_agent.to_string()),
                ..Default::default()
            }
        })
        .collect()
}

// NOTE: HTML comment stripping was considered but dropped. The text-splitter
// chunker merges short sections until they reach MIN_CHARS (800), so template
// placeholders like `<!-- Process with /capture -->` no longer exist as
// standalone chunks — they get merged into larger chunks with real content,
// and the QualityGate filters the combined result. Preserving comments also
// keeps legitimate user notes (`<!-- TODO: research auth -->`) intact.

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Frontmatter tests
    #[test]
    fn test_extract_frontmatter_basic() {
        let content = "---\ntitle: My Note\ntags:\n  - work\n  - rust\n---\n\n# Hello\nBody text.";
        let (fm, body) = extract_frontmatter(content);
        assert_eq!(fm.get_str("title"), Some("My Note"));
        assert_eq!(fm.tags(), vec!["work", "rust"]);
        assert!(body.starts_with("# Hello"));
    }

    #[test]
    fn test_extract_frontmatter_none() {
        let content = "# No frontmatter\nJust text.";
        let (fm, body) = extract_frontmatter(content);
        assert!(fm.fields.is_empty());
        assert_eq!(body, content);
    }

    #[test]
    fn test_extract_frontmatter_aliases() {
        let content = "---\naliases:\n  - Project X\n  - PX\n---\nBody";
        let (fm, _) = extract_frontmatter(content);
        assert_eq!(fm.aliases(), vec!["Project X", "PX"]);
    }

    // Wikilink tests
    #[test]
    fn test_extract_wikilinks_basic() {
        let content = "See [[Project Alpha]] and [[Beta#heading|display text]].";
        let links = extract_wikilinks(content);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "Project Alpha");
        assert_eq!(links[0].heading, None);
        assert!(!links[0].is_embed);
        assert_eq!(links[1].target, "Beta");
        assert_eq!(links[1].heading, Some("heading".to_string()));
        assert_eq!(links[1].display, Some("display text".to_string()));
    }

    #[test]
    fn test_extract_wikilinks_embed() {
        let content = "![[image.png]] and ![[note#section]]";
        let links = extract_wikilinks(content);
        assert_eq!(links.len(), 2);
        assert!(links[0].is_embed);
        assert_eq!(links[0].target, "image.png");
        assert!(links[1].is_embed);
    }

    #[test]
    fn test_extract_wikilinks_with_spans_basic() {
        let content = "See [[Project Alpha]] and [[Beta#heading|display text]].";
        let links = extract_wikilinks_with_spans(content);
        assert_eq!(links.len(), 2);
        assert_eq!(&content[links[0].target_range.clone()], "Project Alpha");
        assert_eq!(&content[links[1].target_range.clone()], "Beta");
        // extract_wikilinks must report the identical occurrences, since it
        // is now expressed in terms of this function.
        let plain = extract_wikilinks(content);
        assert_eq!(plain.len(), links.len());
        for (p, s) in plain.iter().zip(links.iter()) {
            assert_eq!(p.target, s.target);
        }
    }

    #[test]
    fn test_extract_wikilinks_skips_code_blocks() {
        let content = "Real [[link]] here.\n\n```\n[[not a link]]\n```\n\nAnother [[real link]].";
        let links = extract_wikilinks(content);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "link");
        assert_eq!(links[1].target, "real link");
    }

    #[test]
    fn test_extract_inline_tags() {
        let content = "This is #work and #rust/async stuff.\n```\n#not-a-tag\n```";
        let tags = extract_inline_tags(content);
        assert_eq!(tags, vec!["work", "rust/async"]);
    }

    #[test]
    fn test_code_block_ranges_tilde() {
        let content = "before\n~~~\ncode\n~~~\nafter";
        let ranges = code_block_ranges(content);
        assert_eq!(ranges.len(), 1);
    }

    // Vault scanning tests
    #[test]
    fn test_is_obsidian_vault() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(!is_obsidian_vault(dir.path()));
        std::fs::create_dir(dir.path().join(".obsidian")).unwrap();
        assert!(is_obsidian_vault(dir.path()));
    }

    #[test]
    fn test_detect_note_type_daily() {
        assert_eq!(
            detect_note_type(Path::new("daily/2026-04-09.md"), "# Morning\nNotes"),
            NoteType::Daily
        );
        assert_eq!(
            detect_note_type(Path::new("journal/entry.md"), "stuff"),
            NoteType::Daily
        );
        assert_eq!(
            detect_note_type(Path::new("2026-04-09.md"), "stuff"),
            NoteType::Daily
        );
    }

    #[test]
    fn test_detect_note_type_moc() {
        let content =
            "# Index\n- [[Note A]]\n- [[Note B]]\n- [[Note C]]\n- [[Note D]]\n- [[Note E]]\nOne line of prose.";
        assert_eq!(
            detect_note_type(Path::new("index.md"), content),
            NoteType::Moc
        );
    }

    #[test]
    fn short_note_whose_sentences_link_is_not_an_index() {
        // The note from the 2026-10-22 fresh-install test: three lines, two
        // with links. It used to be dropped as a map of content.
        let content = "Soil plan for the spring beds.\nMix compost from [[Compost Log]] into the north bed.\nAsk [[Maren]] which cover crop she wants.";
        assert_eq!(
            detect_note_type(Path::new("Soil Plan.md"), content),
            NoteType::Standard
        );
        assert!(!note_to_documents("v", Path::new("/v/Soil Plan.md"), content, 0).is_empty());
        let chinese = "春天的土壤計畫。\n把[[堆肥紀錄]]裡的堆肥拌進北邊的菜圃。\n問[[瑪倫]]她想種哪一種覆蓋作物。";
        assert_eq!(
            detect_note_type(Path::new("土壤.md"), chinese),
            NoteType::Standard
        );
    }

    #[test]
    fn two_bare_links_are_not_enough_for_an_index() {
        let content = "- [[Note A]]\n- [[Note B]]";
        assert_eq!(
            detect_note_type(Path::new("pair.md"), content),
            NoteType::Standard
        );
    }

    #[test]
    fn empty_note_reason_names_the_rule() {
        let index = "# Index\n- [[A]]\n- [[B]]\n- [[C]]";
        assert_eq!(
            empty_note_reason(Path::new("i.md"), index),
            SKIP_REASON_LINK_INDEX
        );
        assert_eq!(
            empty_note_reason(Path::new("e.md"), "---\ntags: [x]\n---\n"),
            SKIP_REASON_EMPTY
        );
    }

    #[test]
    fn test_detect_note_type_standard() {
        let content = "# My Thoughts\n\nThis is a regular note about something interesting.\n\nIt has [[one link]] but mostly prose.";
        assert_eq!(
            detect_note_type(Path::new("thoughts.md"), content),
            NoteType::Standard
        );
    }

    #[test]
    fn test_should_skip_path() {
        assert!(should_skip(Path::new(".obsidian/plugins/foo.json")));
        assert!(should_skip(Path::new(".trash/deleted.md")));
        assert!(should_skip(Path::new(".git/config")));
        assert!(should_skip(Path::new("templates/daily.md")));
        assert!(!should_skip(Path::new("notes/my-note.md")));
    }

    #[test]
    fn test_scan_vault_files() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".obsidian")).unwrap();
        std::fs::create_dir(dir.path().join(".trash")).unwrap();
        std::fs::create_dir(dir.path().join("notes")).unwrap();

        std::fs::write(dir.path().join("root.md"), "# Root").unwrap();
        std::fs::write(dir.path().join("notes/sub.md"), "# Sub").unwrap();
        std::fs::write(dir.path().join(".obsidian/app.json"), "{}").unwrap();
        std::fs::write(dir.path().join(".trash/deleted.md"), "gone").unwrap();
        std::fs::write(dir.path().join("image.png"), "binary").unwrap();

        let files = scan_vault(dir.path());
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|p| p.ends_with("root.md")));
        assert!(files.iter().any(|p| p.ends_with("sub.md")));
    }

    // note_to_documents tests
    #[test]
    fn test_note_to_raw_documents() {
        let content = "---\ntags:\n  - work\ntype: decision\n---\n\n# Heading\n\nFirst section.\n\n## Details\n\nSecond section with [[Other Note]].";
        let path = Path::new("/vault/my-note.md");
        let source_id = "obsidian-main";

        let docs = note_to_documents(source_id, path, content, 1712678400);
        assert!(!docs.is_empty());
        // Obsidian imports are stored as curated memory (source = "memory"),
        // with origin identified via source_agent = "obsidian".
        assert_eq!(docs[0].source, "memory");
        assert_eq!(docs[0].source_agent, Some("obsidian".to_string()));
        assert_eq!(docs[0].space, Some("work".to_string()));
        assert_eq!(docs[0].memory_type, Some("decision".to_string()));
        assert!(docs[0].source_id.starts_with("obsidian-main::"));
    }

    #[test]
    fn test_note_to_raw_documents_no_frontmatter() {
        let content = "Just plain text without any YAML.";
        let path = Path::new("/vault/plain.md");
        let docs = note_to_documents("src1", path, content, 1712678400);
        assert_eq!(docs.len(), 1);
        assert!(docs[0].space.is_none());
        assert!(docs[0].memory_type.is_none());
    }

    #[test]
    fn projected_page_is_skipped_by_the_scanner() {
        // A page Wenlan projected into the vault carries `origin_id`. Ingesting
        // it would let the system's own distilled prose come back as a
        // "document ingest" and ground — the graph believing its own output.
        let content = "---\ntitle: \"Alice\"\norigin_id: page_abc123\norigin_version: 3\n---\n\nAlice works on ProjectX. She joined in 2024 and leads the ranking work.\n";
        let path = Path::new("/vault/alice.md");
        let docs = note_to_documents("obsidian-main", path, content, 1712678400);
        assert!(
            docs.is_empty(),
            "a projected page must never be re-ingested as an external document"
        );
    }

    #[test]
    fn projected_page_is_skipped_through_the_folder_watcher_too() {
        // The folder watcher's markdown branch funnels through the same
        // `note_to_documents`, so the exclusion covers a projection sitting
        // inside any watched directory, not just a registered vault.
        let dir = tempfile::TempDir::new().unwrap();
        let page = dir.path().join("projected.md");
        std::fs::write(
            &page,
            "---\norigin_id: page_abc123\n---\n\nAlice works on ProjectX and leads ranking.\n",
        )
        .unwrap();

        let outcome = crate::sources::directory::file_to_documents("folder-src", &page, None);
        assert!(
            matches!(outcome, crate::sources::directory::FileOutcome::Skipped(_)),
            "expected the projected page to be skipped, got {outcome:?}"
        );
    }

    #[test]
    fn a_non_string_origin_id_still_counts_as_projected() {
        // The check is presence, not type: a hand-mangled `origin_id` must not
        // become a re-ingestion loophole.
        let content = "---\norigin_id: 12345\n---\n\nAlice works on ProjectX and leads ranking.\n";
        let docs = note_to_documents("src1", Path::new("/vault/n.md"), content, 1712678400);
        assert!(docs.is_empty());
    }

    #[test]
    fn malformed_frontmatter_does_not_reopen_the_projection_loophole() {
        // `extract_frontmatter` swallows a YAML error into an EMPTY map, so the
        // parsed check alone answers "no origin_id" for a file that visibly has
        // one — fail-open, and exactly the direction that poisons the graph.
        // `export::knowledge::read_origin_id` scans lines instead of parsing, so
        // it would still claim this file as ours; the two must not disagree.
        //
        // The unbalanced quote is what breaks the parse. Assert that first, so
        // this test fails loudly if serde_yaml ever starts accepting it and the
        // case stops exercising the fallback at all.
        let content = "---\ntitle: \"unclosed\norigin_id: page_abc123\n---\n\nAlice works on ProjectX and leads the ranking work.\n";
        let (frontmatter, _) = extract_frontmatter(content);
        assert!(
            !frontmatter.has(PROJECTED_PAGE_FRONTMATTER_KEY),
            "the YAML parse must actually be failing here, or this test proves nothing"
        );

        let docs = note_to_documents("src1", Path::new("/vault/n.md"), content, 1712678400);
        assert!(
            docs.is_empty(),
            "the line-prefix fallback must catch what the parser dropped"
        );
    }

    #[test]
    fn test_moc_produces_no_documents() {
        let content = "# Index\n- [[A]]\n- [[B]]\n- [[C]]\n- [[D]]\n- [[E]]";
        let path = Path::new("/vault/moc.md");
        let docs = note_to_documents("src1", path, content, 1712678400);
        assert!(docs.is_empty());
    }

    #[test]
    fn test_note_to_documents_metadata_fields() {
        let content = "---\ntags:\n  - work\n  - rust\naliases:\n  - Project X\n  - PX\n---\n\nSome body content.";
        let path = Path::new("/vault/project.md");
        let docs = note_to_documents("obs1", path, content, 1712678400);
        assert!(!docs.is_empty());
        assert_eq!(docs[0].metadata.get("tags").unwrap(), "work,rust");
        assert_eq!(docs[0].metadata.get("aliases").unwrap(), "Project X,PX");
        assert_eq!(docs[0].metadata.get("extension").unwrap(), "md");
    }

    #[test]
    fn test_note_to_documents_invalid_memory_type() {
        let content = "---\ntype: bogus\n---\n\nBody text.";
        let path = Path::new("/vault/bad-type.md");
        let docs = note_to_documents("src1", path, content, 1712678400);
        assert_eq!(docs.len(), 1);
        // Invalid type should result in None, not a crash
        assert!(docs[0].memory_type.is_none());
    }

    #[test]
    fn test_note_to_documents_multi_chunk_titles() {
        let content = "---\ntags:\n  - dev\n---\n\n# Architecture\n\nThis section describes the architecture of the system in detail with enough text to matter.\n\n## Database Layer\n\nThe database layer uses SQLite with vector extensions for hybrid search capability.\n\n## API Layer\n\nThe API layer exposes REST endpoints for external tools to integrate with the system.";
        let path = Path::new("/vault/architecture.md");
        let docs = note_to_documents("obs1", path, content, 1712678400);
        assert!(!docs.is_empty());
        for doc in &docs {
            assert_eq!(doc.source, "memory");
            assert_eq!(doc.source_agent, Some("obsidian".to_string()));
            assert_eq!(doc.space, Some("dev".to_string()));
        }
        if docs.len() > 1 {
            assert!(docs[1].title.contains("architecture"));
        }
    }

    #[test]
    fn test_note_to_documents_last_modified() {
        let content = "# Simple note\n\nWith content.";
        let path = Path::new("/vault/simple.md");
        let docs = note_to_documents("src1", path, content, 1712678400);
        assert_eq!(docs[0].last_modified, 1712678400);
    }

    #[test]
    fn test_obsidian_full_import_flow() {
        let vault_dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(vault_dir.path().join(".obsidian")).unwrap();

        // Standard note with frontmatter
        std::fs::write(
            vault_dir.path().join("rust-basics.md"),
            "---\ntags:\n  - rust\ntype: fact\n---\n\n# Rust Basics\n\nOwnership is core.\n\n## Borrowing\n\nReferences don't own data.",
        )
        .unwrap();

        // Daily note
        std::fs::create_dir(vault_dir.path().join("daily")).unwrap();
        std::fs::write(
            vault_dir.path().join("daily/2026-04-09.md"),
            "# Morning\nHad coffee.\n\n## Afternoon\nWrote code.",
        )
        .unwrap();

        // MOC (should be skipped)
        std::fs::write(
            vault_dir.path().join("index.md"),
            "# Index\n- [[Rust Basics]]\n- [[Other]]\n- [[Another]]\n- [[More]]\n- [[Even More]]",
        )
        .unwrap();

        // Non-md file (should be ignored by scan)
        std::fs::write(vault_dir.path().join("image.png"), "binary").unwrap();

        // Scan vault
        let files = scan_vault(vault_dir.path());
        assert_eq!(files.len(), 3); // rust-basics, daily note, index

        // Convert all to documents
        let mut all_docs = Vec::new();
        for file in &files {
            let content = std::fs::read_to_string(file).unwrap();
            let docs = note_to_documents("test-vault", file, &content, 1712678400);
            all_docs.extend(docs);
        }

        // MOC should produce no documents
        // Non-MOC files should produce at least one chunk each (2 files)
        assert!(all_docs.len() >= 2);
        assert!(all_docs.iter().all(|d| d.source == "memory"));
        assert!(all_docs
            .iter()
            .all(|d| d.source_agent == Some("obsidian".to_string())));
        assert!(all_docs.iter().any(|d| d.space == Some("rust".to_string())));
        assert!(all_docs
            .iter()
            .any(|d| d.memory_type == Some("fact".to_string())));

        // Verify no MOC content leaked through
        assert!(!all_docs
            .iter()
            .any(|d| d.title.contains("Index") || d.title.contains("index")));
    }
}
