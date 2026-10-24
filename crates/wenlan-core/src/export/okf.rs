// SPDX-License-Identifier: Apache-2.0
//! Pure-OKF v0.2 export bundle: `wenlan export okf <dir>`.
//!
//! The md projection under `~/.wenlan/pages` already conforms to OKF, but it
//! keeps `[[wikilinks]]`, the `<!-- origin:sources:* -->` delimiters and
//! `_sources/` stubs because Obsidian and the page watcher depend on them.
//! This module writes the pure-OKF twin into a caller-named directory: the
//! same frontmatter (minus `related:`, whose edges the converted body links
//! already carry), standard markdown links, and a fixed
//! `index.md` / `pages/` / `sources/` layout with no Obsidian-only syntax.

use crate::error::WenlanError;
use crate::export::ExportStats;
use crate::pages::Page;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;
use wenlan_types::CodeBasisLocator;

pub const OKF_VERSION: &str = "0.2";
pub(crate) const MARKER_FILE: &str = ".wenlan-okf-export.json";
const INDEX_FILE: &str = "index.md";
const PAGES_DIR: &str = "pages";
const SOURCES_DIR: &str = "sources";

/// Where a bundle file somebody edited is moved before the export replaces
/// it. Dot-prefixed on purpose: `sources::directory::scan_directory` skips
/// hidden entries, so the preserved copy is invisible to Wenlan's own
/// importer and to any OKF consumer walking the bundle, while still sitting
/// where a person can find it.
const ARCHIVE_DIR: &str = ".wenlan-archive";

/// Root marker proving a directory holds a Wenlan OKF export (and listing
/// exactly the bundle files the last export wrote, so a re-export can remove
/// stale ones without touching user files beside them).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ExportMarker {
    okf_version: String,
    generated_by: String,
    files: Vec<String>,
    /// SHA-256 of the bytes this exporter last wrote at each listed path.
    ///
    /// `files` alone only ever proved AUTHORSHIP, and a re-export used it to
    /// decide what it could overwrite and unlink. Authorship is not the same
    /// question: somebody can edit an exported page in place, and the marker
    /// keeps saying Wenlan wrote it. The digest answers the question that
    /// actually matters -- are these still the bytes we wrote?
    ///
    /// Absent in a marker written before 0.18.11, hence `serde(default)`: an
    /// old bundle parses, and its first re-export records digests for every
    /// run after that.
    #[serde(default)]
    digests: BTreeMap<String, String>,
}

fn generated_by() -> String {
    format!("wenlan/{}", crate::version())
}

fn digest_of(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

fn marker_for(files: &HashSet<String>, digests: BTreeMap<String, String>) -> ExportMarker {
    let mut files: Vec<String> = files.iter().cloned().collect();
    files.sort();
    ExportMarker {
        okf_version: OKF_VERSION.to_string(),
        generated_by: generated_by(),
        files,
        digests,
    }
}

/// What a re-export may do with one marker-listed path.
#[derive(Debug, PartialEq, Eq)]
enum BundleFileState {
    /// Nothing there, still byte-identical to what the last export wrote,
    /// or -- with no digest on record -- byte-identical to what this run is
    /// about to write anyway. Ours to replace or remove.
    Ours,
    /// Present, and the bytes have changed since. Somebody's edit: preserved,
    /// never overwritten in place and never unlinked.
    Changed,
    /// Present, but not a plain file: a directory or a symlink standing where
    /// a bundle file was recorded. Left exactly as it is. Moving it aside
    /// would clear the obstruction and let the write land, and unlinking a
    /// symlink destroys a link somebody made on purpose. The ordinary write
    /// refuses the path and reports it as a failed page.
    Foreign,
}

/// Decide what a re-export may do with one marker-listed path.
///
/// `planned` is what this run is about to write at that path, when it plans to
/// write there at all. It settles the case the digests cannot: a bundle from a
/// Wenlan older than 0.18.11 recorded no digests, so the only evidence left is
/// whether the bytes on disk are already the bytes we would write. Identical
/// means nobody touched it and the rewrite is a no-op. Anything else is
/// treated as an edit and preserved, because with no digest there is no way to
/// tell an edit from an untouched file, and preserving costs a stray copy
/// while overwriting costs somebody's writing.
fn bundle_file_state(
    target: &Path,
    entry: &str,
    digests: &BTreeMap<String, String>,
    planned: Option<&[u8]>,
) -> BundleFileState {
    let path = target.join(entry);
    let Ok(metadata) = std::fs::symlink_metadata(&path) else {
        return BundleFileState::Ours;
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return BundleFileState::Foreign;
    }
    // A file this pass cannot read is a file it cannot claim.
    let Ok(bytes) = std::fs::read(&path) else {
        return BundleFileState::Changed;
    };
    match digests.get(entry) {
        Some(recorded) if &digest_of(&bytes) == recorded => BundleFileState::Ours,
        Some(_) => BundleFileState::Changed,
        None if planned == Some(bytes.as_slice()) => BundleFileState::Ours,
        None => BundleFileState::Changed,
    }
}

/// Move an edited bundle file into `.wenlan-archive/`, keeping its position in
/// the bundle (`pages/alpha.md` becomes `.wenlan-archive/pages/alpha.md`) so
/// two files of the same name from different directories cannot collide.
/// Suffixes past an occupied destination rather than replacing it: an older
/// preserved copy is the one thing here the database cannot reproduce.
fn preserve_edited_bundle_file(target: &Path, entry: &str) -> Result<(), WenlanError> {
    let source = target.join(entry);
    let destination_dir = match Path::new(entry).parent() {
        Some(parent) if !parent.as_os_str().is_empty() => target.join(ARCHIVE_DIR).join(parent),
        _ => target.join(ARCHIVE_DIR),
    };
    std::fs::create_dir_all(&destination_dir).map_err(WenlanError::Io)?;
    let name = Path::new(entry)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(entry);
    let (stem, extension) = match name.strip_suffix(".md") {
        Some(stem) => (stem, ".md"),
        None => (name, ""),
    };
    let mut destination = destination_dir.join(name);
    let mut attempt = 1;
    while destination.symlink_metadata().is_ok() {
        attempt += 1;
        if attempt > 100 {
            return Err(WenlanError::Conflict(format!(
                "cannot preserve {entry}: {ARCHIVE_DIR} has no free name for it"
            )));
        }
        destination = destination_dir.join(format!("{stem}-{attempt}{extension}"));
    }
    std::fs::rename(&source, &destination).map_err(WenlanError::Io)?;
    log::warn!(
        "[okf] {entry} was edited since the last export; preserved at {}",
        destination.display()
    );
    Ok(())
}

// Dedicated page-id links: `[Title](concept_<id>)` (the shape
// `obsidian::convert_links_to_wikilinks` reads) and `[Title](page_<id>)`
// (current page ids are minted as `page_<uuid>`), plus a capture around the
// id so the replacer need not re-parse the match.
static ID_LINK_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\[([^\]]+)\]\(((?:concept|page)_[a-zA-Z0-9\-]+)\)").unwrap()
});

// Any other `[text](target)` markdown link (URLs, relative paths, ...).
static MD_LINK_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\[([^\]]+)\]\(([^)\s]+)\)").unwrap());

// Mirrors `sources::obsidian::WIKILINK_RE` (kept local: that static is
// private, and this module only needs the match shape, not the extractor).
static WIKILINK_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(!?)\[\[([^\]|#]+)(?:#([^\]|]+))?(?:\|([^\]]+))?\]\]").unwrap()
});

/// True when `entry` names a file this exporter owns: `index.md`,
/// `pages/<name>.md` or `sources/<name>.md` where `<name>` carries no `/`,
/// `\` or `..` and has no leading dot. Anything else in a marker (a forged
/// `../x.md`, an absolute path, a future key) is ignored, never deleted.
pub(crate) fn marker_entry_is_safe(entry: &str) -> bool {
    if entry == INDEX_FILE {
        return true;
    }
    for dir in [PAGES_DIR, SOURCES_DIR] {
        let prefix = format!("{dir}/");
        if let Some(name) = entry.strip_prefix(&prefix) {
            if !name.ends_with(".md") {
                return false;
            }
            let stem = &name[..name.len() - ".md".len()];
            if stem.is_empty() || stem.starts_with('.') {
                return false;
            }
            if stem.contains('/') || stem.contains('\\') || stem.contains("..") {
                return false;
            }
            return true;
        }
    }
    false
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Write `bytes` to `path`, refusing anything that is not already a plain
/// file.
///
/// This used to unlink a symlink standing in the way, so the write could not
/// clobber whatever the link pointed at. That protected the target and
/// destroyed the link, which is somebody's file too: a person who points
/// `pages/alpha.md` at a note they keep elsewhere gets it silently removed on
/// the next export. Refusing protects both. The caller reports the page as
/// failed, exactly as it does for a directory in the way.
fn write_file_nofollow(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "{} is not a plain file; refusing to replace it",
                    path.display()
                ),
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::write(path, bytes)
}

/// Replace the root marker atomically: a temp file in the same directory,
/// fsync, rename over the marker, then fsync the directory.
///
/// `write_file_nofollow` truncates and rewrites in place, so a kill partway
/// through left malformed JSON and the next export refused at the marker
/// parse, stuck until somebody hand-edited the file and lost its ownership and
/// digest records. A rename within one directory leaves the previous marker or
/// the new one, never a torn file. The projection's atomic writer is reused for
/// that, so the symlink guarantee comes with it: the temp is created
/// `create_new` + nofollow, and a marker that is a symlink or a directory is
/// refused rather than replaced.
fn write_marker_atomically(root: &Path, bytes: &[u8]) -> Result<(), WenlanError> {
    write_marker_atomically_with_hook(root, bytes, || Ok(()))
}

fn write_marker_atomically_with_hook(
    root: &Path,
    bytes: &[u8],
    after_temp_write: impl FnOnce() -> std::io::Result<()>,
) -> Result<(), WenlanError> {
    let directory = cap_std::fs::Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
    // Unique per write: a temp left by a killed run (or by a recycled pid) can
    // never collide with this one under `create_new`.
    let temporary = format!("{MARKER_FILE}.{}.tmp", uuid::Uuid::new_v4().simple());
    crate::export::knowledge::write_page_atomically_nofollow_with_hook(
        &directory,
        MARKER_FILE,
        &temporary,
        bytes,
        after_temp_write,
    )?;
    crate::export::knowledge::sync_dir_capability(&directory)
}

/// Remove marker temps that a killed run left behind. Best effort: a temp is
/// only ever a copy of a marker that was never installed, so keeping one costs
/// nothing but clutter. Plain files only; anything else by that name is not
/// ours to touch.
fn remove_stale_marker_temps(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let prefix = format!("{MARKER_FILE}.");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(&prefix) || !name.ends_with(".tmp") {
            continue;
        }
        let path = entry.path();
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Plan stable bundle filenames: pages sorted by `(created_at, id)`, slug
/// via `obsidian::slugify`, empty slug falling back to
/// `sanitize_stub_id(page.id)`, collisions suffixed `-2`, `-3`, ...
/// `index.md` and `log.md` are never a page's file: OKF reserves both at
/// every level of the hierarchy (spec 3.1), so a page titled "Index" is
/// `index-page.md` and one titled "Log" is `log-page.md`.
/// Returns `(page_id, file)` pairs in export order.
pub(crate) fn plan_page_filenames(pages: &[Page]) -> Vec<(String, String)> {
    let mut order: Vec<usize> = (0..pages.len()).collect();
    order.sort_by(|&a, &b| {
        pages[a]
            .created_at
            .cmp(&pages[b].created_at)
            .then_with(|| pages[a].id.cmp(&pages[b].id))
    });
    let mut used: HashSet<String> = HashSet::new();
    let mut planned = Vec::with_capacity(pages.len());
    for i in order {
        let page = &pages[i];
        let mut base = crate::export::obsidian::slugify(&page.title);
        if base.is_empty() {
            base = crate::export::provenance::sanitize_stub_id(&page.id);
        }
        let base = crate::export::knowledge::page_stem_clear_of_reserved(base);
        let mut candidate = format!("{base}.md");
        let mut n = 2;
        while used.contains(&candidate) {
            candidate = format!("{base}-{n}.md");
            n += 1;
        }
        used.insert(candidate.clone());
        planned.push((page.id.clone(), candidate));
    }
    planned
}

/// `(space, folded title)` -> owning page id, or `None` when ambiguous.
pub(crate) type TitleOwners = HashMap<(Option<String>, String), Option<String>>;

/// Fold every exported page title through `MemoryDB::page_title_key` to its
/// owning page id, resolved per Space like
/// `MemoryDB::folded_title_owners_scoped` (`None` = uncategorized): the key
/// is `(space, folded title)`, and the value is `None` when two exported
/// pages in the same Space fold to the same title (the ambiguous-title
/// contract, scoped: such targets stay unresolved).
pub(crate) fn title_owner_map(pages: &[Page]) -> TitleOwners {
    let mut map: TitleOwners = HashMap::new();
    for page in pages {
        let key = (
            page.space.clone(),
            crate::db::MemoryDB::page_title_key(&page.title),
        );
        match map.get_mut(&key) {
            None => {
                map.insert(key, Some(page.id.clone()));
            }
            Some(slot) => {
                *slot = None;
            }
        }
    }
    map
}

/// Byte ranges of inline code spans outside the fenced ranges, using
/// CommonMark backtick runs: a run of N backticks opens a span that ends at
/// the next run of exactly N backticks in the same paragraph (a span never
/// crosses a blank line or a fenced code block). A run with no matching
/// closer is literal text and suppresses nothing after it.
pub(crate) fn inline_code_ranges(content: &str, fenced: &[Range<usize>]) -> Vec<Range<usize>> {
    // Byte offsets where a paragraph ends: each blank (whitespace-only) line
    // and each fenced block. A run's paragraph is the number of breaks
    // before it.
    let mut block_breaks: Vec<usize> = fenced.iter().map(|r| r.start).collect();
    let mut line_start = 0;
    for line in content.split_inclusive('\n') {
        if line.trim().is_empty() {
            block_breaks.push(line_start);
        }
        line_start += line.len();
    }
    block_breaks.sort_unstable();
    let paragraph = |offset: usize| block_breaks.partition_point(|&b| b < offset);
    let bytes = content.as_bytes();
    let mut runs: Vec<(usize, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i] == b'`' {
            i += 1;
        }
        if !fenced.iter().any(|r| r.contains(&start)) {
            runs.push((start, i - start, paragraph(start)));
        }
    }
    let mut ranges = Vec::new();
    let mut k = 0;
    while k < runs.len() {
        let (start, len, para) = runs[k];
        match runs[k + 1..]
            .iter()
            .position(|&(_, l, p)| l == len && p == para)
        {
            Some(offset) => {
                let (close_start, close_len, _) = runs[k + 1 + offset];
                ranges.push(start..close_start + close_len);
                k += 2 + offset;
            }
            // Unmatched: literal text, keep scanning after the run.
            None => k += 1,
        }
    }
    ranges
}

/// Replace regex matches outside code, splicing `replace` over each accepted
/// match. Fenced code-block ranges come from
/// `sources::obsidian::code_block_ranges`; inline code spans are scanned on
/// top (see [`inline_code_ranges`]). Both are recomputed on the current text
/// per pass (a previous pass shifts byte offsets).
fn replace_outside_code(
    content: &str,
    re: &regex::Regex,
    mut replace: impl FnMut(&regex::Captures<'_>) -> String,
) -> String {
    let fenced = crate::sources::obsidian::code_block_ranges(content);
    let inline = inline_code_ranges(content, &fenced);
    let in_code = |offset: usize| {
        fenced.iter().any(|r| r.contains(&offset)) || inline.iter().any(|r| r.contains(&offset))
    };
    let mut out = String::with_capacity(content.len());
    let mut last = 0;
    for cap in re.captures_iter(content) {
        let m = cap.get(0).expect("capture group 0 always matches");
        if in_code(m.start()) {
            continue;
        }
        out.push_str(&content[last..m.start()]);
        out.push_str(&replace(&cap));
        last = m.end();
    }
    out.push_str(&content[last..]);
    out
}

/// What the exporter knows about the concepts Wenlan imported, so a link an
/// imported concept still spells in its ORIGINAL bundle's terms can be
/// repaired to wherever the target is exported now. Built from
/// `okf_concepts`; see [`concept_path_map`].
///
/// Two lookups, because two questions are asked:
///
/// - **From an imported page** (`/pages/b.md`, `./b.md`, `../x/b.md`): the
///   link was written inside one bundle, so it is resolved against that
///   bundle's concept ids, from the linking concept's own folder, exactly as
///   the importer resolved it. Another bundle holding the same path cannot
///   capture it.
/// - **From any other page** (`/concepts/b.md`): no bundle is known, so the
///   absolute path is looked up across every imported bundle, and a path two
///   bundles share resolves to nothing.
#[derive(Debug, Clone, Default)]
pub struct ConceptPathMap {
    /// `/<concept id>.md` -> page id, across all bundles; ambiguous paths absent.
    by_path: HashMap<String, String>,
    /// Imported page id -> `(source id, concept id)`.
    origins: HashMap<String, (String, String)>,
    /// Source id -> lowercased concept id -> page id (`None` when two concept
    /// ids of the bundle differ only by case, which the importer's link graph
    /// folds together).
    by_source: HashMap<String, HashMap<String, Option<String>>>,
}

/// What an imported page's link resolves to.
enum ImportedLink<'a> {
    /// The linking page is not an imported concept; its bundle is unknown.
    NotImported,
    /// Imported linker, but the target is no concept of its bundle.
    Unidentified,
    /// The imported concept the link points at, as a page id.
    Page(&'a str),
}

impl ConceptPathMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// The page an absolute bundle path (`/concepts/b.md`) was imported as,
    /// across all bundles.
    pub fn get(&self, path: &str) -> Option<&String> {
        self.by_path.get(path)
    }

    /// Resolve `target`, a link written in the body of page `linker`, to the
    /// imported concept it names, using the importer's own normalization with
    /// the folder of the linker's original path.
    fn resolve_from_imported(&self, linker: &str, target: &str) -> ImportedLink<'_> {
        let Some((source_id, concept_id)) = self.origins.get(linker) else {
            return ImportedLink::NotImported;
        };
        let from_dir = concept_id.rsplit_once('/').map_or("", |(dir, _)| dir);
        let found = crate::sources::okf::normalize_link_target(from_dir, target)
            .and_then(|id| self.by_source.get(source_id)?.get(&id.to_lowercase()))
            .and_then(|page| page.as_deref());
        match found {
            Some(page) => ImportedLink::Page(page),
            None => ImportedLink::Unidentified,
        }
    }
}

/// Build the lookup the exporter uses for links an imported concept still
/// spells in its original bundle's terms.
///
/// `rows` are `(page id, source id, concept id)` from `okf_concepts`. The
/// global key is the absolute bundle path the concept had,
/// `/<concept id>.md`. Two bundles can both hold `concepts/b.md`, and a page
/// that was not itself imported carries no source, so an ambiguous path
/// resolves to nothing rather than to a guess: the link is then handled as
/// unresolvable, which loses a relationship but never points a reader at the
/// wrong page.
pub fn concept_path_map(rows: &[(String, String, String)]) -> ConceptPathMap {
    let mut seen: HashMap<String, Option<String>> = HashMap::new();
    let mut map = ConceptPathMap::default();
    for (page_id, source_id, concept_id) in rows {
        let key = format!("/{}.md", concept_id.trim_start_matches('/'));
        match seen.get(&key) {
            Some(Some(existing)) if existing == page_id => {}
            Some(_) => {
                seen.insert(key, None);
            }
            None => {
                seen.insert(key, Some(page_id.clone()));
            }
        }
        map.origins
            .insert(page_id.clone(), (source_id.clone(), concept_id.clone()));
        let in_source = map.by_source.entry(source_id.clone()).or_default();
        let folded = concept_id.trim_start_matches('/').to_lowercase();
        match in_source.get(&folded) {
            Some(Some(existing)) if existing == page_id => {}
            Some(_) => {
                in_source.insert(folded, None);
            }
            None => {
                in_source.insert(folded, Some(page_id.clone()));
            }
        }
    }
    map.by_path = seen
        .into_iter()
        .filter_map(|(key, page)| page.map(|page| (key, page)))
        .collect();
    map
}

/// Split a link target into its path and the `#fragment` / `?query` tail
/// (delimiter included), after one pair of surrounding `<` `>`. This is the
/// cut the importer makes before resolving the path.
fn split_link_suffix(target: &str) -> (&str, &str) {
    let inner = target
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'))
        .unwrap_or(target);
    match inner.find(['#', '?']) {
        Some(at) => inner.split_at(at),
        None => (inner, ""),
    }
}

/// What a markdown link to something other than an exported page id becomes
/// in the exported bundle. `linker` is the page whose body holds the link.
///
/// The exporter writes exactly `index.md`, `pages/*.md` and `sources/*.md`, but
/// a link in an imported concept names a path of its ORIGINAL bundle, which
/// can be the same spelling as one of those and mean a different file. So when
/// the linking page was imported, its link is resolved by identity first
/// (see [`ConceptPathMap`]), for absolute and relative spellings alike:
///
/// - identified, and the target is being exported: rewritten to that page,
///   fragment kept, which is the same relationship at its new address;
/// - identified, but its page is not being exported: collapsed to its link
///   text, the same way an unknown page id is handled. A bundle that promises
///   a document it does not carry is worse than prose that lost a link, and a
///   path left behind could be taken over by another page;
/// - not identified: left exactly as written (OKF 6.1 tolerates a broken
///   link), except that an absolute `.md` path outside `index.md`, `pages/`
///   and `sources/` names a document this bundle cannot contain and collapses
///   like an unidentified `/concepts/` link always did.
///
/// A page that was not imported has no bundle to resolve against. Its
/// absolute `/concepts/...` paths are looked up across all imported bundles
/// (a path two bundles share resolves to nothing), and everything else,
/// including its `/pages/` and `/sources/` links, passes through untouched.
/// A non-`.md` target (an image, an asset) is always left alone.
fn convert_markdown_link(
    text: &str,
    target: &str,
    linker: &str,
    id_to_file: &HashMap<String, String>,
    concept_paths: &ConceptPathMap,
) -> String {
    use crate::export::knowledge::escape_index_link_text;
    let unchanged = || format!("[{text}]({target})");
    let (path, suffix) = split_link_suffix(target);
    if path == "/index.md" || path.starts_with("//") || !path.to_ascii_lowercase().ends_with(".md")
    {
        return unchanged();
    }
    let page = match concept_paths.resolve_from_imported(linker, target) {
        ImportedLink::Page(page) => Some(page),
        ImportedLink::Unidentified | ImportedLink::NotImported
            if path.starts_with("/pages/") || path.starts_with("/sources/") =>
        {
            return unchanged();
        }
        ImportedLink::Unidentified => None,
        ImportedLink::NotImported => concept_paths.get(path).map(String::as_str),
    };
    let absolute = path.starts_with('/');
    match page {
        Some(page) => match id_to_file.get(page) {
            Some(file) => {
                let label = escape_index_link_text(text);
                format!("[{label}](/pages/{file}{suffix})")
            }
            None => text.to_string(),
        },
        // A relative link nobody can place stays; so does an absolute one
        // that only an unidentified imported page could have meant.
        None if !absolute => unchanged(),
        None => text.to_string(),
    }
}

/// Convert one page body to bundle links. `page_id` and `page_space` are the
/// linking page's id and Space: `[[Target]]` resolves only against exported
/// pages in the same Space (see [`title_owner_map`]), and an imported page's
/// own links resolve against its original bundle (see [`ConceptPathMap`]).
/// `id_to_file` maps exported page ids to bundle files, and `source_ids` is
/// this page's cited memory ids.
///
/// - `[Text](concept_<id>)` / `[Text](page_<id>)`: exported page ->
///   `[Text](/pages/<file>)`, unknown id -> plain `Text`. Other
///   `[t](target)` links are already standard markdown: kept when `target`
///   is not an exported page id, rewritten when it is, and repaired to the
///   target's exported file when they name an imported concept (see
///   [`convert_markdown_link`]).
/// - `[[Target]]`, `[[Target|Display]]`, `[[Target#Heading]]`, `![[Target]]`:
///   resolved titles -> `[Display or Target](/pages/<file>)` with
///   `#<slugify(heading)>` when a heading is present; unresolved -> plain
///   `Display or Target`. Embeds become ordinary links.
/// - `[[<id>]]` where `<id>` is one of this page's `source_memory_ids` ->
///   `[<id>](/sources/<stub>)`.
/// - Every emitted `[label](...)` passes `label` through
///   `escape_index_link_text` so a bracket in page content cannot break the
///   link; plain-text fallbacks stay unescaped.
/// - Anything inside fenced code blocks or inline code spans is untouched.
pub(crate) fn convert_body(
    content: &str,
    page_id: &str,
    page_space: Option<&str>,
    source_ids: &[String],
    id_to_file: &HashMap<String, String>,
    title_owner: &TitleOwners,
    concept_paths: &ConceptPathMap,
) -> String {
    use crate::export::knowledge::escape_index_link_text;
    let cited: HashSet<&str> = source_ids.iter().map(String::as_str).collect();
    // Pass 1: markdown links, read from the page as written. Runs before the
    // page-id shape below because that shape emits `/pages/<file>` links of
    // this bundle, which a second look would take for an imported concept's
    // original path.
    let pass = replace_outside_code(content, &MD_LINK_RE, |cap| {
        let (text, target) = (&cap[1], &cap[2]);
        match id_to_file.get(target) {
            Some(file) => {
                let label = escape_index_link_text(text);
                format!("[{label}](/pages/{file})")
            }
            None => convert_markdown_link(text, target, page_id, id_to_file, concept_paths),
        }
    });
    // Page-id links left over are not exported pages: collapse to plain text.
    let pass = replace_outside_code(&pass, &ID_LINK_RE, |cap| {
        let (text, id) = (&cap[1], &cap[2]);
        match id_to_file.get(id) {
            Some(file) => {
                let label = escape_index_link_text(text);
                format!("[{label}](/pages/{file})")
            }
            None => text.to_string(),
        }
    });
    // Pass 2: wikilinks (code ranges recomputed on the rewritten text).
    replace_outside_code(&pass, &WIKILINK_RE, |cap| {
        let target = &cap[2];
        let heading = cap.get(3).map(|m| m.as_str());
        let display = cap.get(4).map(|m| m.as_str());
        let is_source_cite =
            heading.is_none() && display.is_none() && cap[1].is_empty() && cited.contains(target);
        if is_source_cite {
            let stub = crate::export::provenance::stub_filename(target);
            let label = escape_index_link_text(target);
            return format!("[{label}](/sources/{stub})");
        }
        let key = (
            page_space.map(|space| space.to_owned()),
            crate::db::MemoryDB::page_title_key(target),
        );
        let owner = title_owner.get(&key).and_then(|o| o.as_ref());
        match owner.and_then(|id| id_to_file.get(id)) {
            Some(file) => {
                let label = escape_index_link_text(display.unwrap_or(target));
                match heading {
                    Some(h) => {
                        let frag = crate::export::obsidian::slugify(h);
                        format!("[{label}](/pages/{file}#{frag})")
                    }
                    None => format!("[{label}](/pages/{file})"),
                }
            }
            None => display.unwrap_or(target).to_string(),
        }
    })
}

/// One bundle page file: projection frontmatter minus `related:`, plus the
/// converted body and a delimiter-free `## Sources` section (omitted when
/// the page cites nothing).
pub(crate) fn render_page_file(
    page: &Page,
    id_to_file: &HashMap<String, String>,
    title_owner: &TitleOwners,
    concept_paths: &ConceptPathMap,
    code_bases: &[CodeBasisLocator],
) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&crate::export::knowledge::page_frontmatter(
        page, false, code_bases,
    ));
    out.push_str("---\n\n");
    out.push_str(&convert_body(
        &page.content,
        &page.id,
        page.space.as_deref(),
        &page.source_memory_ids,
        id_to_file,
        title_owner,
        concept_paths,
    ));
    if page.source_memory_ids.is_empty() {
        out.push('\n');
    } else {
        out.push_str("\n\n## Sources\n");
        for id in &page.source_memory_ids {
            let stub = crate::export::provenance::stub_filename(id);
            let label = crate::export::knowledge::escape_index_link_text(id);
            out.push_str(&format!("- [{label}](/sources/{stub})\n"));
        }
    }
    out
}

/// One bundle source stub: `type: source` frontmatter pointing at the memory,
/// plus a one-sentence body. No memory content, no `origin_stub:` marker.
pub(crate) fn render_source_stub(memory_id: &str) -> String {
    let title = crate::export::provenance::yaml_quoted(memory_id);
    let resource = crate::export::provenance::yaml_quoted(&format!("wenlan://memory/{memory_id}"));
    format!(
        "---\ntype: source\ntitle: {title}\nresource: {resource}\n---\n\n\
         This is a read-only reference to Wenlan memory `{memory_id}`.\n"
    )
}

/// Bundle `index.md`: the same renderer as the projection index, with bundle
/// links (`/pages/<file>`). `files` maps page ids to bundle filenames.
pub(crate) fn render_bundle_index(pages: &[Page], files: &HashMap<String, String>) -> String {
    let rows: Vec<(String, String, String, Option<String>)> = pages
        .iter()
        .filter_map(|page| {
            files.get(&page.id).map(|file| {
                (
                    page.title.clone(),
                    file.clone(),
                    page.summary.clone().unwrap_or_default(),
                    page.space.clone(),
                )
            })
        })
        .collect();
    crate::export::knowledge::render_index_for_entries(rows, "/pages/")
}

/// Read the previous marker. `Ok(None)` means no marker (fresh directory
/// rules apply); an unreadable or unparseable marker is a
/// [`WenlanError::Conflict`] — never guess which files are ours.
fn read_old_marker(root: &Path) -> Result<Option<ExportMarker>, WenlanError> {
    let path = root.join(MARKER_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(WenlanError::Io(e)),
    };
    let marker: ExportMarker = serde_json::from_slice(&bytes).map_err(|_| {
        WenlanError::Conflict(format!(
            "{} exists but is not a Wenlan OKF export marker; refusing to touch {}",
            MARKER_FILE,
            root.display()
        ))
    })?;
    if marker.okf_version != OKF_VERSION {
        return Err(WenlanError::Conflict(format!(
            "{} declares okf_version {:?}; refusing to touch {}",
            MARKER_FILE,
            marker.okf_version,
            root.display()
        )));
    }
    if !marker.generated_by.starts_with("wenlan/") {
        return Err(WenlanError::Conflict(format!(
            "{} was not written by Wenlan (generated_by {:?}); refusing to touch {}",
            MARKER_FILE,
            marker.generated_by,
            root.display()
        )));
    }
    Ok(Some(marker))
}

/// Write the OKF bundle for `pages` into `target`.
///
/// The directory must not exist (it is created), be empty, or hold a
/// previous Wenlan OKF export (root marker naming Wenlan as its writer);
/// anything else is a [`WenlanError::Conflict`]. The root, `pages/` and
/// `sources/` must not be symlinks. A planned path that already exists
/// without being marker-listed is a Conflict — a foreign file is never
/// overwritten. Re-export deletes only marker-listed bundle files that are
/// no longer written (validated names only); user files beside them (`.git/`,
/// README, notes) are never touched.
///
/// A marker-listed file whose bytes no longer match the digest the last export
/// recorded has been edited since, and is moved to `.wenlan-archive/` before
/// anything writes over it or removes it. Being listed proves Wenlan wrote the
/// file, not that the file is still Wenlan's. A bundle written before 0.18.11
/// carries no digests, so its first re-export cannot make that distinction and
/// behaves as it did before; every run after that can.
///
/// `index.md` links only pages actually written. The marker is first rewritten
/// with the union of old and planned files, then — after a successful write —
/// with only the new set and its digests.
pub fn export_okf(pages: &[Page], target: &Path) -> Result<ExportStats, WenlanError> {
    export_okf_with_concept_paths(pages, target, &ConceptPathMap::new(), &HashMap::new())
}

/// [`export_okf`], plus the map that resolves links an imported concept still
/// spells in its original bundle's terms. Callers with database access build
/// it with [`concept_path_map`] from `MemoryDB::okf_concept_paths`; an empty
/// map simply leaves every such link unresolvable. `code_bases` holds each
/// page's code bases by page id (callers with database access load them with
/// `MemoryDB::code_basis_locators_by_page`); a page absent from it lists none.
pub fn export_okf_with_concept_paths(
    pages: &[Page],
    target: &Path,
    concept_paths: &ConceptPathMap,
    code_bases: &HashMap<String, Vec<CodeBasisLocator>>,
) -> Result<ExportStats, WenlanError> {
    export_okf_inner(pages, target, concept_paths, code_bases, false)
}

/// [`export_okf_with_concept_paths`] for a bundle that lives in a folder of a
/// git repository: links between bundle files are written relative to the
/// linking file (`pages/x.md` from the index, `x.md` between pages), since a
/// repository host resolves `/pages/x.md` from the repository root, not the
/// bundle's.
pub fn export_okf_for_repository(
    pages: &[Page],
    target: &Path,
    concept_paths: &ConceptPathMap,
    code_bases: &HashMap<String, Vec<CodeBasisLocator>>,
) -> Result<ExportStats, WenlanError> {
    export_okf_inner(pages, target, concept_paths, code_bases, true)
}

/// The body of a page file in a repository bundle (`pages/<file>`) as page
/// content again: a link the export made to another page of `pages`
/// (`[label](<file>)`) goes back to the `[[wikilink]]` it came from. A link
/// to a heading or anywhere else stays as written.
pub fn wikilinks_from_repository_body(body: &str, pages: &[Page]) -> String {
    let titles: HashMap<String, &str> = plan_page_filenames(pages)
        .into_iter()
        .filter_map(|(id, file)| {
            pages
                .iter()
                .find(|page| page.id == id)
                .map(|page| (file, page.title.as_str()))
        })
        .collect();
    replace_outside_code(body, &MD_LINK_RE, |cap| {
        let (label, target) = (&cap[1], &cap[2]);
        match titles.get(target) {
            Some(title) => {
                let label = label.replace("\\[", "[").replace("\\]", "]");
                if label == *title {
                    format!("[[{title}]]")
                } else {
                    format!("[[{title}|{label}]]")
                }
            }
            None => cap[0].to_string(),
        }
    })
}

/// Rewrite the bundle-root links in `body` (a file at `path` in the bundle) to
/// links relative to that file.
fn relative_bundle_links(path: &str, body: &str) -> String {
    let up = if path.contains('/') { "../" } else { "" };
    let own_dir = path.rsplit_once('/').map(|(dir, _)| dir);
    let mut out = body.to_owned();
    for dir in [PAGES_DIR, SOURCES_DIR] {
        let replacement = if own_dir == Some(dir) {
            "](".to_owned()
        } else {
            format!("]({up}{dir}/")
        };
        out = out.replace(&format!("](/{dir}/"), &replacement);
    }
    out.replace(&format!("](/{INDEX_FILE}"), &format!("]({up}{INDEX_FILE}"))
}

fn export_okf_inner(
    pages: &[Page],
    target: &Path,
    concept_paths: &ConceptPathMap,
    code_bases: &HashMap<String, Vec<CodeBasisLocator>>,
    relative_links: bool,
) -> Result<ExportStats, WenlanError> {
    let mut stats = ExportStats::default();
    for dir in [target, &target.join(PAGES_DIR), &target.join(SOURCES_DIR)] {
        if is_symlink(dir) {
            return Err(WenlanError::Conflict(format!(
                "refusing OKF export through symlink {}",
                dir.display()
            )));
        }
    }
    if target.is_file() {
        return Err(WenlanError::Conflict(format!(
            "OKF export target {} is a file, not a directory",
            target.display()
        )));
    }
    let old_marker = read_old_marker(target)?;
    if old_marker.is_none() && target.exists() {
        // Fail closed: a directory we cannot inspect is not "empty", and
        // writing into it would risk mixing the bundle with unknown files.
        let mut entries = std::fs::read_dir(target).map_err(WenlanError::Io)?;
        if entries.next().is_some() {
            return Err(WenlanError::Conflict(
                "directory is not empty and is not a Wenlan OKF export".to_string(),
            ));
        }
    }
    let (old_file_list, old_digests) = match old_marker {
        Some(marker) => (marker.files, marker.digests),
        None => (Vec::new(), BTreeMap::new()),
    };
    let old_files: HashSet<String> = old_file_list.into_iter().collect();

    let planned_pairs = plan_page_filenames(pages);
    let id_to_file: HashMap<String, String> = planned_pairs.iter().cloned().collect();
    let title_owner = title_owner_map(pages);
    let by_id: HashMap<&str, &Page> = pages.iter().map(|p| (p.id.as_str(), p)).collect();

    let mut planned: HashSet<String> = HashSet::new();
    planned.insert(INDEX_FILE.to_string());
    for (_, file) in &planned_pairs {
        planned.insert(format!("{PAGES_DIR}/{file}"));
    }
    let mut stub_ids: Vec<&String> = pages
        .iter()
        .flat_map(|p| p.source_memory_ids.iter())
        .collect();
    stub_ids.sort();
    stub_ids.dedup();
    for id in &stub_ids {
        planned.insert(format!(
            "{SOURCES_DIR}/{}",
            crate::export::provenance::stub_filename(id)
        ));
    }

    // Never overwrite a file Wenlan did not write: every planned path must
    // be absent or owned by the previous export. Sorted for a deterministic
    // message. Runs before any write, so a refusal leaves the tree untouched.
    let mut planned_sorted: Vec<&String> = planned.iter().collect();
    planned_sorted.sort();
    for entry in planned_sorted {
        if !old_files.contains(entry) && std::fs::symlink_metadata(target.join(entry)).is_ok() {
            return Err(WenlanError::Conflict(format!(
                "refusing to overwrite {entry}: not written by a previous Wenlan OKF export"
            )));
        }
    }

    std::fs::create_dir_all(target.join(PAGES_DIR))?;
    std::fs::create_dir_all(target.join(SOURCES_DIR))?;

    // Render every planned file before touching the tree, so the preservation
    // pass below can compare what is on disk against what this run would put
    // there. The index is rendered as if every page succeeds; a page that
    // later fails to write only makes the comparison miss, which preserves.
    let mut planned_bytes: HashMap<String, Vec<u8>> = HashMap::new();
    for (id, file) in &planned_pairs {
        if let Some(page) = by_id.get(id.as_str()) {
            let page_code_bases = code_bases.get(id).map_or(&[][..], Vec::as_slice);
            let body = render_page_file(
                page,
                &id_to_file,
                &title_owner,
                concept_paths,
                page_code_bases,
            );
            planned_bytes.insert(format!("{PAGES_DIR}/{file}"), body.into_bytes());
        }
    }
    for id in &stub_ids {
        let name = crate::export::provenance::stub_filename(id);
        planned_bytes.insert(
            format!("{SOURCES_DIR}/{name}"),
            render_source_stub(id).into_bytes(),
        );
    }
    planned_bytes.insert(
        INDEX_FILE.to_string(),
        render_bundle_index(pages, &id_to_file).into_bytes(),
    );
    if relative_links {
        for (path, bytes) in planned_bytes.iter_mut() {
            if let Ok(body) = std::str::from_utf8(bytes) {
                *bytes = relative_bundle_links(path, body).into_bytes();
            }
        }
    }

    // Preservation pass, before anything is written or removed. Being listed
    // in the last marker says Wenlan WROTE a file; it never says the bytes are
    // still Wenlan's. A listed file whose digest no longer matches has been
    // edited since, so it moves to `.wenlan-archive/` rather than being
    // overwritten by the planned write below or unlinked by the stale sweep.
    // Both paths then find nothing where the file was, which is exactly the
    // state they already handle.
    let mut listed: Vec<&String> = old_files.iter().collect();
    listed.sort();
    for entry in listed {
        if !marker_entry_is_safe(entry) {
            continue;
        }
        let planned_here = planned_bytes.get(entry.as_str()).map(Vec::as_slice);
        if bundle_file_state(target, entry, &old_digests, planned_here) == BundleFileState::Changed
        {
            preserve_edited_bundle_file(target, entry)?;
        }
    }

    // Crash-safety: the union marker owns every file either generation may
    // have left behind, so a failed run stays re-collectable. It carries every
    // digest the last export recorded, including for paths this run is about
    // to rewrite. If the process dies mid-replacement the next run reads those
    // bytes as an edit and preserves them: a stray copy of a half-written file
    // in `.wenlan-archive/`, which is the cost of never guessing that
    // unrecognized bytes were ours.
    let union: HashSet<String> = old_files.union(&planned).cloned().collect();
    let carried_digests: BTreeMap<String, String> = old_digests.clone();
    remove_stale_marker_temps(target);
    write_marker_atomically(
        target,
        serde_json::to_vec_pretty(&marker_for(&union, carried_digests.clone()))
            .expect("export marker serializes")
            .as_slice(),
    )?;

    // Stale files: marker-listed, validated, no longer planned, and still
    // ours (an edited one was preserved above and is already gone).
    let mut stale: Vec<&String> = old_files.difference(&planned).collect();
    stale.sort();
    // Entries this pass meant to remove and could not. Continuing past an
    // obstruction is right; FORGETTING it is not. Without this the entry drops
    // out of the final marker, so the next run has no record that the file
    // exists, never retries it, and the bundle keeps an obsolete concept for
    // good -- even after the obstruction is cleared.
    let mut unswept: Vec<String> = Vec::new();
    for entry in stale {
        if !marker_entry_is_safe(entry) {
            continue;
        }
        let path = target.join(entry);
        // Only a plain file is ours to remove. A directory or a symlink
        // standing where a bundle file was is somebody else's, and its
        // presence must not abort an export that has already written the
        // union marker: every retry would fail at the same entry.
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_file() && !m.file_type().is_symlink() => {}
            Ok(_) => {
                log::warn!("[okf] leaving {entry} alone: not a plain file");
                unswept.push(entry.clone());
                stats.failed += 1;
                continue;
            }
            // Already gone: nothing to remember.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            // Any other stat failure says nothing about whether the file is
            // still there, and forgetting it is the bug this block exists to
            // fix, so it stays listed for the next run.
            Err(e) => {
                log::warn!("[okf] could not inspect stale {entry}: {e}");
                unswept.push(entry.clone());
                stats.failed += 1;
                continue;
            }
        }
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("[okf] could not remove stale {entry}: {e}");
                unswept.push(entry.clone());
                stats.failed += 1;
            }
        }
    }

    let mut new_digests: BTreeMap<String, String> = BTreeMap::new();

    let mut written_ids: HashSet<&str> = HashSet::new();
    for (id, file) in &planned_pairs {
        let page = match by_id.get(id.as_str()) {
            Some(page) => page,
            None => continue,
        };
        let entry = format!("{PAGES_DIR}/{file}");
        let body = planned_bytes.get(&entry).expect("page rendered above");
        match write_file_nofollow(&target.join(PAGES_DIR).join(file), body) {
            Ok(()) => {
                stats.exported += 1;
                written_ids.insert(id.as_str());
                new_digests.insert(entry, digest_of(body));
            }
            Err(e) => {
                log::warn!("[okf] export failed for '{}': {}", page.title, e);
                stats.failed += 1;
            }
        }
    }
    for id in &stub_ids {
        let name = crate::export::provenance::stub_filename(id);
        let entry = format!("{SOURCES_DIR}/{name}");
        let stub = planned_bytes.get(&entry).expect("stub rendered above");
        if let Err(e) = write_file_nofollow(&target.join(SOURCES_DIR).join(&name), stub) {
            log::warn!("[okf] source stub failed for '{id}': {e}");
            stats.failed += 1;
        } else {
            new_digests.insert(entry, digest_of(stub));
        }
    }
    // The index lists only pages actually written: a failed page file must
    // not be linked. The final marker still lists every planned file so a
    // later run can clean up.
    let written_pages: Vec<Page> = pages
        .iter()
        .filter(|page| written_ids.contains(page.id.as_str()))
        .cloned()
        .collect();
    let mut index = render_bundle_index(&written_pages, &id_to_file);
    if relative_links {
        index = relative_bundle_links(INDEX_FILE, &index);
    }
    write_file_nofollow(&target.join(INDEX_FILE), index.as_bytes())?;
    new_digests.insert(INDEX_FILE.to_string(), digest_of(index.as_bytes()));
    // A planned file that failed to write keeps whatever digest the previous
    // export recorded for it, so the next run can still tell an edit from the
    // bytes we left there. Digests for files this run removed are dropped:
    // the final marker only describes the bundle it just produced.
    // ... plus the ones it tried to remove and could not. Those keep their
    // recorded digest so the next run can still tell an edit from its own
    // bytes when it retries the removal.
    let still_listed: HashSet<String> = planned
        .iter()
        .cloned()
        .chain(unswept.iter().cloned())
        .collect();
    for (entry, digest) in carried_digests {
        if still_listed.contains(&entry) {
            new_digests.entry(entry).or_insert(digest);
        }
    }
    write_marker_atomically(
        target,
        serde_json::to_vec_pretty(&marker_for(&still_listed, new_digests))
            .expect("export marker serializes")
            .as_slice(),
    )?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_bundle_links_are_relative_to_the_linking_file() {
        let body =
            "[A](/pages/a.md) [S](/sources/s.md) [Home](/index.md) [x](https://e.com/pages/x)";
        assert_eq!(
            relative_bundle_links("index.md", body),
            "[A](pages/a.md) [S](sources/s.md) [Home](index.md) [x](https://e.com/pages/x)"
        );
        assert_eq!(
            relative_bundle_links("pages/b.md", body),
            "[A](a.md) [S](../sources/s.md) [Home](../index.md) [x](https://e.com/pages/x)"
        );
    }

    fn test_page(id: &str, title: &str, content: &str) -> Page {
        Page {
            id: id.to_string(),
            title: title.to_string(),
            summary: Some(format!("{title} summary")),
            content: content.to_string(),
            entity_id: None,
            space: Some("test-space".to_string()),
            source_memory_ids: vec![],
            version: 1,
            status: "active".to_string(),
            created_at: "2026-09-01T00:00:00+00:00".to_string(),
            last_compiled: "2026-09-02T00:00:00+00:00".to_string(),
            last_modified: "2026-09-02T00:00:00+00:00".to_string(),
            sources_updated_count: 0,
            stale_reason: None,
            pending_rebuild: None,
            refresh_blocked_reason: None,
            review_required: false,
            user_edited: false,
            relevance_score: 0.0,
            last_edited_by: None,
            last_edited_at: None,
            last_delta_summary: None,
            changelog: None,
            workspace: None,
            creation_kind: "distilled".to_string(),
            review_status: "confirmed".to_string(),
            citations: Vec::new(),
            kind: "concept".to_string(),
            truth: None,
        }
    }

    fn test_page_in_space(id: &str, title: &str, content: &str, space: Option<&str>) -> Page {
        let mut page = test_page(id, title, content);
        page.space = space.map(|space| space.to_owned());
        page
    }

    fn link_context(pages: &[Page]) -> (HashMap<String, String>, TitleOwners) {
        let id_to_file: HashMap<String, String> = plan_page_filenames(pages).into_iter().collect();
        let title_owner = title_owner_map(pages);
        (id_to_file, title_owner)
    }

    fn convert(pages: &[Page], page: &Page) -> String {
        convert_with_concepts(pages, page, &ConceptPathMap::new())
    }

    fn convert_with_concepts(
        pages: &[Page],
        page: &Page,
        concept_paths: &ConceptPathMap,
    ) -> String {
        let (id_to_file, title_owner) = link_context(pages);
        convert_body(
            &page.content,
            &page.id,
            page.space.as_deref(),
            &page.source_memory_ids,
            &id_to_file,
            &title_owner,
            concept_paths,
        )
    }

    fn convert_all(pages: &[Page], probe: &Page) -> String {
        let all: Vec<Page> = pages
            .iter()
            .cloned()
            .chain(std::iter::once(probe.clone()))
            .collect();
        convert(&all, probe)
    }

    #[test]
    fn concept_link_resolved_to_bundle_path_and_unknown_collapses() {
        let pages = vec![
            test_page("concept_a", "Alpha", "see [Beta](concept_b)"),
            test_page("concept_b", "Beta", "body"),
        ];
        let body = convert(&pages, &pages[0]);
        assert!(body.contains("[Beta](/pages/beta.md)"), "{body}");
        let ghost = test_page("concept_g", "Ghost", "see [Nobody](concept_nobody9)");
        let pages_and_ghost: Vec<Page> = pages
            .iter()
            .cloned()
            .chain(std::iter::once(ghost.clone()))
            .collect();
        let body = convert(&pages_and_ghost, &ghost);
        assert!(body.contains("see Nobody"), "{body}");
        assert!(!body.contains("concept_nobody9"), "{body}");
    }

    #[test]
    fn wikilink_display_heading_and_embed_forms() {
        let pages = [
            test_page("concept_a", "Alpha", "x"),
            test_page("concept_b", "Target Page", "body"),
        ];
        let probe = test_page(
            "concept_probe",
            "Probe",
            "a [[Target Page]] b [[Target Page|Shown]] c [[Target Page#My Heading]] d ![[Target Page]]",
        );
        let all: Vec<Page> = pages
            .iter()
            .cloned()
            .chain(std::iter::once(probe.clone()))
            .collect();
        let body = convert(&all, &probe);
        assert!(
            body.contains("[Target Page](/pages/target-page.md)"),
            "{body}"
        );
        assert!(body.contains("[Shown](/pages/target-page.md)"), "{body}");
        assert!(
            body.contains("[Target Page](/pages/target-page.md#my-heading)"),
            "{body}"
        );
        // Embeds become ordinary links; no `![[` survives.
        assert!(!body.contains("![["), "{body}");
        assert!(!body.contains("[["), "{body}");
    }

    #[test]
    fn ambiguous_title_stays_plain_text() {
        let pages = [
            test_page("concept_a", "Shared Name", "x"),
            test_page("concept_b", "shared name", "y"),
        ];
        let probe = test_page(
            "concept_probe",
            "Probe",
            "see [[Shared Name|here]] and [[Shared Name]]",
        );
        let all: Vec<Page> = pages
            .iter()
            .cloned()
            .chain(std::iter::once(probe.clone()))
            .collect();
        let body = convert(&all, &probe);
        assert!(body.contains("see here and Shared Name"), "{body}");
        assert!(!body.contains("/pages/"), "{body}");
    }

    #[test]
    fn unresolved_wikilink_keeps_display_only() {
        let pages = [test_page("concept_a", "Alpha", "x")];
        let probe = test_page("concept_probe", "Probe", "[[Nobody]] and [[Nobody|Nick]]");
        let all: Vec<Page> = pages
            .iter()
            .cloned()
            .chain(std::iter::once(probe.clone()))
            .collect();
        let body = convert(&all, &probe);
        assert_eq!(body, "Nobody and Nick");
    }

    #[test]
    fn source_cite_links_to_stub_and_sources_section_has_no_delimiters() {
        let mut page = test_page("concept_a", "Alpha", "grounded in [[mem_1]] and [[Beta]]");
        page.source_memory_ids = vec!["mem_1".to_string()];
        let other = test_page("concept_b", "Beta", "body");
        let pages = vec![page.clone(), other];
        let (id_to_file, title_owner) = link_context(&pages);
        let file = render_page_file(
            &page,
            &id_to_file,
            &title_owner,
            &ConceptPathMap::new(),
            &[],
        );
        assert!(file.contains("[mem_1](/sources/mem_1.md)"), "{file}");
        assert!(file.contains("[Beta](/pages/beta.md)"), "{file}");
        assert!(
            file.contains("## Sources\n- [mem_1](/sources/mem_1.md)\n"),
            "{file}"
        );
        assert!(!file.contains("origin:sources"), "{file}");
        assert!(!file.contains("[["), "{file}");
    }

    #[test]
    fn source_less_page_has_no_sources_section() {
        let pages = vec![test_page("concept_a", "Alpha", "plain body")];
        let (id_to_file, title_owner) = link_context(&pages);
        let file = render_page_file(
            &pages[0],
            &id_to_file,
            &title_owner,
            &ConceptPathMap::new(),
            &[],
        );
        assert!(!file.contains("## Sources"), "{file}");
        assert!(file.ends_with("plain body\n"), "{file}");
    }

    #[test]
    fn fenced_code_blocks_are_untouched() {
        let pages = [
            test_page("concept_a", "Alpha", "x"),
            test_page("concept_b", "Target Page", "body"),
        ];
        let fence = "```md\n[Beta](concept_b) and [[Target Page]]\n```";
        let probe = test_page(
            "concept_probe",
            "Probe",
            &format!("before\n{fence}\nafter [[Target Page]]"),
        );
        let all: Vec<Page> = pages
            .iter()
            .cloned()
            .chain(std::iter::once(probe.clone()))
            .collect();
        let body = convert(&all, &probe);
        assert!(body.contains(fence), "{body}");
        assert!(
            body.contains("after [Target Page](/pages/target-page.md)"),
            "{body}"
        );
    }

    #[test]
    fn slug_collision_gets_numeric_suffix() {
        let pages = vec![
            test_page("concept_a", "Same Name", "x"),
            test_page("concept_b", "Same Name", "y"),
            test_page("concept_c", "Same Name", "z"),
        ];
        let planned = plan_page_filenames(&pages);
        let mut files: Vec<&str> = planned.iter().map(|(_, f)| f.as_str()).collect();
        files.sort();
        assert_eq!(
            files,
            vec!["same-name-2.md", "same-name-3.md", "same-name.md"]
        );
    }

    /// The bundle side of the reserved-name rule: `log.md` is a directory's
    /// update history in OKF (spec 3.1), never a concept document.
    #[test]
    fn a_page_titled_log_gets_log_page_in_the_bundle() {
        let pages = vec![
            test_page("concept_log", "Log", "x"),
            test_page("concept_log_page", "Log Page", "y"),
        ];
        let planned: Vec<String> = plan_page_filenames(&pages)
            .into_iter()
            .map(|(_, file)| file)
            .collect();
        assert_eq!(planned, vec!["log-page.md", "log-page-2.md"], "{planned:?}");
    }

    #[test]
    fn a_page_titled_index_and_one_titled_index_page_get_distinct_files() {
        let pages = vec![
            test_page("concept_a", "Index", "x"),
            test_page("concept_b", "Index Page", "y"),
            test_page("concept_c", "INDEX", "z"),
        ];
        let planned = plan_page_filenames(&pages);
        let files: Vec<&str> = planned.iter().map(|(_, f)| f.as_str()).collect();
        assert_eq!(
            files,
            vec!["index-page.md", "index-page-2.md", "index-page-3.md"]
        );
    }

    #[test]
    fn empty_slug_falls_back_to_sanitized_id() {
        let pages = vec![test_page("concept_a!!", "!!!", "x")];
        let planned = plan_page_filenames(&pages);
        assert_eq!(planned.len(), 1);
        let expected = format!(
            "{}.md",
            crate::export::provenance::sanitize_stub_id("concept_a!!")
        );
        assert_eq!(planned[0].1, expected);
    }

    #[test]
    fn page_frontmatter_matches_projection_minus_related() {
        let mut page = test_page("concept_a", "Alpha", "see [[Beta]]");
        page.source_memory_ids = vec!["mem_1".to_string()];
        let projection = crate::export::knowledge::page_frontmatter(&page, true, &[]);
        let bundle = crate::export::knowledge::page_frontmatter(&page, false, &[]);
        assert!(projection.contains("related:"), "{projection}");
        assert!(!bundle.contains("related:"), "{bundle}");
        let projection_without_related: String = projection
            .lines()
            .filter(|line| !line.starts_with("related:"))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(projection_without_related, bundle);
    }

    fn locator(path: &str, start: u32, end: u32) -> CodeBasisLocator {
        CodeBasisLocator {
            repo_root: "/home/someone/private-checkout".to_string(),
            path: path.to_string(),
            line_start: start,
            line_end: end,
        }
    }

    #[test]
    fn rendered_page_file_lists_code_bases_as_repo_sources() {
        let mut page = test_page("concept_a", "Alpha", "body");
        page.source_memory_ids = vec!["mem_1".to_string()];
        let pages = vec![page];
        let (id_to_file, title_owner) = link_context(&pages);
        let file = render_page_file(
            &pages[0],
            &id_to_file,
            &title_owner,
            &ConceptPathMap::new(),
            &[locator("src/lib.rs", 10, 20)],
        );
        assert!(
            file.contains(
                "sources:\n  - {id: \"mem_1\", resource: \"wenlan://memory/mem_1\"}\n  \
                 - {resource: \"repo://src/lib.rs#L10-L20\"}\n"
            ),
            "{file}"
        );
        assert!(!file.contains("private-checkout"), "{file}");
        // Same frontmatter the projection writes, minus `related:`.
        assert!(file.starts_with(&format!(
            "---\n{}---\n\n",
            crate::export::knowledge::page_frontmatter(
                &pages[0],
                false,
                &[locator("src/lib.rs", 10, 20)]
            )
        )));
    }

    /// The importer still drops `repo://` sources: a bundle exported with one
    /// reads back without error and without a source or a link made from it.
    #[test]
    fn a_bundle_with_a_repo_source_still_imports_without_creating_a_source() {
        let dir = tempfile::tempdir().unwrap();
        let pages = vec![test_page("concept_a", "Alpha", "body")];
        let mut code_bases = HashMap::new();
        code_bases.insert("concept_a".to_string(), vec![locator("src/lib.rs", 10, 20)]);
        export_okf_with_concept_paths(&pages, dir.path(), &ConceptPathMap::new(), &code_bases)
            .unwrap();
        let file = dir.path().join(PAGES_DIR).join("alpha.md");
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("repo://src/lib.rs#L10-L20"), "{text}");

        let (concept, _body) = crate::sources::okf::parse_concept(dir.path(), &file, &text)
            .expect("a repo:// source must not make the page unparseable");
        assert!(
            concept.links.is_empty(),
            "a repo:// source is not a link to another concept: {:?}",
            concept.links
        );
        let outcome =
            crate::sources::okf::concept_file_to_documents("okf-test", &file, dir.path(), None);
        assert!(
            !matches!(outcome, crate::sources::okf::ConceptOutcome::Error(_)),
            "{outcome:?}"
        );
        assert!(
            !matches!(
                outcome,
                crate::sources::okf::ConceptOutcome::Ingested { .. }
            ),
            "an exported page is Wenlan's own and is never ingested as a source: {outcome:?}"
        );
    }

    #[test]
    fn rendered_page_file_opens_with_shared_frontmatter() {
        let pages = vec![test_page("concept_a", "Alpha", "body")];
        let (id_to_file, title_owner) = link_context(&pages);
        let file = render_page_file(
            &pages[0],
            &id_to_file,
            &title_owner,
            &ConceptPathMap::new(),
            &[],
        );
        let expected_head = format!(
            "---\n{}---\n\n",
            crate::export::knowledge::page_frontmatter(&pages[0], false, &[])
        );
        assert!(file.starts_with(&expected_head), "{file}");
    }

    #[test]
    fn source_stub_frontmatter_points_at_memory() {
        let stub = render_source_stub("mem_1");
        assert!(stub.contains("type: source"), "{stub}");
        assert!(stub.contains("title: \"mem_1\""), "{stub}");
        assert!(
            stub.contains("resource: \"wenlan://memory/mem_1\""),
            "{stub}"
        );
        assert!(!stub.contains("origin_stub"), "{stub}");
        // One sentence, no memory content beyond the id.
        assert!(stub.matches('\n').count() >= 6, "{stub}");
        assert!(!stub.contains("origin:"), "{stub}");
    }

    #[test]
    fn bundle_index_uses_pages_prefix_and_groups_unfiled_last() {
        let mut filed = test_page("concept_a", "Zulu", "x");
        filed.space = Some("b-space".to_string());
        let mut unfiled = test_page("concept_b", "Alpha", "y");
        unfiled.space = None;
        let pages = vec![filed, unfiled];
        let id_to_file: HashMap<String, String> = plan_page_filenames(&pages).into_iter().collect();
        let index = render_bundle_index(&pages, &id_to_file);
        assert!(
            index.starts_with("---\nokf_version: \"0.2\"\n---\n\n"),
            "{index}"
        );
        assert!(index.contains("[Zulu](/pages/zulu.md)"), "{index}");
        assert!(index.contains("[Alpha](/pages/alpha.md)"), "{index}");
        assert!(!index.contains("](/zulu.md)"), "{index}");
        let unfiled_pos = index.find("## Unfiled").expect("Unfiled section");
        let space_pos = index.find("## b-space").expect("space section");
        assert!(space_pos < unfiled_pos, "{index}");
    }

    /// Same rule in the exported bundle: no summary, no ` - ` tail. The
    /// renderer is shared with the projection index, so this pins the bundle
    /// side of it against a future prefix-only change.
    #[test]
    fn bundle_index_line_for_a_page_without_a_summary_has_no_trailing_separator() {
        let described = test_page("concept_a", "Described", "x");
        let mut bare = test_page("concept_b", "Bare", "y");
        bare.summary = None;
        let pages = vec![described, bare];
        let id_to_file: HashMap<String, String> = plan_page_filenames(&pages).into_iter().collect();
        let index = render_bundle_index(&pages, &id_to_file);
        assert!(
            index.contains("* [Described](/pages/described.md) - Described summary\n"),
            "{index}"
        );
        assert!(index.contains("* [Bare](/pages/bare.md)\n"), "{index}");
        assert!(!index.contains("(/pages/bare.md) - "), "{index}");
    }

    /// `export_okf` never reports a skip of its own. The OKF export route
    /// (`wenlan-server/src/page_routes.rs`, `handle_export_pages`) filters the
    /// page list by `page_write_permit` BEFORE calling this, then overwrites
    /// `stats.skipped` with its own declined count -- so a skip counted here
    /// would be silently thrown away. Re-export, where stale files are removed,
    /// is the case most likely to want one.
    #[test]
    fn export_reports_no_skips_of_its_own_so_the_route_owns_that_count() {
        let dir = tempfile::TempDir::new().unwrap();
        let both = vec![
            test_page("concept_a", "Alpha", "x"),
            test_page("concept_b", "Beta", "y"),
        ];
        let first = export_okf(&both, dir.path()).unwrap();
        assert_eq!(first.exported, 2, "{first:?}");
        assert_eq!(first.skipped, 0, "{first:?}");

        // Second pass over the same directory with one page gone: the stale
        // file is removed, and that is still not a "skip".
        let second = export_okf(&both[..1], dir.path()).unwrap();
        assert_eq!(second.exported, 1, "{second:?}");
        assert_eq!(second.skipped, 0, "{second:?}");
        assert_eq!(second.failed, 0, "{second:?}");
    }

    /// Astra finding 1. The marker's file list only ever proved Wenlan WROTE
    /// a file. A re-export used it as permission to overwrite, so an edit made
    /// in the exported bundle disappeared on the next run.
    #[test]
    fn reexport_preserves_an_edited_page_instead_of_overwriting_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let pages = vec![test_page("concept_a", "Alpha", "first body")];
        export_okf(&pages, dir.path()).unwrap();

        let mine = "---\ntype: page\n---\n\nMy own rewrite of Alpha.\n";
        std::fs::write(dir.path().join("pages/alpha.md"), mine).unwrap();

        let later = vec![test_page("concept_a", "Alpha", "second body")];
        let stats = export_okf(&later, dir.path()).unwrap();
        assert_eq!(stats.exported, 1, "{stats:?}");

        assert_eq!(
            std::fs::read_to_string(dir.path().join(".wenlan-archive/pages/alpha.md")).unwrap(),
            mine,
            "the edit is preserved, byte for byte"
        );
        let fresh = std::fs::read_to_string(dir.path().join("pages/alpha.md")).unwrap();
        assert!(fresh.contains("second body"), "{fresh}");
    }

    /// The same mistake on the deletion path: a page renamed or archived in
    /// Wenlan made its exported file stale, and stale meant unlink.
    #[test]
    fn reexport_preserves_an_edited_stale_page_instead_of_deleting_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let both = vec![
            test_page("concept_a", "Alpha", "x"),
            test_page("concept_b", "Beta", "y"),
        ];
        export_okf(&both, dir.path()).unwrap();

        let mine = "---\ntype: page\n---\n\nNotes I added to Beta.\n";
        std::fs::write(dir.path().join("pages/beta.md"), mine).unwrap();

        export_okf(&both[..1], dir.path()).unwrap();

        assert!(
            !dir.path().join("pages/beta.md").exists(),
            "the stale file still leaves the bundle"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".wenlan-archive/pages/beta.md")).unwrap(),
            mine,
            "but its bytes are preserved, not unlinked"
        );
    }

    /// An untouched bundle file is still Wenlan's: it is replaced and removed
    /// exactly as before, with nothing accumulating in the archive.
    #[test]
    fn reexport_does_not_archive_files_it_wrote_itself() {
        let dir = tempfile::TempDir::new().unwrap();
        let both = vec![
            test_page("concept_a", "Alpha", "x"),
            test_page("concept_b", "Beta", "y"),
        ];
        export_okf(&both, dir.path()).unwrap();
        export_okf(&both[..1], dir.path()).unwrap();
        assert!(!dir.path().join("pages/beta.md").exists(), "stale removed");
        assert!(
            !dir.path().join(".wenlan-archive").exists(),
            "nothing to preserve, so no archive directory"
        );
    }

    /// A bundle written before digests existed records no digest to compare
    /// against, so the only evidence left is whether the bytes on disk are
    /// already the bytes this run would write. An edit is not, so it is
    /// preserved like any other; an untouched file is, so nothing is archived
    /// for it. Either way the run records digests, and every run after that
    /// can check properly.
    #[test]
    fn a_pre_digest_bundle_still_preserves_an_edit_and_gains_digests() {
        let dir = tempfile::TempDir::new().unwrap();
        let pages = vec![test_page("concept_a", "Alpha", "x")];
        export_okf(&pages, dir.path()).unwrap();
        // Rewrite the marker the way 0.18.10 wrote it: files, no digests.
        let strip_digests = |dir: &Path| {
            let mut marker: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.join(MARKER_FILE)).unwrap()).unwrap();
            marker.as_object_mut().unwrap().remove("digests");
            std::fs::write(
                dir.join(MARKER_FILE),
                serde_json::to_vec_pretty(&marker).unwrap(),
            )
            .unwrap();
        };
        strip_digests(dir.path());

        // An untouched pre-digest bundle costs nothing to upgrade.
        export_okf(&pages, dir.path()).unwrap();
        assert!(
            !dir.path().join(".wenlan-archive").exists(),
            "bytes we would write anyway are ours, digest or no digest"
        );
        let marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join(MARKER_FILE)).unwrap()).unwrap();
        assert_eq!(
            marker["digests"]["pages/alpha.md"].as_str().unwrap(),
            digest_of(&std::fs::read(dir.path().join("pages/alpha.md")).unwrap()),
            "the digest recorded matches the bytes on disk"
        );

        // An edit made against a pre-digest bundle survives the upgrade run.
        strip_digests(dir.path());
        std::fs::write(dir.path().join("pages/alpha.md"), "edited before upgrade").unwrap();
        export_okf(&pages, dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".wenlan-archive/pages/alpha.md")).unwrap(),
            "edited before upgrade",
            "Muse: the one-time upgrade overwrite is gone"
        );

        // With a digest on record, the next edit IS preserved too.
        std::fs::write(dir.path().join("pages/alpha.md"), "edited after upgrade").unwrap();
        export_okf(&pages, dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".wenlan-archive/pages/alpha-2.md")).unwrap(),
            "edited after upgrade"
        );
    }

    /// Muse: the export used to unlink a symlink standing at a planned path
    /// and write a regular file over it, which protected the link's target and
    /// destroyed the link. Both are somebody's file.
    #[test]
    // Both cases turn on a symlink standing at a planned path, which only
    // Unix can create here; Windows proves the other side of the gate.
    #[cfg(unix)]
    fn a_symlink_at_a_planned_path_is_left_alone() {
        let dir = tempfile::TempDir::new().unwrap();
        let pages = vec![test_page("concept_a", "Alpha", "x")];
        export_okf(&pages, dir.path()).unwrap();

        let elsewhere = dir.path().join("precious.md");
        std::fs::write(&elsewhere, "the user's real note").unwrap();
        let link = dir.path().join("pages/alpha.md");
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();

        let stats = export_okf(&pages, dir.path()).unwrap();
        assert_eq!((stats.exported, stats.failed), (0, 1), "reported, not done");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link itself survives"
        );
        assert_eq!(
            std::fs::read_to_string(&elsewhere).unwrap(),
            "the user's real note"
        );
    }

    /// Muse: a stale entry replaced by a directory used to abort the whole
    /// export with the union marker already written, so every retry failed at
    /// the same entry.
    #[test]
    fn a_stale_entry_that_is_not_a_plain_file_does_not_abort_the_export() {
        let dir = tempfile::TempDir::new().unwrap();
        let both = vec![
            test_page("concept_a", "Alpha", "x"),
            test_page("concept_b", "Beta", "y"),
        ];
        export_okf(&both, dir.path()).unwrap();

        std::fs::remove_file(dir.path().join("pages/beta.md")).unwrap();
        std::fs::create_dir(dir.path().join("pages/beta.md")).unwrap();
        std::fs::write(dir.path().join("pages/beta.md/inside.md"), "mine").unwrap();

        let stats = export_okf(&both[..1], dir.path()).unwrap();
        assert_eq!(stats.exported, 1, "the export still finishes");
        assert_eq!(
            stats.failed, 1,
            "and reports the entry it could not sweep rather than swallowing it"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("pages/beta.md/inside.md")).unwrap(),
            "mine",
            "and leaves what it does not understand alone"
        );
    }

    /// A stale entry the sweep could not remove stays in the marker. It used
    /// to be dropped, which meant the next run had no record that the file
    /// existed, never retried it, and the bundle kept an obsolete concept for
    /// good -- even after whatever blocked the removal was cleared.
    #[test]
    fn an_obstructed_stale_entry_stays_in_the_marker_so_a_later_run_retries_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let both = vec![
            test_page("concept_a", "Alpha", "x"),
            test_page("concept_b", "Beta", "y"),
        ];
        export_okf(&both, dir.path()).unwrap();
        let beta = std::fs::read(dir.path().join("pages/beta.md")).unwrap();

        // Something that is not ours takes the name the stale page occupies.
        std::fs::remove_file(dir.path().join("pages/beta.md")).unwrap();
        std::fs::create_dir(dir.path().join("pages/beta.md")).unwrap();
        std::fs::write(dir.path().join("pages/beta.md/inside.md"), "mine").unwrap();

        let stats = export_okf(&both[..1], dir.path()).unwrap();
        assert_eq!(stats.failed, 1, "the entry it could not sweep is reported");

        let marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join(MARKER_FILE)).unwrap()).unwrap();
        let files: Vec<&str> = marker["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(files.contains(&"pages/beta.md"), "{files:?}");
        assert_eq!(
            marker["digests"]["pages/beta.md"].as_str().unwrap(),
            digest_of(&beta),
            "and keeps its digest, so the retry can still tell our bytes from an edit"
        );

        // The obstruction clears and Wenlan's own file is back at that name.
        std::fs::remove_dir_all(dir.path().join("pages/beta.md")).unwrap();
        std::fs::write(dir.path().join("pages/beta.md"), &beta).unwrap();

        export_okf(&both[..1], dir.path()).unwrap();
        assert!(
            !dir.path().join("pages/beta.md").exists(),
            "the next run finishes the removal it could not do before"
        );
    }

    /// Muse: a planned write that failed used to lose its digest, so the next
    /// run could not tell an edit at that path from the bytes it left there.
    #[test]
    // Both cases turn on a symlink standing at a planned path, which only
    // Unix can create here; Windows proves the other side of the gate.
    #[cfg(unix)]
    fn a_failed_write_keeps_its_digest_so_the_next_run_can_still_tell() {
        let dir = tempfile::TempDir::new().unwrap();
        let pages = vec![test_page("concept_a", "Alpha", "x")];
        export_okf(&pages, dir.path()).unwrap();
        let first = std::fs::read(dir.path().join("pages/alpha.md")).unwrap();

        // Make the planned write fail without disturbing the recorded bytes.
        let link = dir.path().join("pages/alpha.md");
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(dir.path().join("precious.md"), &link).unwrap();
        export_okf(&pages, dir.path()).unwrap();

        let marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join(MARKER_FILE)).unwrap()).unwrap();
        assert_eq!(
            marker["digests"]["pages/alpha.md"].as_str().unwrap(),
            digest_of(&first),
            "the failed path keeps the digest of the bytes actually on disk"
        );
    }

    /// Astra finding 3. An imported concept's body links in its ORIGINAL
    /// bundle's terms; the export used to write that link out untouched, so it
    /// pointed at a path the bundle does not contain.
    #[test]
    fn an_imported_concept_link_resolves_to_the_exported_page() {
        let beta = test_page("concept_b", "Beta", "y");
        let alpha = test_page(
            "concept_a",
            "Alpha",
            "See [Beta](/concepts/b.md) and its [setup](/concepts/b.md#setup).",
        );
        let concept_paths = concept_path_map(&[(
            "concept_b".to_string(),
            "src_1".to_string(),
            "concepts/b".to_string(),
        )]);

        let body = convert_with_concepts(&[alpha.clone(), beta], &alpha, &concept_paths);

        assert!(body.contains("[Beta](/pages/beta.md)"), "{body}");
        assert!(body.contains("[setup](/pages/beta.md#setup)"), "{body}");
    }

    /// An absolute link the bundle cannot satisfy collapses to its text, the
    /// same way an unknown page id already does. A bundle that promises a
    /// document it does not carry is worse than prose that lost a link.
    #[test]
    fn an_unresolvable_absolute_concept_link_degrades_to_text() {
        let alpha = test_page("concept_a", "Alpha", "See [Gamma](/concepts/gamma.md).");
        let body = convert(std::slice::from_ref(&alpha), &alpha);
        assert_eq!(body.trim(), "See Gamma.", "{body}");
    }

    fn imported(page: &str, source: &str, concept: &str) -> (String, String, String) {
        (page.to_string(), source.to_string(), concept.to_string())
    }

    /// Astra finding 4. An imported bundle holds `pages/b.md` ("Beta") and
    /// `sources/x.md`; a concept links to both in the bundle's terms. The
    /// export names Beta `beta.md` and the other concept `x-notes.md`, so the
    /// verbatim `/pages/` and `/sources/` paths dangled. The link follows the
    /// target's identity to its export file and keeps the fragment.
    #[test]
    fn an_imported_pages_and_sources_link_follows_a_renamed_target() {
        let alpha = test_page(
            "concept_a",
            "Alpha",
            "See [Beta](/pages/b.md), its [setup](/pages/b.md#setup) and [X](/sources/x.md#top).",
        );
        let beta = test_page("concept_b", "Beta", "y");
        let notes = test_page("concept_x", "X Notes", "z");
        let concept_paths = concept_path_map(&[
            imported("concept_a", "src_1", "concepts/a"),
            imported("concept_b", "src_1", "pages/b"),
            imported("concept_x", "src_1", "sources/x"),
        ]);

        let body = convert_with_concepts(&[alpha.clone(), beta, notes], &alpha, &concept_paths);

        assert!(body.contains("[Beta](/pages/beta.md)"), "{body}");
        assert!(body.contains("[setup](/pages/beta.md#setup)"), "{body}");
        assert!(body.contains("[X](/pages/x-notes.md#top)"), "{body}");
    }

    /// The dangerous half: after the rename a DIFFERENT page owns `b.md`. The
    /// old spelling would silently retarget the link at it; the target's
    /// identity must win, and the link written for the other page by name is
    /// not disturbed.
    #[test]
    fn a_different_page_now_holding_the_old_filename_does_not_capture_the_link() {
        let alpha = test_page("concept_a", "Alpha", "[Beta](/pages/b.md)");
        let beta = test_page("concept_b", "Beta", "y");
        let other = test_page("concept_o", "B", "unrelated");
        let concept_paths = concept_path_map(&[
            imported("concept_a", "src_1", "concepts/a"),
            imported("concept_b", "src_1", "pages/b"),
        ]);
        let pages = [alpha.clone(), beta, other];
        let (id_to_file, _) = link_context(&pages);
        assert_eq!(id_to_file["concept_o"], "b.md", "the old filename is taken");

        let body = convert_with_concepts(&pages, &alpha, &concept_paths);

        assert_eq!(body.trim(), "[Beta](/pages/beta.md)", "{body}");
    }

    /// Relative links resolve from the importing concept's original folder,
    /// the way the importer resolved them, then follow the target's identity.
    #[test]
    fn an_imported_relative_link_resolves_from_the_original_folder() {
        let alpha = test_page(
            "concept_a",
            "Alpha",
            "[Beta](./b.md#why) [Up](../x/b.md) [Bare](b.md) [Gamma](../c.md)",
        );
        let beta = test_page("concept_b", "Beta", "y");
        let xbeta = test_page("concept_xb", "Xbeta", "y");
        let gamma = test_page("concept_c", "Gamma", "y");
        let other = test_page("concept_o", "B", "unrelated");
        let concept_paths = concept_path_map(&[
            imported("concept_a", "src_1", "concepts/a"),
            imported("concept_b", "src_1", "concepts/b"),
            imported("concept_xb", "src_1", "x/b"),
            imported("concept_c", "src_1", "c"),
        ]);

        let body = convert_with_concepts(
            &[alpha.clone(), beta, xbeta, gamma, other],
            &alpha,
            &concept_paths,
        );

        assert_eq!(
            body.trim(),
            "[Beta](/pages/beta.md#why) [Up](/pages/xbeta.md) [Bare](/pages/beta.md) \
             [Gamma](/pages/gamma.md)",
            "{body}"
        );
    }

    /// A link whose target cannot be identified stays as written (OKF 6.1
    /// tolerates a broken link) rather than being guessed, and another
    /// bundle's concept of the same path is not the target.
    #[test]
    fn an_unidentifiable_imported_link_is_left_as_written() {
        let alpha = test_page(
            "concept_a",
            "Alpha",
            "[Z](/pages/zzz.md) [N](./nope.md) [S](/sources/nothing.md) [B](/pages/b.md)",
        );
        let beta = test_page("concept_b", "Beta", "y");
        let concept_paths = concept_path_map(&[
            imported("concept_a", "src_1", "concepts/a"),
            imported("concept_b", "src_2", "pages/b"),
        ]);

        let body = convert_with_concepts(&[alpha.clone(), beta], &alpha, &concept_paths);

        assert_eq!(body.trim(), alpha.content, "{body}");
    }

    /// Identified but not exported: the path is not left behind for another
    /// page to take over, so the link degrades to its text like an
    /// unresolvable `/concepts/` link does.
    #[test]
    fn an_imported_link_to_a_page_that_is_not_exported_degrades_to_text() {
        let alpha = test_page("concept_a", "Alpha", "[Beta](/pages/b.md) [Rel](./b.md)");
        let other = test_page("concept_o", "B", "unrelated");
        let concept_paths = concept_path_map(&[
            imported("concept_a", "src_1", "concepts/a"),
            imported("concept_b", "src_1", "pages/b"),
            imported("concept_b", "src_1", "concepts/b"),
        ]);

        let body = convert_with_concepts(&[alpha.clone(), other], &alpha, &concept_paths);

        assert_eq!(body.trim(), "Beta Rel", "{body}");
    }

    /// A page that was not imported has no bundle to read its paths in, so
    /// its own `/pages/` and relative links are not reinterpreted.
    #[test]
    fn a_page_that_was_not_imported_keeps_its_pages_and_relative_links() {
        let alpha = test_page("concept_a", "Alpha", "[B](/pages/b.md) [R](./b.md)");
        let beta = test_page("concept_b", "Beta", "y");
        let concept_paths = concept_path_map(&[imported("concept_b", "src_1", "pages/b")]);

        let body = convert_with_concepts(&[alpha.clone(), beta], &alpha, &concept_paths);

        assert_eq!(body.trim(), alpha.content, "{body}");
    }

    /// The `/pages/<file>` link written for a `concept_<id>` link is this
    /// bundle's own address, not an imported concept's original path.
    #[test]
    fn a_page_id_link_is_not_reread_as_an_imported_path() {
        let alpha = test_page("concept_a", "Alpha", "[Beta](concept_b)");
        let beta = test_page("concept_b", "Beta", "y");
        let gamma = test_page("concept_c", "Gamma", "y");
        let concept_paths = concept_path_map(&[
            imported("concept_a", "src_1", "concepts/a"),
            imported("concept_c", "src_1", "pages/beta"),
        ]);

        let body = convert_with_concepts(&[alpha.clone(), beta, gamma], &alpha, &concept_paths);

        assert_eq!(body.trim(), "[Beta](/pages/beta.md)", "{body}");
    }

    /// A bundle's `/pages/b.md` is its own: two bundles holding the same path
    /// each resolve to their own concept, where a page of neither would find
    /// the path ambiguous.
    #[test]
    fn two_bundles_sharing_a_path_each_resolve_their_own() {
        let one = test_page("concept_1", "One", "[B](/pages/b.md)");
        let two = test_page("concept_2", "Two", "[B](/pages/b.md)");
        let b1 = test_page("concept_b1", "Beta One", "y");
        let b2 = test_page("concept_b2", "Beta Two", "y");
        let concept_paths = concept_path_map(&[
            imported("concept_1", "src_1", "one"),
            imported("concept_b1", "src_1", "pages/b"),
            imported("concept_2", "src_2", "two"),
            imported("concept_b2", "src_2", "pages/b"),
        ]);
        let pages = [one.clone(), two.clone(), b1, b2];

        assert_eq!(
            convert_with_concepts(&pages, &one, &concept_paths).trim(),
            "[B](/pages/beta-one.md)"
        );
        assert_eq!(
            convert_with_concepts(&pages, &two, &concept_paths).trim(),
            "[B](/pages/beta-two.md)"
        );
    }

    /// End to end through the exporter: the written page carries the repaired
    /// link, and a repository bundle makes it relative to the linking file.
    #[test]
    fn export_writes_the_repaired_link_in_both_bundle_shapes() {
        let alpha = test_page("concept_a", "Alpha", "[Beta](/pages/b.md#setup)");
        let beta = test_page("concept_b", "Beta", "y");
        let other = test_page("concept_o", "B", "unrelated");
        let concept_paths = concept_path_map(&[
            imported("concept_a", "src_1", "concepts/a"),
            imported("concept_b", "src_1", "pages/b"),
        ]);
        let pages = [alpha, beta, other];

        let dir = tempfile::TempDir::new().unwrap();
        export_okf_with_concept_paths(&pages, dir.path(), &concept_paths, &HashMap::new()).unwrap();
        let written = std::fs::read_to_string(dir.path().join("pages/alpha.md")).unwrap();
        assert!(
            written.contains("[Beta](/pages/beta.md#setup)"),
            "{written}"
        );

        let repo = tempfile::TempDir::new().unwrap();
        export_okf_for_repository(&pages, repo.path(), &concept_paths, &HashMap::new()).unwrap();
        let written = std::fs::read_to_string(repo.path().join("pages/alpha.md")).unwrap();
        assert!(written.contains("[Beta](beta.md#setup)"), "{written}");
    }

    /// The exporter's own absolute links, and non-markdown targets, pass
    /// through untouched.
    #[test]
    fn absolute_bundle_links_and_assets_are_left_alone() {
        let alpha = test_page(
            "concept_a",
            "Alpha",
            "[Home](/index.md) [P](/pages/beta.md) [S](/sources/mem_1.md) \
             [Pic](/assets/diagram.png)",
        );
        let body = convert(std::slice::from_ref(&alpha), &alpha);
        for link in [
            "[Home](/index.md)",
            "[P](/pages/beta.md)",
            "[S](/sources/mem_1.md)",
            "[Pic](/assets/diagram.png)",
        ] {
            assert!(body.contains(link), "{link} missing from {body}");
        }
    }

    /// Two bundles can both hold `concepts/b.md`, and a body link carries no
    /// source. An ambiguous path resolves to nothing rather than to a guess.
    #[test]
    fn concept_path_map_drops_a_path_two_bundles_share() {
        let map = concept_path_map(&[
            (
                "page_1".to_string(),
                "src_1".to_string(),
                "concepts/b".to_string(),
            ),
            (
                "page_2".to_string(),
                "src_2".to_string(),
                "concepts/b".to_string(),
            ),
            (
                "page_3".to_string(),
                "src_1".to_string(),
                "concepts/c".to_string(),
            ),
        ]);
        assert_eq!(map.get("/concepts/b.md"), None, "ambiguous path dropped");
        assert_eq!(
            map.get("/concepts/c.md").map(String::as_str),
            Some("page_3")
        );
    }

    #[test]
    fn refuses_foreign_nonempty_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("README.md"), b"someone else").unwrap();
        let pages = vec![test_page("concept_a", "Alpha", "x")];
        let err = export_okf(&pages, dir.path()).unwrap_err();
        assert!(
            err.to_string()
                .contains("directory is not empty and is not a Wenlan OKF export"),
            "{err}"
        );
    }

    #[test]
    fn corrupt_marker_is_a_conflict() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(MARKER_FILE), b"{not json").unwrap();
        let pages = vec![test_page("concept_a", "Alpha", "x")];
        let err = export_okf(&pages, dir.path()).unwrap_err();
        assert!(matches!(err, WenlanError::Conflict(_)), "{err}");
    }

    #[test]
    fn reexport_keeps_user_files_and_removes_stale_pages() {
        let dir = tempfile::TempDir::new().unwrap();
        let both = vec![
            test_page("concept_a", "Alpha", "x"),
            test_page("concept_b", "Beta", "y"),
        ];
        let stats = export_okf(&both, dir.path()).unwrap();
        assert_eq!(stats.exported, 2);
        // User files beside the bundle (never in the marker).
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join("README.md"), b"mine").unwrap();
        assert!(dir.path().join("pages/beta.md").exists());

        let just_a = vec![test_page("concept_a", "Alpha", "x")];
        let stats = export_okf(&just_a, dir.path()).unwrap();
        assert_eq!(stats.exported, 1);
        assert!(
            !dir.path().join("pages/beta.md").exists(),
            "stale page removed"
        );
        assert!(dir.path().join("pages/alpha.md").exists());
        assert!(dir.path().join("README.md").exists(), "user file kept");
        assert!(dir.path().join(".git").is_dir(), ".git kept");
        let marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join(MARKER_FILE)).unwrap()).unwrap();
        let files: HashSet<String> = marker["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(!files.iter().any(|f| f.contains("beta")), "{files:?}");
        assert!(files.contains("index.md"), "{files:?}");
        assert!(files.contains("pages/alpha.md"), "{files:?}");
    }

    #[test]
    fn malicious_marker_entries_delete_nothing_outside() {
        let outer = tempfile::TempDir::new().unwrap();
        let root = outer.path().join("bundle");
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::write(outer.path().join("evil.md"), b"precious").unwrap();
        std::fs::write(root.join("pages").join("keep.md"), b"user note").unwrap();
        let marker = serde_json::json!({
            "okf_version": "0.2",
            "generated_by": "wenlan/test",
            "files": ["../evil.md", "pages/../../evil.md", "pages/.hidden.md", "index.md", "/abs.md"],
        });
        std::fs::write(root.join(MARKER_FILE), serde_json::to_vec(&marker).unwrap()).unwrap();
        let stats = export_okf(&[], root.as_path()).unwrap();
        assert_eq!(stats.exported, 0);
        assert!(
            outer.path().join("evil.md").exists(),
            "outside file untouched"
        );
        assert!(
            root.join("pages").join("keep.md").exists(),
            "unlisted file kept"
        );
    }

    #[test]
    fn marker_entry_validation_rejects_escapes() {
        assert!(marker_entry_is_safe("index.md"));
        assert!(marker_entry_is_safe("pages/alpha.md"));
        assert!(marker_entry_is_safe("sources/mem_1.md"));
        for evil in [
            "../x.md",
            "pages/../../x.md",
            "pages/.hidden.md",
            ".hidden.md",
            "pages/a/b.md",
            "pages/a\\b.md",
            "pages/a..b.md",
            "pages/.md",
            "pages/x.txt",
            "other/x.md",
            "",
        ] {
            assert!(!marker_entry_is_safe(evil), "{evil} must be rejected");
        }
    }

    #[test]
    fn bundle_conforms_okf_shape() {
        let mut a = test_page(
            "concept_a",
            "Alpha",
            "see [[Beta]]\n\n```md\n[[untouched]]\n```",
        );
        a.source_memory_ids = vec!["mem_1".to_string()];
        let mut b = test_page("concept_b", "Beta", "plain");
        b.space = None;
        b.summary = None;
        let pages = vec![a, b];
        let dir = tempfile::TempDir::new().unwrap();
        export_okf(&pages, dir.path()).unwrap();

        let mut md_files = Vec::new();
        collect_md(dir.path(), &mut md_files);
        assert!(md_files.len() >= 4, "{md_files:?}");
        for path in &md_files {
            let rel = path.strip_prefix(dir.path()).unwrap();
            let content = std::fs::read_to_string(path).unwrap();
            let (fm, _body) = crate::sources::obsidian::extract_frontmatter(&content);
            if rel == Path::new("index.md") {
                let keys: HashSet<String> = fm.fields.keys().cloned().collect();
                assert_eq!(
                    keys,
                    HashSet::from(["okf_version".to_string()]),
                    "{content}"
                );
                assert_eq!(fm.get_str("okf_version"), Some("0.2"));
            } else {
                let ty = fm.get_str("type").unwrap_or("");
                assert!(!ty.is_empty(), "{rel:?} needs a non-empty type:\n{content}");
            }
            assert!(
                !outside_code_contains(&content, "[["),
                "{rel:?} leaks wikilinks"
            );
            assert!(
                !outside_code_contains(&content, "<!-- origin:"),
                "{rel:?} leaks delimiters"
            );
        }
    }

    #[test]
    fn wikilinks_resolve_within_the_linking_page_space() {
        let alpha_a = test_page_in_space("page_alpha_a", "Alpha", "A", Some("a"));
        let alpha_b = test_page_in_space("page_alpha_b", "Alpha", "B", Some("b"));
        let beta_a = test_page_in_space("page_beta_a", "Beta", "C", Some("a"));
        let linker_a = test_page_in_space("page_linker_a", "Linker A", "see [[Alpha]]", Some("a"));
        let linker_b = test_page_in_space(
            "page_linker_b",
            "Linker B",
            "see [[Alpha]] and [[Beta]]",
            Some("b"),
        );
        let pages = vec![alpha_a, alpha_b, beta_a, linker_a.clone(), linker_b.clone()];
        // Globally "Alpha" is ambiguous, but per Space each linker sees one.
        let body_a = convert(&pages, &linker_a);
        assert!(body_a.contains("[Alpha](/pages/alpha.md)"), "{body_a}");
        let body_b = convert(&pages, &linker_b);
        assert!(body_b.contains("[Alpha](/pages/alpha-2.md)"), "{body_b}");
        // Beta lives only in space `a`, so the space-`b` link stays plain.
        assert!(body_b.contains("and Beta"), "{body_b}");
        assert!(!body_b.contains("/pages/beta.md"), "{body_b}");
    }

    #[test]
    fn page_id_links_resolve_and_unknown_collapse() {
        let target_id = "page_12345678-1234-4000-8000-1234567890ab";
        let pages = vec![
            test_page("concept_a", "Alpha", "x"),
            test_page(target_id, "Known", "body"),
        ];
        let probe = test_page(
            "concept_probe",
            "Probe",
            &format!("[Known]({target_id}) and [Gone](page_00000000-0000-4000-8000-00000000dead)"),
        );
        let body = convert_all(&pages, &probe);
        assert!(body.contains("[Known](/pages/known.md)"), "{body}");
        assert!(body.contains("and Gone"), "{body}");
        assert!(!body.contains("page_00000000"), "{body}");
    }

    #[test]
    fn inline_code_spans_are_untouched() {
        let pages = [
            test_page("concept_a", "Alpha", "x"),
            test_page("concept_b", "Target Page", "body"),
        ];
        let probe = test_page(
            "concept_probe",
            "Probe",
            "use `[[Target Page]]` verbatim and see [[Target Page]]",
        );
        let body = convert_all(&pages, &probe);
        assert!(body.contains("use `[[Target Page]]` verbatim"), "{body}");
        assert!(
            body.contains("see [Target Page](/pages/target-page.md)"),
            "{body}"
        );
    }

    #[test]
    fn double_backtick_span_with_inner_backtick_is_untouched() {
        let pages = [test_page("concept_b", "Target Page", "body")];
        let probe = test_page(
            "concept_probe",
            "Probe",
            "``a ` [[Target Page]]`` then [[Target Page]]",
        );
        let body = convert_all(&pages, &probe);
        assert!(body.contains("``a ` [[Target Page]]``"), "{body}");
        assert!(
            body.contains("then [Target Page](/pages/target-page.md)"),
            "{body}"
        );
    }

    #[test]
    fn inline_code_span_does_not_cross_a_blank_line() {
        let pages = [test_page("concept_b", "Target Page", "body")];
        let probe = test_page(
            "concept_probe",
            "Probe",
            "a ` stray\n\nsee [[Target Page]]\n\nanother ` stray",
        );
        let body = convert_all(&pages, &probe);
        assert!(
            body.contains("see [Target Page](/pages/target-page.md)"),
            "{body}"
        );
    }

    #[test]
    fn inline_code_span_does_not_cross_a_fenced_block() {
        let pages = [test_page("concept_b", "Target Page", "body")];
        let probe = test_page(
            "concept_probe",
            "Probe",
            "a ` stray\n```\ncode\n```\nsee [[Target Page]] and ` stray",
        );
        let body = convert_all(&pages, &probe);
        assert!(
            body.contains("see [Target Page](/pages/target-page.md)"),
            "{body}"
        );
    }

    #[test]
    fn unmatched_backtick_does_not_suppress_later_links() {
        let pages = [test_page("concept_b", "Target Page", "body")];
        let probe = test_page("concept_probe", "Probe", "a ` stray then [[Target Page]]");
        let body = convert_all(&pages, &probe);
        assert!(
            body.contains("then [Target Page](/pages/target-page.md)"),
            "{body}"
        );
    }

    #[test]
    fn emitted_link_labels_escape_brackets() {
        let pages = vec![
            test_page("concept_a", "Alpha", "x"),
            test_page("concept_b", "Target [Page", "body"),
        ];
        let probe = test_page("concept_probe", "Probe", "see [[Target [Page]]");
        let body = convert_all(&pages, &probe);
        assert!(
            body.contains("[Target \\[Page](/pages/target-page.md)"),
            "{body}"
        );
        assert!(!body.contains("[Target [Page]("), "{body}");
    }

    #[test]
    fn refuses_to_overwrite_user_file_not_in_marker() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("pages")).unwrap();
        std::fs::write(
            dir.path().join(MARKER_FILE),
            serde_json::to_vec(&serde_json::json!({
                "okf_version": "0.2",
                "generated_by": "wenlan/test",
                "files": ["index.md"],
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(dir.path().join("pages/alpha.md"), b"user bytes").unwrap();
        let marker_before = std::fs::read(dir.path().join(MARKER_FILE)).unwrap();
        let pages = vec![test_page("concept_a", "Alpha", "x")];
        let err = export_okf(&pages, dir.path()).unwrap_err();
        assert!(
            err.to_string().contains(
                "refusing to overwrite pages/alpha.md: not written by a previous Wenlan OKF export"
            ),
            "{err}"
        );
        assert_eq!(
            std::fs::read(dir.path().join("pages/alpha.md")).unwrap(),
            b"user bytes"
        );
        assert_eq!(
            std::fs::read(dir.path().join(MARKER_FILE)).unwrap(),
            marker_before,
            "marker untouched"
        );
        assert!(!dir.path().join("index.md").exists(), "nothing written");
        assert!(!dir.path().join("sources").exists(), "nothing written");
    }

    #[test]
    fn foreign_marker_writer_is_a_conflict() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(MARKER_FILE),
            serde_json::to_vec(&serde_json::json!({
                "okf_version": "0.2",
                "generated_by": "someone-else",
                "files": [],
            }))
            .unwrap(),
        )
        .unwrap();
        let marker_before = std::fs::read(dir.path().join(MARKER_FILE)).unwrap();
        let pages = vec![test_page("concept_a", "Alpha", "x")];
        let err = export_okf(&pages, dir.path()).unwrap_err();
        assert!(matches!(err, WenlanError::Conflict(_)), "{err}");
        assert_eq!(
            std::fs::read(dir.path().join(MARKER_FILE)).unwrap(),
            marker_before,
            "marker untouched"
        );
        assert!(!dir.path().join("index.md").exists(), "nothing written");
        assert!(!dir.path().join("pages").exists(), "nothing written");
    }

    #[test]
    fn a_page_titled_index_is_exported_beside_the_index_not_as_one() {
        let dir = tempfile::TempDir::new().unwrap();
        // A bundle from 0.18.9 wrote such a page at `pages/index.md`.
        std::fs::create_dir_all(dir.path().join(PAGES_DIR)).unwrap();
        std::fs::write(dir.path().join("pages/index.md"), "old").unwrap();
        std::fs::write(
            dir.path().join(MARKER_FILE),
            serde_json::to_vec(&serde_json::json!({
                "okf_version": "0.2",
                "generated_by": "wenlan/test",
                "files": ["index.md", "pages/index.md"],
            }))
            .unwrap(),
        )
        .unwrap();
        let pages = vec![test_page("concept_index", "Index", "x")];
        let stats = export_okf(&pages, dir.path()).unwrap();
        assert_eq!(stats.exported, 1);
        assert!(
            !dir.path().join("pages/index.md").exists(),
            "stale file removed"
        );
        let page = std::fs::read_to_string(dir.path().join("pages/index-page.md")).unwrap();
        assert!(page.contains("title: \"Index\""), "{page}");
        let index = std::fs::read_to_string(dir.path().join("index.md")).unwrap();
        assert!(index.contains("[Index](/pages/index-page.md)"), "{index}");
    }

    #[test]
    fn index_lists_only_pages_actually_written() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(MARKER_FILE),
            serde_json::to_vec(&serde_json::json!({
                "okf_version": "0.2",
                "generated_by": "wenlan/test",
                "files": ["index.md", "pages/alpha.md"],
            }))
            .unwrap(),
        )
        .unwrap();
        // A directory where the bundle file should go makes that page's write fail.
        std::fs::create_dir_all(dir.path().join("pages/alpha.md")).unwrap();
        let pages = vec![
            test_page("concept_alpha", "Alpha", "x"),
            test_page("concept_beta", "Beta", "y"),
        ];
        let stats = export_okf(&pages, dir.path()).unwrap();
        assert_eq!(stats.exported, 1);
        assert_eq!(stats.failed, 1);
        let index = std::fs::read_to_string(dir.path().join("index.md")).unwrap();
        assert!(index.contains("/pages/beta.md"), "{index}");
        assert!(!index.contains("Alpha"), "{index}");
        assert!(!index.contains("alpha.md"), "{index}");
        // The final marker still lists every planned file for later cleanup.
        let marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join(MARKER_FILE)).unwrap()).unwrap();
        let files: HashSet<String> = marker["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(files.contains("pages/alpha.md"), "{files:?}");
        assert!(files.contains("pages/beta.md"), "{files:?}");
    }

    #[test]
    fn reexport_is_byte_stable() {
        let mut a = test_page("concept_a", "Alpha", "see [[Beta]]");
        a.source_memory_ids = vec!["mem_1".to_string()];
        let pages = vec![a, test_page("concept_b", "Beta", "plain")];
        let dir = tempfile::TempDir::new().unwrap();
        export_okf(&pages, dir.path()).unwrap();
        let first = snapshot_dir(dir.path());
        assert!(first.len() >= 5, "{first:?}");
        export_okf(&pages, dir.path()).unwrap();
        let second = snapshot_dir(dir.path());
        assert_eq!(first, second);
    }

    fn snapshot_dir(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        collect_all_files(root, &mut out);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn collect_all_files(dir: &Path, out: &mut Vec<(std::path::PathBuf, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                collect_all_files(&path, out);
            } else {
                out.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
    }

    fn collect_md(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_md(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                out.push(path);
            }
        }
    }

    fn outside_code_contains(content: &str, needle: &str) -> bool {
        let ranges = crate::sources::obsidian::code_block_ranges(content);
        let mut visible = String::new();
        let mut last = 0;
        for r in &ranges {
            visible.push_str(&content[last..r.start]);
            last = r.end;
        }
        visible.push_str(&content[last..]);
        visible.contains(needle)
    }

    fn marker_temps(root: &Path) -> Vec<String> {
        std::fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with(&format!("{MARKER_FILE}.")))
            .collect()
    }

    /// A kill between writing the temp and the rename must leave the previous
    /// marker byte-for-byte: it used to be truncated in place, and the next
    /// export refused at the malformed-marker parse.
    #[test]
    fn an_interrupted_marker_update_keeps_the_previous_marker_and_the_next_export_works() {
        let dir = tempfile::TempDir::new().unwrap();
        let both = vec![
            test_page("concept_a", "Alpha", "x"),
            test_page("concept_b", "Beta", "y"),
        ];
        export_okf(&both, dir.path()).unwrap();
        let marker_path = dir.path().join(MARKER_FILE);
        let before = std::fs::read(&marker_path).unwrap();

        let replacement =
            serde_json::to_vec_pretty(&marker_for(&HashSet::new(), BTreeMap::new())).unwrap();
        let temp_seen = std::cell::Cell::new(false);
        let result = write_marker_atomically_with_hook(dir.path(), &replacement, || {
            // The replacement is complete on disk beside the marker, which is
            // still the old one: the window the in-place write left torn.
            temp_seen.set(!marker_temps(dir.path()).is_empty());
            assert_eq!(std::fs::read(&marker_path).unwrap(), before);
            Err(std::io::Error::other("injected after temp write"))
        });
        assert!(result.is_err());
        assert!(temp_seen.get(), "hook ran after the temp was written");
        assert_eq!(std::fs::read(&marker_path).unwrap(), before);
        assert!(
            marker_temps(dir.path()).is_empty(),
            "failed temp cleaned up"
        );

        // The previous ownership record is intact, so the re-export still
        // knows `beta.md` is its own to remove.
        let stats = export_okf(&both[..1], dir.path()).unwrap();
        assert_eq!(stats.failed, 0, "{stats:?}");
        assert!(!dir.path().join("pages/beta.md").exists());
        let marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(marker_path).unwrap()).unwrap();
        assert!(marker["digests"]["pages/alpha.md"].is_string());
        assert!(marker["digests"].get("pages/beta.md").is_none());
    }

    /// A process killed mid-update leaves its temp behind (a half-written one
    /// here). It is not the marker, so it blocks nothing, and the next export
    /// keeps the ownership and digests of the marker still in place.
    #[test]
    fn a_stale_marker_temp_neither_blocks_nor_corrupts_the_next_export() {
        let dir = tempfile::TempDir::new().unwrap();
        let pages = vec![test_page("concept_a", "Alpha", "x")];
        export_okf(&pages, dir.path()).unwrap();
        let before = std::fs::read(dir.path().join(MARKER_FILE)).unwrap();

        let stale = dir.path().join(format!("{MARKER_FILE}.deadbeef.tmp"));
        std::fs::write(&stale, b"{\"okf_version\": \"0.2\", \"fil").unwrap();
        std::fs::create_dir(dir.path().join(format!("{MARKER_FILE}.dir.tmp"))).unwrap();

        let stats = export_okf(&pages, dir.path()).unwrap();
        assert_eq!(stats.failed, 0, "{stats:?}");
        assert_eq!(std::fs::read(dir.path().join(MARKER_FILE)).unwrap(), before);
        assert!(!stale.exists(), "stale temp swept");
        assert!(
            dir.path().join(format!("{MARKER_FILE}.dir.tmp")).is_dir(),
            "only plain files are swept"
        );
        assert!(
            !dir.path().join(ARCHIVE_DIR).exists(),
            "no page was mistaken for an edit"
        );
    }

    #[test]
    fn the_marker_is_swapped_in_as_a_new_file_not_truncated_in_place() {
        let dir = tempfile::TempDir::new().unwrap();
        write_marker_atomically(dir.path(), b"one").unwrap();
        let marker_path = dir.path().join(MARKER_FILE);
        let first = std::fs::read(&marker_path).unwrap();
        assert_eq!(first, b"one");
        #[cfg(unix)]
        let inode = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&marker_path).unwrap().ino()
        };
        write_marker_atomically(dir.path(), b"two").unwrap();
        assert_eq!(std::fs::read(&marker_path).unwrap(), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_ne!(std::fs::metadata(&marker_path).unwrap().ino(), inode);
        }
        assert!(marker_temps(dir.path()).is_empty());
    }

    /// The marker is not exempt from the symlink rule the atomic swap must not
    /// weaken: a link standing at the marker is refused, not replaced, and
    /// nothing is written through it.
    #[test]
    #[cfg(unix)]
    fn a_symlinked_marker_is_refused_and_left_alone() {
        let dir = tempfile::TempDir::new().unwrap();
        let pages = vec![test_page("concept_a", "Alpha", "x")];
        export_okf(&pages, dir.path()).unwrap();

        let elsewhere = tempfile::TempDir::new().unwrap();
        let real = elsewhere.path().join("marker.json");
        std::fs::rename(dir.path().join(MARKER_FILE), &real).unwrap();
        let before = std::fs::read(&real).unwrap();
        let link = dir.path().join(MARKER_FILE);
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(export_okf(&pages, dir.path()).is_err());
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::read(&real).unwrap(), before);
        assert!(marker_temps(dir.path()).is_empty());

        // A dangling link is refused too, and nothing appears at its target.
        std::fs::remove_file(&link).unwrap();
        let missing = elsewhere.path().join("missing.json");
        std::os::unix::fs::symlink(&missing, &link).unwrap();
        assert!(write_marker_atomically(dir.path(), b"{}").is_err());
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert!(!missing.exists());
        assert!(marker_temps(dir.path()).is_empty());
    }
}
