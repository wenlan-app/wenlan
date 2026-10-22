// SPDX-License-Identifier: Apache-2.0
//! Logseq graph reader: the file-based (Markdown) Logseq graph format.
//!
//! A graph is a folder holding `logseq/` (settings and backups), `pages/` and
//! `journals/`. Only `.md` files under `pages/` and `journals/` are read, so
//! the copies Logseq keeps in `logseq/bak/` are never stored twice.
//!
//! Logseq writes every page as an outline of `- ` blocks with `key:: value`
//! properties. [`page_to_documents`] turns that into plain Markdown before
//! chunking, so search and page writing see prose:
//!
//! - top-level blocks become paragraphs (a `- # Title` block a heading),
//!   nested blocks stay a list one level shallower;
//! - page properties are read (`title::`, `tags::`, `alias::`, `type::`) and
//!   block properties (`id::`, `collapsed::`, ...) are dropped;
//! - a block reference `((uuid))` becomes the referenced block's text when
//!   that block is on the same page, and is dropped otherwise;
//! - `{{embed [[page]]}}` becomes `[[page]]`; other `{{...}}` macros are
//!   dropped;
//! - the title is the `title::` property, else the file name decoded the way
//!   Logseq encodes it (`a___b` is `a/b`, `%3F` is `?`); a journal file
//!   `2026_10_21.md` is titled `2026-10-21`.
//!
//! The new Logseq "DB version" keeps a graph in a database, not in files; it
//! is not read here.

use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use super::obsidian::{
    extract_frontmatter, has_projection_marker_line, note_chunks_to_documents, NoteChunks,
    PROJECTED_PAGE_FRONTMATTER_KEY,
};
use crate::sources::{MemoryType, RawDocument};

/// The `source_agent` of every Logseq document.
pub const LOGSEQ_SOURCE_AGENT: &str = "logseq";

/// The graph folders that hold pages. Everything else (`logseq/`, `assets/`,
/// `draws/`, `whiteboards/`) is settings, backups or media.
const PAGE_DIRS: &[&str] = &["pages", "journals"];

static PROPERTY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z0-9_][A-Za-z0-9_\-./]*)::(?:\s+(.*))?$").unwrap());

static BLOCK_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(\(([0-9A-Fa-f-]{36})\)\)").unwrap());

static EMBED_PAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{embed \[\[([^\]]+)\]\]\}\}").unwrap());

static EMBED_BLOCK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{embed (\(\([0-9A-Fa-f-]{36}\)\))\}\}").unwrap());

static MACRO_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{[^}]*\}\}").unwrap());

static JOURNAL_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{4})_(\d{2})_(\d{2})$").unwrap());

static LINK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[[^\]]*\]\]").unwrap());

static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#\S+").unwrap());

// ---------------------------------------------------------------------------
// Graph detection & scanning
// ---------------------------------------------------------------------------

/// Whether `path` is the root of a Logseq graph: a `logseq/` folder next to
/// `pages/` or `journals/`.
pub fn is_logseq_graph(path: &Path) -> bool {
    path.join("logseq").is_dir() && PAGE_DIRS.iter().any(|dir| path.join(dir).is_dir())
}

/// Every `.md` page of the graph under `pages/` and `journals/`.
pub fn scan_graph(root: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    for dir in PAGE_DIRS {
        scan_dir(&root.join(dir), &mut results, false);
    }
    results
}

/// Whether the graph holds at least one page. Stops at the first one.
pub fn has_any_page(root: &Path) -> bool {
    let mut found = Vec::new();
    PAGE_DIRS.iter().any(|dir| {
        scan_dir(&root.join(dir), &mut found, true);
        !found.is_empty()
    })
}

fn scan_dir(dir: &Path, results: &mut Vec<PathBuf>, first_only: bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if first_only && !results.is_empty() {
            return;
        }
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            scan_dir(&path, results, first_only);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            results.push(path);
        }
    }
}

// ---------------------------------------------------------------------------
// Titles
// ---------------------------------------------------------------------------

/// The page title Logseq shows for a file, without its `title::` property:
/// a journal's date, or the decoded file name.
pub fn title_from_path(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled");
    let in_journals = path
        .parent()
        .and_then(|p| p.file_name())
        .is_some_and(|name| name == "journals");
    if in_journals {
        if let Some(date) = JOURNAL_NAME_RE.captures(stem) {
            return format!("{}-{}-{}", &date[1], &date[2], &date[3]);
        }
    }
    percent_decode(&stem.replace("___", "/"))
}

/// Decode `%XX` escapes. An escape that is not valid hex, or bytes that are
/// not UTF-8, are kept as written.
fn percent_decode(text: &str) -> String {
    fn hex(byte: u8) -> Option<u8> {
        (byte as char).to_digit(16).map(|d| d as u8)
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(high << 4 | low);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

// ---------------------------------------------------------------------------
// Outline -> Markdown
// ---------------------------------------------------------------------------

/// One `- ` block: its depth, first line, and the lines under it.
#[derive(Debug, Default)]
struct Block {
    level: usize,
    first: String,
    rest: Vec<String>,
    properties: Vec<(String, String)>,
}

/// A page split into its page properties and its blocks.
#[derive(Debug, Default)]
struct Outline {
    properties: Vec<(String, String)>,
    blocks: Vec<Block>,
}

/// Indent depth of a line: a tab or two spaces is one level.
fn indent_of(line: &str) -> (usize, &str) {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let indent = &line[..line.len() - trimmed.len()];
    let tabs = indent.chars().filter(|c| *c == '\t').count();
    let spaces = indent.chars().filter(|c| *c == ' ').count();
    (tabs + spaces / 2, trimmed)
}

fn bullet_text(trimmed: &str) -> Option<&str> {
    if trimmed == "-" {
        Some("")
    } else {
        trimmed.strip_prefix("- ")
    }
}

fn property(line: &str) -> Option<(String, String)> {
    let caps = PROPERTY_RE.captures(line.trim())?;
    Some((
        caps[1].to_ascii_lowercase(),
        caps.get(2).map_or("", |m| m.as_str()).trim().to_string(),
    ))
}

fn is_fence(trimmed: &str) -> bool {
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

fn parse_outline(body: &str) -> Outline {
    let mut outline = Outline::default();
    let mut current: Option<Block> = None;
    let mut in_fence = false;
    for line in body.lines() {
        let (level, trimmed) = indent_of(line);
        if !in_fence {
            if let Some(text) = bullet_text(trimmed) {
                if let Some(block) = current.take() {
                    outline.blocks.push(block);
                }
                let mut block = Block {
                    level,
                    ..Block::default()
                };
                match property(text) {
                    Some(prop) => block.properties.push(prop),
                    None => block.first = text.to_string(),
                }
                in_fence = is_fence(text);
                current = Some(block);
                continue;
            }
        }
        let Some(block) = current.as_mut() else {
            // Lines before the first block: page properties, or text.
            if let Some(prop) = property(line) {
                outline.properties.push(prop);
            } else if !line.trim().is_empty() {
                outline.blocks.push(Block {
                    first: line.trim().to_string(),
                    ..Block::default()
                });
            }
            continue;
        };
        if !in_fence {
            if let Some(prop) = property(trimmed) {
                block.properties.push(prop);
                continue;
            }
        }
        // Continuation lines sit two columns right of the bullet.
        let mut rest = line;
        for _ in 0..=block.level {
            rest = rest
                .strip_prefix('\t')
                .or_else(|| rest.strip_prefix("  "))
                .unwrap_or(rest);
        }
        if block.first.is_empty() && block.rest.is_empty() {
            block.first = rest.to_string();
        } else {
            block.rest.push(rest.to_string());
        }
        if is_fence(trimmed) {
            in_fence = !in_fence;
        }
    }
    if let Some(block) = current {
        outline.blocks.push(block);
    }
    // Logseq also writes page properties as a first block that holds
    // nothing else.
    if outline.properties.is_empty() {
        if let Some(first) = outline.blocks.first() {
            if first.first.is_empty() && first.rest.is_empty() && !first.properties.is_empty() {
                let first = outline.blocks.remove(0);
                outline.properties = first.properties;
            }
        }
    }
    outline
}

fn block_text(block: &Block) -> String {
    let mut text = block.first.clone();
    for line in &block.rest {
        text.push('\n');
        text.push_str(line);
    }
    text
}

/// Replace macros and block references with readable text.
fn clean_inline(text: &str, refs: &HashMap<String, String>) -> String {
    let text = EMBED_PAGE_RE.replace_all(text, "[[$1]]");
    let text = EMBED_BLOCK_RE.replace_all(&text, "$1");
    let text = MACRO_RE.replace_all(&text, "");
    let text = BLOCK_REF_RE.replace_all(&text, |caps: &regex::Captures<'_>| {
        refs.get(&caps[1].to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    });
    text.into_owned()
}

/// Render the outline as Markdown: top-level blocks as paragraphs, nested
/// blocks as a list one level shallower.
fn render(outline: &Outline) -> String {
    let refs: HashMap<String, String> = outline
        .blocks
        .iter()
        .filter_map(|block| {
            let id = block
                .properties
                .iter()
                .find(|(key, _)| key == "id")?
                .1
                .to_ascii_lowercase();
            Some((id, block.first.trim().to_string()))
        })
        .collect();

    let mut out = String::new();
    let mut previous_level: Option<usize> = None;
    for block in &outline.blocks {
        let text = clean_inline(&block_text(block), &refs);
        if text.trim().is_empty() {
            continue;
        }
        let top = block.level == 0;
        let was_top = previous_level.is_none_or(|level| level == 0);
        if !out.is_empty() && (top || was_top) {
            out.push('\n');
        }
        let mut lines = text.lines();
        let first = lines.next().unwrap_or_default();
        if top {
            out.push_str(first);
            out.push('\n');
            for line in lines {
                out.push_str(line);
                out.push('\n');
            }
        } else {
            let pad = "  ".repeat(block.level - 1);
            out.push_str(&format!("{pad}- {first}\n"));
            for line in lines {
                out.push_str(&format!("{pad}  {line}\n"));
            }
        }
        previous_level = Some(block.level);
    }
    out
}

/// A value list such as `tags:: [[a b]], c, #d`.
fn property_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|item| {
            item.trim()
                .trim_start_matches('#')
                .trim_start_matches("[[")
                .trim_end_matches("]]")
                .trim()
                .to_string()
        })
        .filter(|item| !item.is_empty())
        .collect()
}

/// Whether every line only links elsewhere, like the sidebar `Contents`
/// page. Logseq pages link far more than Obsidian notes, so the Obsidian
/// test (half the lines hold a link) would drop ordinary journals.
fn is_link_index(markdown: &str) -> bool {
    let mut any_link = false;
    for line in markdown.lines().filter(|line| !line.trim().is_empty()) {
        any_link |= LINK_RE.is_match(line) || TAG_RE.is_match(line);
        let without = LINK_RE.replace_all(line, "");
        let without = TAG_RE.replace_all(&without, "");
        if without.chars().any(char::is_alphanumeric) {
            return false;
        }
    }
    any_link
}

// ---------------------------------------------------------------------------
// Page -> RawDocument
// ---------------------------------------------------------------------------

/// Convert one Logseq page file into chunk documents with ids
/// `<source_id>::<path>::<n>`. Returns nothing for an empty page, a page
/// that only links to others, or one of Wenlan's own projected pages.
pub fn page_to_documents(
    source_id: &str,
    path: &Path,
    content: &str,
    mtime: i64,
) -> Vec<RawDocument> {
    let (frontmatter, body) = extract_frontmatter(content);
    if frontmatter.has(PROJECTED_PAGE_FRONTMATTER_KEY) || has_projection_marker_line(content) {
        log::info!(
            "[ingest] skipping {} — carries `{PROJECTED_PAGE_FRONTMATTER_KEY}` frontmatter, \
             so it is one of Wenlan's own projected pages, not a source document",
            path.display()
        );
        return Vec::new();
    }

    let outline = parse_outline(body);
    let markdown = render(&outline);
    if markdown.trim().is_empty() || is_link_index(&markdown) {
        return Vec::new();
    }

    let page_property = |key: &str| {
        outline
            .properties
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    let mut tags = frontmatter.tags();
    tags.extend(page_property("tags").map(property_list).unwrap_or_default());
    let mut aliases = frontmatter.aliases();
    aliases.extend(
        page_property("alias")
            .map(property_list)
            .unwrap_or_default(),
    );
    let memory_type = frontmatter
        .get_str("type")
        .or_else(|| page_property("type"))
        .and_then(|t| t.parse::<MemoryType>().ok())
        .map(|mt| mt.to_string());
    let title = page_property("title")
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| title_from_path(path));

    let mut metadata = HashMap::new();
    if !tags.is_empty() {
        metadata.insert("tags".to_string(), tags.join(","));
    }
    if !aliases.is_empty() {
        metadata.insert("aliases".to_string(), aliases.join(","));
    }
    metadata.insert("extension".to_string(), "md".to_string());

    note_chunks_to_documents(NoteChunks {
        source_id,
        path,
        title,
        body: &markdown,
        metadata,
        // Like an Obsidian note: the first tag names the Space.
        space: tags.first().cloned(),
        memory_type,
        mtime,
        source_agent: LOGSEQ_SOURCE_AGENT,
    })
}

#[cfg(test)]
#[path = "logseq_tests.rs"]
mod tests;
