// SPDX-License-Identifier: Apache-2.0
//! Provenance projection helpers: delimiter-owned Sources block, canonical
//! storage validation, and `_sources/` stub projection + GC.

use crate::error::WenlanError;
use crate::export::vault_fs::{self, Disposal, Replacement};
use wenlan_types::CodeBasisLocator;

/// Opening delimiter for the export-only `## Sources` block. The block is
/// generated from DB truth at projection time and stripped at ingress; it is
/// NEVER part of canonical `Page.content`.
pub const SOURCES_BLOCK_START: &str = "<!-- origin:sources:start -->";
/// Closing delimiter for the export-only Sources block.
pub const SOURCES_BLOCK_END: &str = "<!-- origin:sources:end -->";

/// Strip ONLY the delimiter-owned Sources block from a page body. A user may
/// legitimately type a `## Sources` heading or a `[[mem_123]]` wikilink in
/// prose; neither is touched. Removes the first `START..END` span (inclusive
/// of both delimiters and any trailing newline before START) and returns the
/// remainder trimmed of trailing whitespace. If the delimiters are absent or
/// malformed (END before START, or START with no END), the body is returned
/// trimmed but otherwise untouched.
pub fn canonicalize_page_body(body: &str) -> String {
    let start = match body.find(SOURCES_BLOCK_START) {
        Some(i) => i,
        None => return body.trim_end().to_string(),
    };
    let after_start = start + SOURCES_BLOCK_START.len();
    let end_rel = match body[after_start..].find(SOURCES_BLOCK_END) {
        Some(i) => i,
        None => return body.trim_end().to_string(),
    };
    let end = after_start + end_rel + SOURCES_BLOCK_END.len();
    // Drop whitespace/newlines immediately preceding the block so a fresh
    // projection (body + "\n\n" + block) canonicalizes back to bare body.
    let head = body[..start].trim_end();
    let tail = body[end..].trim_start();
    let mut out = String::from(head);
    if !tail.is_empty() {
        out.push_str("\n\n");
        out.push_str(tail);
    }
    out.trim_end().to_string()
}

/// Validate CLIENT-supplied canonical Page source without rewriting it.
///
/// The exporter owns both Sources delimiters. Persisting either token would
/// make vault projection decoding ambiguous, so canonical ingress rejects the
/// whole write instead of silently deleting user bytes. Delimiter-free source
/// is returned as the same borrowed string so every later storage consumer
/// sees the exact representation the caller supplied.
pub fn validate_canonical_page_content(content: &str) -> Result<&str, WenlanError> {
    if content.contains(SOURCES_BLOCK_START) || content.contains(SOURCES_BLOCK_END) {
        return Err(WenlanError::Validation(
            "Page content contains a daemon-reserved Sources delimiter".to_string(),
        ));
    }
    Ok(content)
}

/// Compatibility projection decoder retained for downstream callers that used
/// the pre-0.14.1 ingress helper directly.
///
/// Canonical storage paths must use [`validate_canonical_page_content`].
/// This function intentionally preserves its historical lossy behavior for
/// callers decoding daemon-owned projection text.
pub fn sanitize_ingress_content(content: &str) -> String {
    let stripped = canonicalize_page_body(content);
    let cleaned = stripped
        .replace(SOURCES_BLOCK_START, "")
        .replace(SOURCES_BLOCK_END, "");
    cleaned.trim_end().to_string()
}

/// Encode `s` as a YAML-safe double-quoted scalar. A JSON string literal is
/// a valid YAML flow scalar, so serde_json handles quotes/backslashes/control
/// chars correctly. Falls back to a naive quote only if JSON encoding fails
/// (it never does for a String).
pub(crate) fn yaml_quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{}\"", s.replace('"', "'")))
}

/// Render the export-only `## Sources` block from a page's cited memory ids.
/// Returns the empty string when there are no sources (source-less pages get
/// no block). The block is wrapped in the delimiters so the ingress
/// canonicalizer can strip it exactly.
pub fn render_sources_block(source_memory_ids: &[String]) -> String {
    if source_memory_ids.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(SOURCES_BLOCK_START);
    out.push_str("\n## Sources\n");
    for id in source_memory_ids {
        out.push_str(&format!("- [[{id}]]\n"));
    }
    out.push_str(SOURCES_BLOCK_END);
    out.push('\n');
    out
}

/// Render the read-only `sources:` frontmatter block as an OKF v0.2 list.
/// Each cited memory is an `{id, resource}` object; each code basis the note
/// relies on follows as a resource-only `{resource: "repo://<path>#L<a>-L<b>"}`
/// entry, sorted by path then line. The repository root is never written: a
/// reader of the export gets the in-repo path, not where it lives on this
/// machine. Empty string when there is nothing to list.
///
/// Only the OKF bundle export passes code bases. The `.md` projection passes
/// `&[]`: a note's `page_code_basis` rows change outside page writes (create
/// stores them after the projection, confirm, remove and recheck move them
/// without bumping the version), so a projected `repo://` line would go stale
/// and the file would read as hand-edited.
/// PROJECTION-OUT ONLY — the watcher never reads this back.
pub fn sources_frontmatter(
    source_memory_ids: &[String],
    code_bases: &[CodeBasisLocator],
) -> String {
    if source_memory_ids.is_empty() && code_bases.is_empty() {
        return String::new();
    }
    let mut out = String::from("sources:\n");
    for id in source_memory_ids {
        out.push_str(&format!(
            "  - {{id: {}, resource: {}}}\n",
            yaml_quoted(id),
            yaml_quoted(&format!("wenlan://memory/{id}"))
        ));
    }
    let mut code: Vec<&CodeBasisLocator> = code_bases.iter().collect();
    code.sort_by(|a, b| {
        (&a.path, a.line_start, a.line_end).cmp(&(&b.path, b.line_start, b.line_end))
    });
    for basis in code {
        out.push_str(&format!(
            "  - {{resource: {}}}\n",
            yaml_quoted(&format!(
                "repo://{}#L{}-L{}",
                basis.path, basis.line_start, basis.line_end
            ))
        ));
    }
    out
}

/// Render the read-only `related:` frontmatter line from page→page wikilink
/// targets. Empty string when there are none.
pub fn related_frontmatter(related_titles: &[String]) -> String {
    if related_titles.is_empty() {
        return String::new();
    }
    let quoted: Vec<String> = related_titles
        .iter()
        .map(|t| yaml_quoted(&format!("[[{t}]]")))
        .collect();
    format!("related: [{}]\n", quoted.join(", "))
}

use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _};
use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Read as _, Write as _};
use std::path::Path;

/// Subdir under `knowledge_path` for read-only source stubs. The page_watcher
/// scans only the top-level `.md` files, so this subdir is never synced back.
pub const SOURCES_STUB_DIR: &str = "_sources";

/// Bounded read for classifying one `_sources/` candidate. A stub is a few
/// hundred bytes; anything past this is not one, and a file the pass cannot
/// read whole is one it keeps.
const STUB_CLASSIFY_SCAN_BYTES: u64 = 1024 * 1024;

/// Maps page_id → the memory ids that page currently cites. Persisted at
/// `_sources/.manifest.json` so GC knows which stubs are still referenced
/// across daemon restarts. GC reaps by the `origin_stub:` marker (see
/// `gc_orphan_stubs`), so daemon stubs of any id shape (`mem_*`, `import_*`)
/// are reaped when orphaned while unmarked user notes are spared.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StubManifest {
    pages: HashMap<String, Vec<String>>,
}

impl StubManifest {
    pub fn record(&mut self, page_id: &str, source_memory_ids: &[String]) {
        if source_memory_ids.is_empty() {
            self.pages.remove(page_id);
        } else {
            self.pages
                .insert(page_id.to_string(), source_memory_ids.to_vec());
        }
    }

    pub fn forget_page(&mut self, page_id: &str) {
        self.pages.remove(page_id);
    }

    fn cited_ids(&self) -> HashSet<String> {
        self.pages.values().flatten().cloned().collect()
    }

    pub fn load(knowledge_path: &Path) -> Self {
        let p = knowledge_path.join(SOURCES_STUB_DIR).join(".manifest.json");
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|d| serde_json::from_str(&d).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, knowledge_path: &Path) -> std::io::Result<()> {
        let dir = knowledge_path.join(SOURCES_STUB_DIR);
        std::fs::create_dir_all(&dir)?;
        let data = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        std::fs::write(dir.join(".manifest.json"), data)
    }

    pub(crate) fn load_from(root: &Dir) -> Self {
        let Ok(dir) = open_or_create_sources_dir(root, false) else {
            return Self::default();
        };
        let Some(dir) = dir else {
            return Self::default();
        };
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let mut file = match dir.open_with(".manifest.json", &options) {
            Ok(file) => file,
            Err(_) => return Self::default(),
        };
        let mut data = String::new();
        if (&mut file)
            .take(4 * 1024 * 1024 + 1)
            .read_to_string(&mut data)
            .is_err()
            || data.len() > 4 * 1024 * 1024
        {
            return Self::default();
        }
        serde_json::from_str(&data).unwrap_or_default()
    }

    pub(crate) fn save_to(&self, root: &Dir) -> std::io::Result<()> {
        let dir = open_or_create_sources_dir(root, true)?
            .expect("create=true always returns the sources directory");
        let data = serde_json::to_vec_pretty(self).unwrap_or_else(|_| b"{}".to_vec());
        write_regular_nofollow(&dir, ".manifest.json", &data)
    }
}

/// The exact bytes this module writes for one memory's source stub.
///
/// One source of truth, because the GC below compares a candidate file against
/// it to decide whether the file is still Wenlan's to remove.
fn stub_body(id: &str) -> String {
    let quoted = yaml_quoted(id);
    format!(
        "---\ntype: source\ntitle: {quoted}\norigin_stub: {quoted}\n---\n\n\
         This is a read-only source projection for memory `{id}`. \
         Edit the memory in Wenlan, not this file.\n"
    )
}

/// The `origin_stub` value in `content`'s leading YAML frontmatter, if it has
/// one.
///
/// A KEY, not a substring. This used to be `content.contains("origin_stub:")`,
/// which matched the text ANYWHERE in the file, so a note of the user's own
/// that happened to quote the marker — in a code fence, in a sentence about
/// how the projection works — read as a daemon stub and was unlinked. The
/// marker only means "Wenlan wrote this" where [`stub_body`] puts it, which is
/// the frontmatter block at the top of the file.
fn frontmatter_origin_stub(content: &str) -> Option<String> {
    // An editor that normalizes line endings or adds a byte-order mark must
    // not make a stub Wenlan wrote unrecognizable: a stub that reads as "not
    // ours" is never reaped and never archived, so it leaks forever.
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---")?;
    for line in rest[..end].lines() {
        // A nested key is indented; ours is at the top level of the block.
        let Some(raw) = line.strip_prefix("origin_stub:") else {
            continue;
        };
        let raw = raw.trim();
        return Some(
            serde_json::from_str::<String>(raw)
                .unwrap_or_else(|_| raw.trim_matches('"').to_string()),
        );
    }
    None
}

/// What a GC pass may do with one uncited `.md` file under `_sources/`.
#[derive(Debug, PartialEq, Eq)]
enum StubDisposition {
    /// No `origin_stub` frontmatter key: not Wenlan's file at all. Left alone.
    NotOurs,
    /// Byte-identical to what [`stub_body`] writes today AND sitting at the
    /// filename that stub would have. Regenerable from the database, so
    /// removing it loses nothing.
    Pristine,
    /// Carries the marker but the bytes have changed. Wenlan wrote the file;
    /// somebody has since typed into it. Archived, never unlinked.
    Edited,
}

/// `filename` guards the one case bytes alone cannot: `cp _sources/mem_1.md
/// _sources/my-copy.md` leaves a file whose bytes are a pristine stub at a
/// name Wenlan would never write. That copy is the user's, so it is archived
/// rather than unlinked.
fn classify_stub(content: &str, filename: &str) -> StubDisposition {
    match frontmatter_origin_stub(content) {
        None => StubDisposition::NotOurs,
        Some(id) if stub_body(&id) == content && stub_filename(&id) == filename => {
            StubDisposition::Pristine
        }
        Some(_) => StubDisposition::Edited,
    }
}

/// Clears orphan source stubs: `.md` files under `_sources/` that no page in
/// `manifest` cites any more.
///
/// Scope is the `origin_stub` frontmatter KEY, not the filename and not a
/// substring of the body. A file is reaped iff it carries that key (so Wenlan
/// wrote it) AND its filename is not in the cited set. A user note under
/// `_sources/`, of any name including `mem_*.md`, and whatever its body says,
/// is never touched. Keying on the marker rather than a name prefix also
/// reaches `import_*` stubs, closing the leak the old name-prefix scope had.
///
/// An orphan Wenlan still recognizes byte for byte is removed; one that has
/// been edited since is moved to `archive/` instead. The marker proves
/// authorship, never that the bytes are still Wenlan's.
///
/// `knowledge_path` is the projection root, so the archive lands beside the
/// archived pages rather than inside `_sources/`.
pub fn gc_orphan_stubs(knowledge_path: &Path, manifest: &StubManifest) -> std::io::Result<()> {
    if !knowledge_path.join(SOURCES_STUB_DIR).exists() {
        return Ok(());
    }
    let root = Dir::open_ambient_dir(knowledge_path, cap_std::ambient_authority())?;
    gc_orphan_stubs_in(&root, manifest)
}

pub(crate) fn gc_orphan_stubs_in(root: &Dir, manifest: &StubManifest) -> std::io::Result<()> {
    gc_orphan_stubs_in_with_hooks(root, manifest, &mut |_| {}, &mut |_| {})
}

/// [`gc_orphan_stubs_in`] with two seams for the interleavings an editor can
/// produce. `after_classify` runs once a stub has been classified and before
/// the pass acts on it, with the stub's name. `after_aside` runs once a
/// classified-pristine stub has been taken aside and before its bytes are
/// re-read, with the private name it now has. Production passes no-ops.
fn gc_orphan_stubs_in_with_hooks(
    root: &Dir,
    manifest: &StubManifest,
    after_classify: &mut dyn FnMut(&str),
    after_aside: &mut dyn FnMut(&str),
) -> std::io::Result<()> {
    let Some(dir) = open_or_create_sources_dir(root, false)? else {
        return Ok(());
    };
    let cited: HashSet<String> = manifest
        .cited_ids()
        .iter()
        .map(|id| stub_filename(id))
        .collect();
    for entry in dir.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name_text) = name.to_str() else {
            continue;
        };
        // Non-`.md` files (the `.manifest.json` projection index) and still
        // cited stubs are not candidates.
        if !name_text.ends_with(".md") || cited.contains(name_text) {
            continue;
        }
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let mut file = match dir.open_with(Path::new(&name), &options) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let mut content = String::new();
        // A file this pass cannot read whole is a file it cannot classify, and
        // an unclassified file is kept.
        let readable = (&mut file)
            .take(STUB_CLASSIFY_SCAN_BYTES + 1)
            .read_to_string(&mut content)
            .is_ok()
            && content.len() as u64 <= STUB_CLASSIFY_SCAN_BYTES;
        drop(file);
        if !readable {
            continue;
        }
        match classify_stub(&content, name_text) {
            StubDisposition::NotOurs => {}
            StubDisposition::Pristine => {
                after_classify(name_text);
                // The read above decided about the bytes that were there THEN.
                // An editor can save between that read and an unlink by name,
                // and the unlink would take the save with it. So the file
                // moves to a private name first, which captures whatever is at
                // the name now, and only bytes that still classify as pristine
                // are unlinked.
                let outcome = vault_fs::remove_if_unchanged_with_hook(
                    &dir,
                    name_text,
                    STUB_CLASSIFY_SCAN_BYTES,
                    |bytes| {
                        std::str::from_utf8(bytes).is_ok_and(|current| {
                            classify_stub(current, name_text) == StubDisposition::Pristine
                        })
                    },
                    |aside| after_aside(aside),
                );
                match outcome {
                    Ok(Disposal::Absent | Disposal::Removed) => {}
                    // Edited after it was classified, and put back. The next
                    // pass sees an edited stub and archives it.
                    Ok(Disposal::Kept) => {
                        log::info!(
                            "[provenance] kept {name_text}: edited while it was being reaped"
                        );
                    }
                    // Edited, and a newer file already holds the name, so the
                    // edit is still under its private name. Archive that.
                    Ok(Disposal::Displaced(aside)) => {
                        if let Err(e) = crate::export::archive::archive_aside_from(
                            root, &dir, &aside, name_text,
                        ) {
                            log::warn!(
                                "[provenance] could not archive edited stub {name_text} \
                                 (left as {aside} in {SOURCES_STUB_DIR}): {e}"
                            );
                        }
                    }
                    Err(e) => {
                        log::warn!("[provenance] could not reap orphan stub {name_text}: {e}");
                    }
                }
            }
            StubDisposition::Edited => {
                after_classify(name_text);
                if let Err(e) = crate::export::archive::archive_file_from(root, &dir, name_text) {
                    log::warn!("[provenance] could not archive edited stub {name_text}: {e}");
                }
            }
        }
    }
    Ok(())
}

/// Map a memory id to a filesystem/wikilink-safe token. Ids already matching
/// `[A-Za-z0-9_-]+` (all stored Wenlan `mem_<uuid>` ids) pass through
/// unchanged. Other chars are hex-escaped as `_XX`.
/// NOTE: not collision-free for arbitrary input — because `_` is both the
/// escape introducer and a passthrough char, `"mem_a/"` and a literal
/// `"mem_a_2f"` both map to `"mem_a_2f"`. Safe for P1 (cited ids are always
/// UUID-shaped `mem_*`). When imported ids (which may carry `/`, spaces, etc.)
/// can become page sources in P2, make this injective then — e.g. append a
/// deterministic hash for ids failing the safe-charset check — with a
/// collision regression test. Tracked as a P2 follow-up.
pub fn sanitize_stub_id(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for c in id.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else {
            out.push('_');
            for b in c.to_string().as_bytes() {
                out.push_str(&format!("{b:02x}"));
            }
        }
    }
    out
}

/// Stub filename for a memory id, e.g. `mem_1.md`.
pub fn stub_filename(id: &str) -> String {
    format!("{}.md", sanitize_stub_id(id))
}

/// Project a read-only stub note for each cited memory id under
/// `<knowledge_path>/_sources/`. Idempotent: rewrites the stub each call,
/// except where somebody has typed in it (see [`project_stubs_for_page_in`],
/// which this delegates to so there is one implementation to keep honest).
pub fn project_stubs_for_page(
    knowledge_path: &Path,
    page_id: &str,
    source_memory_ids: &[String],
) -> std::io::Result<()> {
    if source_memory_ids.is_empty() {
        return Ok(());
    }
    // `create_dir_all` first: the old body created a missing projection root on
    // the way to `_sources/`, and opening a directory does not.
    std::fs::create_dir_all(knowledge_path)?;
    let root = Dir::open_ambient_dir(knowledge_path, cap_std::ambient_authority())?;
    project_stubs_for_page_in(&root, page_id, source_memory_ids)
}

pub(crate) fn project_stubs_for_page_in(
    root: &Dir,
    page_id: &str,
    source_memory_ids: &[String],
) -> std::io::Result<()> {
    project_stubs_for_page_in_with_hook(root, page_id, source_memory_ids, &mut |_| {})
}

/// [`project_stubs_for_page_in`] with a seam between classifying a stub (and
/// archiving it, when it was edited) and writing it. `before_write` receives
/// the stub's name. Production passes a no-op.
fn project_stubs_for_page_in_with_hook(
    root: &Dir,
    _page_id: &str,
    source_memory_ids: &[String],
    before_write: &mut dyn FnMut(&str),
) -> std::io::Result<()> {
    if source_memory_ids.is_empty() {
        return Ok(());
    }
    let dir = open_or_create_sources_dir(root, true)?
        .expect("create=true always returns the sources directory");
    for id in source_memory_ids {
        let body = stub_body(id);
        let name = stub_filename(id);
        // A stub is a generated, read-only projection of a memory, but the
        // file sits in somebody's vault and they can type into it. This used
        // to truncate it on every write of every page that cites the memory,
        // so an edit survived only until the next refresh. The GC below
        // protects a stub that has gone ORPHAN; nothing protected one that is
        // still cited, which is the common case.
        match stub_target(&dir, &name, body.as_bytes()) {
            StubTarget::Writable => {}
            StubTarget::Edited => {
                if let Err(e) = crate::export::archive::archive_file_from(root, &dir, &name) {
                    // Same call as the orphan GC makes. If the bytes cannot be
                    // moved aside they are not overwritten either: skip the
                    // stub and leave the projection one file behind rather
                    // than destroy an edit.
                    log::warn!("[provenance] could not archive edited stub {name}: {e}");
                    continue;
                }
            }
            StubTarget::Foreign => {
                log::warn!("[provenance] leaving {name} alone: not a plain file");
                continue;
            }
        }
        before_write(&name);
        // The classification above is a read of the bytes that were there
        // then. Truncating the name afterwards would destroy a save that
        // landed in between, so the stub is installed without replacing
        // anything but the exact bytes this pass classified as its own.
        match vault_fs::replace_if_unchanged(
            &dir,
            &name,
            body.as_bytes(),
            STUB_CLASSIFY_SCAN_BYTES,
            |bytes| bytes == body.as_bytes(),
        )? {
            Replacement::Written | Replacement::Unchanged => {}
            // Somebody typed into it after it was classified. Left where it
            // is; the next write of a page citing it archives the edit and
            // rewrites the stub.
            Replacement::Kept => {
                log::warn!("[provenance] leaving {name} alone: it changed while it was written");
            }
            Replacement::Displaced(aside) => {
                if let Err(e) =
                    crate::export::archive::archive_aside_from(root, &dir, &aside, &name)
                {
                    log::warn!(
                        "[provenance] could not archive edited stub {name} \
                         (left as {aside} in {SOURCES_STUB_DIR}): {e}"
                    );
                }
            }
        }
    }
    Ok(())
}

/// What the stub projection may do with whatever is already at `name`.
#[derive(Debug, PartialEq, Eq)]
enum StubTarget {
    /// Nothing there, or the bytes are already the ones this pass would write.
    /// Ours to write.
    Writable,
    /// Present, with bytes that are not the ones this pass would write.
    /// Somebody typed in it: archived before it is replaced.
    Edited,
    /// Present, but not something this pass may claim: not a plain file, or a
    /// plain file it could not read whole. Left exactly as it is.
    Foreign,
}

/// Classify the stub file at `name` by comparing it with the bytes this pass
/// would write. No marker can answer this: a stub's frontmatter is identical
/// whether or not somebody edited the body, which is the whole lesson of this
/// arc. The bytes are the only evidence.
fn stub_target(dir: &Dir, name: &str, planned: &[u8]) -> StubTarget {
    let metadata = match dir.symlink_metadata(Path::new(name)) {
        Ok(metadata) => metadata,
        // Only "nothing there" makes the name ours. Any other stat failure
        // (a directory this process cannot search, an I/O error) says nothing
        // about what is at the name, and the write below truncates whatever
        // is, so it is not attempted.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return StubTarget::Writable,
        Err(_) => return StubTarget::Foreign,
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return StubTarget::Foreign;
    }
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let Ok(mut file) = dir.open_with(Path::new(name), &options) else {
        // Present but unreadable: a file this pass cannot read is a file it
        // cannot claim, so it is not archived either. Archiving renames, which
        // needs no read permission, so classifying this as `Edited` would let
        // the pass take over a name whose bytes it never saw. The orphan GC
        // keeps such a file; so does this.
        return StubTarget::Foreign;
    };
    let mut content = Vec::new();
    let readable = (&mut file)
        .take(STUB_CLASSIFY_SCAN_BYTES + 1)
        .read_to_end(&mut content)
        .is_ok()
        && content.len() as u64 <= STUB_CLASSIFY_SCAN_BYTES;
    drop(file);
    if !readable {
        return StubTarget::Foreign;
    }
    if content == planned {
        StubTarget::Writable
    } else {
        StubTarget::Edited
    }
}

fn open_or_create_sources_dir(root: &Dir, create: bool) -> std::io::Result<Option<Dir>> {
    match root.symlink_metadata(SOURCES_STUB_DIR) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            root.open_dir_nofollow(SOURCES_STUB_DIR).map(Some)
        }
        Ok(_) => Err(std::io::Error::other(
            "source projection directory is not a plain directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
            root.create_dir(SOURCES_STUB_DIR)?;
            root.open_dir_nofollow(SOURCES_STUB_DIR).map(Some)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_regular_nofollow(directory: &Dir, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create(true)
        .truncate(true)
        .follow(FollowSymlinks::No);
    let mut file = directory.open_with(name, &options)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_only_the_delimiter_block() {
        let body = format!(
            "## Overview\nReal prose here.\n\n{SOURCES_BLOCK_START}\n## Sources\n- [[mem_1]]\n{SOURCES_BLOCK_END}\n"
        );
        let canon = canonicalize_page_body(&body);
        assert_eq!(canon, "## Overview\nReal prose here.");
        assert!(!canon.contains(SOURCES_BLOCK_START));
        assert!(!canon.contains("[[mem_1]]"));
    }

    #[test]
    fn user_typed_mem_wikilink_in_prose_survives() {
        // No delimiter block at all — a bare `[[mem_123]]` the user wrote.
        let body = "I cited [[mem_123]] in my own note.\n\n## Sources\nhand-written, not the daemon block.";
        let canon = canonicalize_page_body(body);
        assert_eq!(canon, body.trim_end());
        assert!(canon.contains("[[mem_123]]"));
        assert!(canon.contains("## Sources"));
    }

    #[test]
    fn missing_end_delimiter_leaves_body_untouched() {
        let body = format!("prose\n\n{SOURCES_BLOCK_START}\n## Sources\n- [[mem_1]]\n");
        let canon = canonicalize_page_body(&body);
        // No END → no strip; only trailing whitespace trimmed.
        assert!(canon.contains(SOURCES_BLOCK_START));
        assert!(canon.contains("[[mem_1]]"));
    }

    #[test]
    fn preserves_prose_after_the_block() {
        let body =
            format!("head prose\n\n{SOURCES_BLOCK_START}\nx\n{SOURCES_BLOCK_END}\n\ntail prose");
        let canon = canonicalize_page_body(&body);
        assert_eq!(canon, "head prose\n\ntail prose");
    }

    #[test]
    fn canonical_content_validation_preserves_delimiter_free_source_exactly() {
        let content = "\u{feff}\r\n  ## Overview  \r\nNormal prose.\t \r\n\r\n";
        assert_ne!(
            content.trim_end(),
            content,
            "positive control: trimming must change this fixture"
        );
        let validated = validate_canonical_page_content(content).unwrap();
        assert_eq!(validated, content);
        assert_eq!(validated.as_ptr(), content.as_ptr());
    }

    #[test]
    fn canonical_content_validation_rejects_reserved_delimiters_without_rewriting() {
        let cases = [
            format!("before {SOURCES_BLOCK_START} after"),
            format!("before {SOURCES_BLOCK_END} after"),
            format!("{SOURCES_BLOCK_START}\nowned\n{SOURCES_BLOCK_END}"),
            format!("```md\n{SOURCES_BLOCK_START}\n```\nkept prose"),
        ];
        for content in cases {
            let error = validate_canonical_page_content(&content).unwrap_err();
            assert!(matches!(error, WenlanError::Validation(_)));
            assert!(
                content.contains("before")
                    || content.contains("owned")
                    || content.contains("kept prose")
            );
        }
    }

    #[test]
    fn compatibility_ingress_sanitizer_keeps_its_projection_decoding_behavior() {
        let content = format!(
            "head\n\n{SOURCES_BLOCK_START}\n## Sources\n- [[mem_1]]\n{SOURCES_BLOCK_END}\n\ntail"
        );
        assert_eq!(sanitize_ingress_content(&content), "head\n\ntail");
    }

    #[test]
    fn render_sources_block_is_delimiter_wrapped_and_canonicalizes_to_empty() {
        let ids = ["mem_1".to_string(), "mem_2".to_string()];
        let block = render_sources_block(&ids);
        assert!(block.starts_with(SOURCES_BLOCK_START));
        assert!(block.trim_end().ends_with(SOURCES_BLOCK_END));
        assert!(block.contains("## Sources"));
        assert!(block.contains("[[mem_1]]"));
        assert!(block.contains("[[mem_2]]"));
        // A body that is exactly the block canonicalizes to empty.
        assert_eq!(canonicalize_page_body(&block), "");
    }

    #[test]
    fn render_sources_block_empty_for_no_sources() {
        let ids: [String; 0] = [];
        assert_eq!(render_sources_block(&ids), String::new());
    }

    #[test]
    fn sources_frontmatter_emits_okf_object_list() {
        let ids = ["mem_1".to_string(), "mem_2".to_string()];
        let fm = sources_frontmatter(&ids, &[]);
        assert_eq!(
            fm,
            "sources:\n  - {id: \"mem_1\", resource: \"wenlan://memory/mem_1\"}\n  - {id: \"mem_2\", resource: \"wenlan://memory/mem_2\"}\n"
        );
        // Must parse as valid YAML — a flow mapping per list item.
        let yaml = format!("title: x\n{fm}");
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&yaml).expect("OKF sources block must be valid YAML");
        let sources = parsed
            .get("sources")
            .and_then(|v| v.as_sequence())
            .expect("sources seq");
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].get("id").and_then(|v| v.as_str()), Some("mem_1"));
        assert_eq!(
            sources[0].get("resource").and_then(|v| v.as_str()),
            Some("wenlan://memory/mem_1")
        );
    }

    /// Review finding 1: an id with YAML-hostile characters (colon, hash,
    /// space) must not break the flow mapping or leak into `resource` —
    /// `id` needs the same `yaml_quoted` treatment as `resource`.
    #[test]
    fn sources_frontmatter_quotes_odd_character_ids() {
        let ids = ["mem_a: b #c".to_string()];
        let fm = sources_frontmatter(&ids, &[]);
        let yaml = format!("title: x\n{fm}");
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&yaml).expect("odd-character id must still parse as YAML");
        let sources = parsed
            .get("sources")
            .and_then(|v| v.as_sequence())
            .expect("sources seq");
        assert_eq!(sources.len(), 1);
        assert_eq!(
            sources[0].get("id").and_then(|v| v.as_str()),
            Some("mem_a: b #c"),
            "id must round-trip exactly through YAML"
        );
        assert_eq!(
            sources[0].get("resource").and_then(|v| v.as_str()),
            Some("wenlan://memory/mem_a: b #c")
        );
    }

    #[test]
    fn sources_frontmatter_empty_emits_nothing() {
        let ids: [String; 0] = [];
        assert_eq!(sources_frontmatter(&ids, &[]), String::new());
    }

    fn code_basis(path: &str, start: u32, end: u32) -> CodeBasisLocator {
        CodeBasisLocator {
            repo_root: "/home/someone/private-checkout".to_string(),
            path: path.to_string(),
            line_start: start,
            line_end: end,
        }
    }

    #[test]
    fn sources_frontmatter_lists_code_bases_as_resource_only_repo_entries() {
        let code = [code_basis("src/lib.rs", 10, 20)];
        // Code only: the `sources:` header is still emitted.
        let fm = sources_frontmatter(&[], &code);
        assert_eq!(
            fm,
            "sources:\n  - {resource: \"repo://src/lib.rs#L10-L20\"}\n"
        );
        // After the memory entries, never before.
        let ids = ["mem_1".to_string()];
        let both = sources_frontmatter(&ids, &code);
        assert_eq!(
            both,
            "sources:\n  - {id: \"mem_1\", resource: \"wenlan://memory/mem_1\"}\n  - {resource: \"repo://src/lib.rs#L10-L20\"}\n"
        );
        // The repository root is a fact about one machine; it is never written.
        assert!(!both.contains("private-checkout"));
        let parsed: serde_yaml::Value = serde_yaml::from_str(&format!("title: x\n{both}"))
            .expect("a resource-only entry must be valid YAML");
        let sources = parsed.get("sources").and_then(|v| v.as_sequence()).unwrap();
        assert_eq!(sources.len(), 2);
        assert!(sources[1].get("id").is_none());
        assert_eq!(
            sources[1].get("resource").and_then(|v| v.as_str()),
            Some("repo://src/lib.rs#L10-L20")
        );
    }

    #[test]
    fn sources_frontmatter_sorts_code_bases_by_path_then_line() {
        let code = [
            code_basis("src/b.rs", 1, 2),
            code_basis("src/a.rs", 30, 40),
            code_basis("src/a.rs", 5, 9),
        ];
        let fm = sources_frontmatter(&[], &code);
        let resources: Vec<&str> = fm.lines().skip(1).collect();
        assert_eq!(
            resources,
            [
                "  - {resource: \"repo://src/a.rs#L5-L9\"}",
                "  - {resource: \"repo://src/a.rs#L30-L40\"}",
                "  - {resource: \"repo://src/b.rs#L1-L2\"}",
            ]
        );
    }

    #[test]
    fn related_frontmatter_quotes_page_titles() {
        let titles = ["Other Page".to_string()];
        let fm = related_frontmatter(&titles);
        assert_eq!(fm, "related: [\"[[Other Page]]\"]\n");
    }

    #[test]
    fn related_frontmatter_escapes_titles_to_valid_yaml() {
        let titles = ["My \"Quoted\" Page".to_string()];
        let fm = related_frontmatter(&titles);
        // The emitted frontmatter block must parse as valid YAML (no map collapse).
        let yaml = format!("title: x\n{fm}");
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml)
            .expect("frontmatter with a quote-bearing title must be valid YAML");
        // And the related entry must round-trip to the original wikilink target.
        let related = parsed
            .get("related")
            .and_then(|v| v.as_sequence())
            .expect("related seq");
        assert_eq!(related[0].as_str().unwrap(), "[[My \"Quoted\" Page]]");
    }

    #[test]
    fn sanitize_stub_id_passes_safe_mem_ids() {
        assert_eq!(sanitize_stub_id("mem_abc123"), "mem_abc123");
        assert_eq!(sanitize_stub_id("mem_1"), "mem_1");
    }

    #[test]
    fn sanitize_stub_id_escapes_unsafe_chars() {
        // Imported ids may carry slashes/spaces/dots that break filenames.
        let unsafe_id = "mem_a/b c.d";
        let safe = sanitize_stub_id(unsafe_id);
        assert!(!safe.contains('/'));
        assert!(!safe.contains(' '));
        // Distinct safe-charset ids map distinctly.
        assert_ne!(sanitize_stub_id("mem_a/b"), sanitize_stub_id("mem_a-b"));
    }

    #[test]
    fn project_stubs_writes_read_only_notes_under_sources_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let ids = ["mem_1".to_string(), "mem_2".to_string()];
        project_stubs_for_page(dir.path(), "page_a", &ids).unwrap();
        let p1 = dir.path().join("_sources").join("mem_1.md");
        let p2 = dir.path().join("_sources").join("mem_2.md");
        assert!(p1.exists());
        assert!(p2.exists());
        let body = std::fs::read_to_string(&p1).unwrap();
        // Stub identifies the memory and is marked a read-only projection.
        assert!(body.contains("mem_1"));
        assert!(body.contains("read-only"));
        // OKF v0.2: `type: source` is the first frontmatter key.
        let (fm, _) = crate::sources::obsidian::extract_frontmatter(&body);
        assert_eq!(fm.get_str("type"), Some("source"));
        let fm_start = body.find("---\n").unwrap() + 4;
        assert!(
            body[fm_start..].starts_with("type: source\n"),
            "type: source must be the first frontmatter key, got: {body}"
        );
    }

    #[test]
    fn shared_stub_bytes_do_not_depend_on_citing_page() {
        let dir = tempfile::TempDir::new().unwrap();
        let source = ["mem_shared".to_string()];
        project_stubs_for_page(dir.path(), "page_a", &source).unwrap();
        let path = dir.path().join("_sources").join("mem_shared.md");
        let from_page_a = std::fs::read(&path).unwrap();

        project_stubs_for_page(dir.path(), "page_b", &source).unwrap();

        assert_eq!(std::fs::read(path).unwrap(), from_page_a);
    }

    #[test]
    fn gc_removes_orphan_mem_stubs_keeps_still_cited() {
        let dir = tempfile::TempDir::new().unwrap();
        // page_a cites mem_1, mem_2; page_b cites mem_2.
        let mut manifest = StubManifest::default();
        manifest.record("page_a", &["mem_1".to_string(), "mem_2".to_string()]);
        manifest.record("page_b", &["mem_2".to_string()]);
        project_stubs_for_page(
            dir.path(),
            "page_a",
            &["mem_1".to_string(), "mem_2".to_string()],
        )
        .unwrap();
        project_stubs_for_page(dir.path(), "page_b", &["mem_2".to_string()]).unwrap();

        // page_a re-projected, now citing only mem_1 → mem_2 still cited by page_b.
        manifest.record("page_a", &["mem_1".to_string()]);
        gc_orphan_stubs(dir.path(), &manifest).unwrap();
        assert!(dir.path().join("_sources").join("mem_1.md").exists());
        assert!(dir.path().join("_sources").join("mem_2.md").exists());

        // page_b drops mem_2 entirely → mem_2 now orphan → GC removes it.
        manifest.record("page_b", &[]);
        gc_orphan_stubs(dir.path(), &manifest).unwrap();
        assert!(dir.path().join("_sources").join("mem_1.md").exists());
        assert!(!dir.path().join("_sources").join("mem_2.md").exists());
    }

    #[test]
    fn gc_reaps_daemon_stubs_spares_user_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        // Daemon-written stubs (carry the origin_stub: marker) for a mem_ id
        // AND a non-mem_ import id — both must be reaped when orphaned.
        project_stubs_for_page(dir.path(), "page_a", &["mem_orphan".to_string()]).unwrap();
        project_stubs_for_page(dir.path(), "page_b", &["import_42_3".to_string()]).unwrap();
        // User notes under _sources/ (no marker) — any name, including one that
        // looks like a daemon stub (`mem_decoy.md`). Both must survive.
        std::fs::write(sources.join("my-research.md"), "my own note").unwrap();
        std::fs::write(sources.join("mem_decoy.md"), "user note, not a stub").unwrap();

        let manifest = StubManifest::default(); // nothing cited → all stubs orphan
        gc_orphan_stubs(dir.path(), &manifest).unwrap();

        assert!(
            !sources.join("mem_orphan.md").exists(),
            "orphan daemon mem_ stub reaped"
        );
        assert!(
            !sources.join("import_42_3.md").exists(),
            "orphan daemon import_ stub reaped (no longer leaks)"
        );
        assert!(
            sources.join("my-research.md").exists(),
            "user file (no marker) must survive"
        );
        assert!(
            sources.join("mem_decoy.md").exists(),
            "user file named mem_* (no marker) must survive — marker-based, not name-based"
        );
    }

    /// Muse: a person's copy of a stub has pristine bytes at a name Wenlan
    /// would never write. Bytes alone said "regenerable, delete it"; the
    /// filename says whose file it is.
    #[test]
    fn gc_archives_a_users_copy_of_a_stub_instead_of_deleting_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        std::fs::create_dir_all(&sources).unwrap();
        let body = stub_body("mem_1");
        std::fs::write(sources.join(stub_filename("mem_1")), &body).unwrap();
        std::fs::write(sources.join("my-copy.md"), &body).unwrap();

        gc_orphan_stubs(dir.path(), &StubManifest::default()).unwrap();

        assert!(
            !sources.join(stub_filename("mem_1")).exists(),
            "the stub at its own name is still reaped"
        );
        assert!(
            !sources.join("my-copy.md").exists(),
            "the copy is moved, not left in place"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("archive/my-copy.md")).unwrap(),
            body,
            "the user's copy is preserved under archive/"
        );
    }

    /// Muse: an editor that normalizes to CRLF or adds a byte-order mark used
    /// to make a stub read as "not ours", so it was never reaped and leaked.
    #[test]
    fn gc_still_recognizes_a_stub_with_crlf_endings_or_a_bom() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        std::fs::create_dir_all(&sources).unwrap();
        std::fs::write(
            sources.join(stub_filename("mem_crlf")),
            stub_body("mem_crlf").replace('\n', "\r\n"),
        )
        .unwrap();
        std::fs::write(
            sources.join(stub_filename("mem_bom")),
            format!("\u{feff}{}", stub_body("mem_bom")),
        )
        .unwrap();

        gc_orphan_stubs(dir.path(), &StubManifest::default()).unwrap();

        // Neither is byte-identical to what stub_body writes, so both are
        // treated as edited: recognized as ours, and preserved rather than
        // deleted.
        assert!(
            !sources.join(stub_filename("mem_crlf")).exists()
                && !sources.join(stub_filename("mem_bom")).exists(),
            "a normalized stub is recognized instead of leaking forever"
        );
        assert!(
            dir.path()
                .join("archive")
                .join(stub_filename("mem_crlf"))
                .exists()
                && dir
                    .path()
                    .join("archive")
                    .join(stub_filename("mem_bom"))
                    .exists(),
            "and bytes Wenlan cannot reproduce exactly are kept"
        );
    }

    /// The finding this GC was rewritten for: ownership used to be
    /// `content.contains("origin_stub:")`, so a note of the user's own that
    /// merely QUOTED the marker was unlinked on the next projection write.
    #[test]
    fn gc_spares_a_user_note_that_mentions_the_marker_in_its_body() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        std::fs::create_dir_all(&sources).unwrap();
        let note = "# How the projection works\n\nEach stub carries \
                    `origin_stub:` in its frontmatter, which is how GC knows \
                    the file is Wenlan's.\n";
        std::fs::write(sources.join("provenance-notes.md"), note).unwrap();
        // The marker in a fenced block, and one indented under another key:
        // neither is a top-level frontmatter key of this file.
        std::fs::write(
            sources.join("snippet.md"),
            "---\ntitle: \"notes\"\nmeta:\n  origin_stub: \"mem_x\"\n---\n\n```yaml\norigin_stub: \"mem_x\"\n```\n",
        )
        .unwrap();

        gc_orphan_stubs(dir.path(), &StubManifest::default()).unwrap();

        assert_eq!(
            std::fs::read_to_string(sources.join("provenance-notes.md")).unwrap(),
            note,
            "a user note quoting the marker is not a daemon stub"
        );
        assert!(
            sources.join("snippet.md").exists(),
            "the marker in a code fence or under a nested key is not ownership"
        );
        assert!(
            !dir.path().join("archive").exists(),
            "a file that was never Wenlan's is not archived either, just left alone"
        );
    }

    /// The marker proves Wenlan WROTE the stub, never that the bytes are still
    /// Wenlan's: an edit made in place in the vault leaves frontmatter alone.
    /// So an orphaned stub someone has typed into moves to `archive/` instead
    /// of being unlinked.
    #[test]
    fn gc_archives_an_edited_stub_and_removes_a_pristine_one() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        project_stubs_for_page(
            dir.path(),
            "page_a",
            &["mem_edited".to_string(), "mem_pristine".to_string()],
        )
        .unwrap();
        let edited = format!(
            "{}\nMy own note about why this memory matters.\n",
            std::fs::read_to_string(sources.join("mem_edited.md")).unwrap()
        );
        std::fs::write(sources.join("mem_edited.md"), &edited).unwrap();

        gc_orphan_stubs(dir.path(), &StubManifest::default()).unwrap();

        assert!(
            !sources.join("mem_pristine.md").exists(),
            "an untouched stub is regenerable, so it is removed"
        );
        assert!(!sources.join("mem_edited.md").exists(), "edited stub moved");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("archive/mem_edited.md")).unwrap(),
            edited,
            "the edit is preserved byte for byte in the projection root's archive/"
        );
    }

    /// A stub that is STILL CITED and has been typed in is archived before it
    /// is replaced, the same treatment the orphan GC already gave. The bug
    /// this covers: the projection truncated it on every write of every page
    /// citing that memory, so an edit survived only until the next refresh.
    #[test]
    fn an_edited_cited_stub_is_archived_before_it_is_rewritten() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        let ids = ["mem_note".to_string()];
        project_stubs_for_page(dir.path(), "page_a", &ids).unwrap();
        let pristine = std::fs::read_to_string(sources.join("mem_note.md")).unwrap();
        let edited = format!("{pristine}\nWhy this memory matters to me.\n");
        std::fs::write(sources.join("mem_note.md"), &edited).unwrap();

        // Any later page citing the same memory re-projects the stub.
        project_stubs_for_page(dir.path(), "page_b", &ids).unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("archive/mem_note.md")).unwrap(),
            edited,
            "the edit is preserved byte for byte before the stub is regenerated"
        );
        assert_eq!(
            std::fs::read_to_string(sources.join("mem_note.md")).unwrap(),
            pristine,
            "the stub itself is still refreshed to the generated bytes"
        );
    }

    /// The archive is for edits, not for traffic: re-projecting an untouched
    /// stub must not fill `archive/` with copies of bytes nobody changed.
    #[test]
    fn an_untouched_cited_stub_is_rewritten_without_archiving() {
        let dir = tempfile::TempDir::new().unwrap();
        let ids = ["mem_quiet".to_string()];
        project_stubs_for_page(dir.path(), "page_a", &ids).unwrap();
        project_stubs_for_page(dir.path(), "page_b", &ids).unwrap();
        project_stubs_for_page(dir.path(), "page_c", &ids).unwrap();

        assert!(
            !dir.path().join("archive").exists(),
            "no archive entry for a stub nobody edited"
        );
    }

    /// Muse: archiving renames, which needs no read permission, so treating an
    /// unreadable file as "edited" would let the pass take over a name whose
    /// bytes it never saw. The orphan GC keeps such a file; so does this.
    #[cfg(unix)]
    #[test]
    fn a_cited_stub_this_pass_cannot_read_is_left_alone_not_archived() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        std::fs::create_dir_all(&sources).unwrap();
        let theirs = sources.join("mem_locked.md");
        std::fs::write(&theirs, "a note only this user has\n").unwrap();
        std::fs::set_permissions(&theirs, std::fs::Permissions::from_mode(0o000)).unwrap();

        project_stubs_for_page(dir.path(), "page_a", &["mem_locked".to_string()]).unwrap();

        assert!(
            !dir.path().join("archive/mem_locked.md").exists(),
            "a file this pass never read must not be claimed by renaming it away"
        );
        std::fs::set_permissions(&theirs, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            std::fs::read_to_string(&theirs).unwrap(),
            "a note only this user has\n",
            "and its bytes are untouched"
        );
    }

    /// A symlink at a stub's name is left exactly as it is: not followed (it
    /// could point anywhere in the user's vault) and not replaced.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_cited_stub_is_neither_followed_nor_replaced() {
        let dir = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let target = outside.path().join("elsewhere.md");
        std::fs::write(&target, "# Not a stub\n").unwrap();
        let sources = dir.path().join("_sources");
        std::fs::create_dir_all(&sources).unwrap();
        std::os::unix::fs::symlink(&target, sources.join("mem_link.md")).unwrap();

        project_stubs_for_page(dir.path(), "page_a", &["mem_link".to_string()]).unwrap();

        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "# Not a stub\n",
            "the symlink was not followed"
        );
        assert!(
            std::fs::symlink_metadata(sources.join("mem_link.md"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the symlink itself is still there"
        );
    }

    #[test]
    fn gc_reaps_orphan_import_stub() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        // An imported memory cited by a page → daemon stub projected.
        let mut manifest = StubManifest::default();
        manifest.record("page_x", &["import_9_9".to_string()]);
        project_stubs_for_page(dir.path(), "page_x", &["import_9_9".to_string()]).unwrap();
        assert!(sources.join("import_9_9.md").exists());

        // Page drops the source → import stub is now orphan → GC reaps it.
        manifest.record("page_x", &[]);
        gc_orphan_stubs(dir.path(), &manifest).unwrap();
        assert!(
            !sources.join("import_9_9.md").exists(),
            "orphan import_ stub must be reaped (the leak this fix closes)"
        );
    }

    // The tests below interleave an editor's save with the exporter at the
    // point the issue names: after a file was classified, before the pass
    // acts on its pathname.

    fn private_names(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.starts_with(".wenlan-"))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// An editor that saves by writing a new file and renaming it over the
    /// name (Obsidian on some platforms, most sync clients) lands after the
    /// pass read the pristine bytes. The unlink used to take the save with it.
    #[test]
    fn gc_keeps_an_edit_saved_by_replacing_the_file_after_classification() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        project_stubs_for_page(dir.path(), "page_a", &["mem_race".to_string()]).unwrap();
        let root = Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();
        let mut classified = Vec::new();

        gc_orphan_stubs_in_with_hooks(
            &root,
            &StubManifest::default(),
            &mut |name| {
                classified.push(name.to_string());
                std::fs::write(sources.join(".editor.tmp"), "my edit, saved by replacing\n")
                    .unwrap();
                std::fs::rename(sources.join(".editor.tmp"), sources.join(name)).unwrap();
            },
            &mut |_| {},
        )
        .unwrap();

        assert_eq!(
            classified,
            ["mem_race.md"],
            "the hook ran at the race point"
        );
        assert_eq!(
            std::fs::read_to_string(sources.join("mem_race.md")).unwrap(),
            "my edit, saved by replacing\n",
            "the edit is still where the person saved it"
        );
        assert!(private_names(&sources).is_empty(), "no private name left");
    }

    /// The same, for an editor that writes into the existing file.
    #[test]
    fn gc_keeps_an_edit_written_in_place_after_classification() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        project_stubs_for_page(dir.path(), "page_a", &["mem_race".to_string()]).unwrap();
        let root = Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();

        gc_orphan_stubs_in_with_hooks(
            &root,
            &StubManifest::default(),
            &mut |name| std::fs::write(sources.join(name), "typed in place\n").unwrap(),
            &mut |_| {},
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(sources.join("mem_race.md")).unwrap(),
            "typed in place\n"
        );
        assert!(private_names(&sources).is_empty());
    }

    /// An editor still holding the file open writes into it after the pass has
    /// moved it aside but before the pass re-reads it. The re-read sees the
    /// edit, and the file goes back.
    #[test]
    fn gc_keeps_an_edit_written_through_an_open_handle_after_the_take() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        project_stubs_for_page(dir.path(), "page_a", &["mem_race".to_string()]).unwrap();
        let root = Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();

        gc_orphan_stubs_in_with_hooks(&root, &StubManifest::default(), &mut |_| {}, &mut |aside| {
            std::fs::write(sources.join(aside), "late edit\n").unwrap()
        })
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(sources.join("mem_race.md")).unwrap(),
            "late edit\n"
        );
        assert!(private_names(&sources).is_empty());
    }

    /// While the pass held the edited bytes aside, a newer save took the name.
    /// Neither is lost: the newer file stays, and the edit is archived.
    #[test]
    fn gc_archives_the_edit_when_a_newer_file_took_the_name_meanwhile() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        project_stubs_for_page(dir.path(), "page_a", &["mem_race".to_string()]).unwrap();
        let root = Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();

        gc_orphan_stubs_in_with_hooks(&root, &StubManifest::default(), &mut |_| {}, &mut |aside| {
            std::fs::write(sources.join(aside), "the first edit\n").unwrap();
            std::fs::write(sources.join("mem_race.md"), "a newer save\n").unwrap();
        })
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(sources.join("mem_race.md")).unwrap(),
            "a newer save\n",
            "the file that took the name is not replaced"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("archive/mem_race.md")).unwrap(),
            "the first edit\n",
            "the bytes that were set aside are archived, not dropped"
        );
        assert!(private_names(&sources).is_empty());
    }

    /// A stub classified as edited is archived by taking the file, so an even
    /// later save is what lands in `archive/`; nothing is dropped.
    #[test]
    fn gc_archives_whatever_is_at_an_edited_stub_when_it_acts() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        project_stubs_for_page(dir.path(), "page_a", &["mem_race".to_string()]).unwrap();
        // Still carries the marker, so it classifies as edited, not foreign.
        let edit = |n: &str| format!("{}\n{n} edit\n", stub_body("mem_race"));
        std::fs::write(sources.join("mem_race.md"), edit("first")).unwrap();
        let root = Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();

        gc_orphan_stubs_in_with_hooks(
            &root,
            &StubManifest::default(),
            &mut |name| std::fs::write(sources.join(name), edit("second")).unwrap(),
            &mut |_| {},
        )
        .unwrap();

        assert!(!sources.join("mem_race.md").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("archive/mem_race.md")).unwrap(),
            edit("second")
        );
    }

    /// An untouched stub is still reaped with no hook interference: the new
    /// path removes exactly what it used to.
    #[test]
    fn gc_still_removes_a_pristine_stub_it_took_aside() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        project_stubs_for_page(dir.path(), "page_a", &["mem_quiet".to_string()]).unwrap();
        let root = Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();
        gc_orphan_stubs_in_with_hooks(&root, &StubManifest::default(), &mut |_| {}, &mut |_| {})
            .unwrap();
        assert!(!sources.join("mem_quiet.md").exists());
        assert!(private_names(&sources).is_empty());
        assert!(!dir.path().join("archive").exists());
    }

    /// The projection side of the same race: a cited stub that was identical to
    /// what is about to be written is edited between the check and the write.
    /// It used to be truncated in place.
    #[test]
    fn projection_keeps_an_edit_made_after_the_stub_was_classified() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        let ids = ["mem_note".to_string()];
        project_stubs_for_page(dir.path(), "page_a", &ids).unwrap();
        let root = Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();

        project_stubs_for_page_in_with_hook(&root, "page_b", &ids, &mut |name| {
            std::fs::write(sources.join(".editor.tmp"), "typed meanwhile\n").unwrap();
            std::fs::rename(sources.join(".editor.tmp"), sources.join(name)).unwrap();
        })
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(sources.join("mem_note.md")).unwrap(),
            "typed meanwhile\n",
            "the edit was not overwritten"
        );
        assert!(private_names(&sources).is_empty());

        // The next write sees an edited stub, archives it and refreshes.
        project_stubs_for_page(dir.path(), "page_c", &ids).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("archive/mem_note.md")).unwrap(),
            "typed meanwhile\n"
        );
        assert_eq!(
            std::fs::read_to_string(sources.join("mem_note.md")).unwrap(),
            stub_body("mem_note")
        );
    }

    /// After an edited stub is archived the name is free. A save that takes it
    /// before the refreshed stub lands is kept, not replaced.
    #[test]
    fn projection_does_not_replace_a_file_that_took_the_name_after_archiving() {
        let dir = tempfile::TempDir::new().unwrap();
        let sources = dir.path().join("_sources");
        let ids = ["mem_note".to_string()];
        project_stubs_for_page(dir.path(), "page_a", &ids).unwrap();
        std::fs::write(sources.join("mem_note.md"), "first edit\n").unwrap();
        let root = Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();

        project_stubs_for_page_in_with_hook(&root, "page_b", &ids, &mut |name| {
            assert!(!sources.join(name).exists(), "archived before the hook");
            std::fs::write(sources.join(name), "second edit\n").unwrap();
        })
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(sources.join("mem_note.md")).unwrap(),
            "second edit\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("archive/mem_note.md")).unwrap(),
            "first edit\n"
        );
    }
}
