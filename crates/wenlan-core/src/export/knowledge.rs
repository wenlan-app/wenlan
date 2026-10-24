// SPDX-License-Identifier: Apache-2.0
//! Knowledge writer — exports pages as `.md` files with state tracking.

use crate::error::WenlanError;
use crate::export::obsidian::{convert_links_to_wikilinks, slugify};
use crate::pages::Page;
use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _};
use cap_std::fs::{Dir, OpenOptions};
use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use wenlan_types::CodeBasisLocator;

#[path = "knowledge_folders.rs"]
mod folders;
pub use folders::{
    create_knowledge_folder, knowledge_edit_targets, knowledge_markdown_paths,
    list_knowledge_folders, move_projected_page, read_knowledge_markdown,
    validate_knowledge_folder_path, KnowledgeFolderEntry,
};

const KNOWLEDGE_STATE_SCHEMA_V2: u32 = 2;
static PROJECTION_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static PROJECTION_STATE_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const REPAIR_CAPABILITY_READ_MAX_BYTES: u64 = 16 * 1024 * 1024;
const ORPHAN_BASELINE_ENTRY_BUDGET_BYTES: u64 = 4096;
const PROJECTION_UNLINK_STAGE_FILE: &str = "source";
const PROJECTION_ROLLBACK_QUARANTINE_STAGE_FILE: &str = "rollback-quarantine";
const PROJECTION_STATE_STAGE_FILE: &str = "state";
const PROJECTION_STAGE_OWNER_FILE: &str = "owner.json";
#[cfg(unix)]
type ProjectionStateMode = u32;
#[cfg(not(unix))]
type ProjectionStateMode = bool;

/// Process-local monotonic counter, combined with the pid, so concurrent
/// `write_page` calls for the same page never pick the same temp filename.
static TEMP_FILE_SEQ: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
#[path = "projection_invariant_test.rs"]
mod projection_invariant_test;

#[derive(Debug, Serialize, Deserialize)]
struct KnowledgeState {
    #[serde(default = "default_schema_v2")]
    schema_version: u32,
    #[serde(deserialize_with = "folders::deserialize_pages")]
    pages: HashMap<String, PageFileState>,
}

impl Default for KnowledgeState {
    fn default() -> Self {
        Self {
            schema_version: KNOWLEDGE_STATE_SCHEMA_V2,
            pages: HashMap::new(),
        }
    }
}

fn default_schema_v2() -> u32 {
    KNOWLEDGE_STATE_SCHEMA_V2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PageFileState {
    file: String,
    /// SHA-256 of the exact bytes installed, not of canonicalized prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_sha256: Option<String>,
    version: i64,
    last_written: String,
    /// The index-relevant fields as of the write that produced `file`, so
    /// `regenerate_index_cap` can build `index.md` from `state.json` instead
    /// of re-opening and re-parsing every projected file. `None` means a
    /// legacy entry written before this field existed -- `regenerate_index_cap`
    /// falls back to a file-head read for those.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    space: Option<String>,
}

/// What one `reconcile` pass repaired. Logged as a single summary line at
/// startup — per-page detail would be thousands of lines on a real corpus.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileStats {
    /// Pages with a `state.json` entry, i.e. ones we could check at all.
    pub checked: usize,
    /// Projections found behind the DB and rewritten.
    pub rewritten: usize,
    /// `write_page` leftovers swept from the pages directory.
    pub temp_files_removed: usize,
    /// Repairs that failed; the page keeps its stale file until next boot.
    pub errors: usize,
}

/// The `origin_version` stamp `render_markdown` writes into every projected
/// file, or 0 when the file has no parseable frontmatter (empty, truncated, or
/// never ours). Shared by the page-watcher and the startup reconcile: they
/// make opposite decisions off this number, so reading it two different ways
/// would let reconcile overwrite exactly what the watcher protects.
///
/// Tolerant of `3`, `3.0` and `"3"` — YAML gives each a different serde shape,
/// and a hand-retyped frontmatter should not be mistaken for a stale one.
pub(crate) fn projected_origin_version(fm: &crate::sources::obsidian::NoteFrontmatter) -> i64 {
    fm.fields
        .get("origin_version")
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_f64().map(|f| f as i64))
                .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
        })
        .unwrap_or(0)
}

/// True only for the `.{page_id}.{pid}.{seq}.tmp` names `write_page` creates.
/// Page ids carry no dots, so "exactly three components, the last two all
/// digits" is our writer's shape and nobody else's — an editor swap file or a
/// user's own `.tmp` scratch in the pages directory does not match.
fn is_write_page_temp_file(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('.').and_then(|n| n.strip_suffix(".tmp")) else {
        return false;
    };
    let mut parts = rest.rsplitn(3, '.');
    let seq = parts.next().unwrap_or_default();
    let pid = parts.next().unwrap_or_default();
    let page_id = parts.next().unwrap_or_default();
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    !page_id.is_empty() && !page_id.contains('.') && all_digits(pid) && all_digits(seq)
}

/// Legacy v1 KnowledgeState. The Phase 0 (Page) taxonomy refactor renamed
/// `concepts` → `pages` and changed the page-id prefix `concept_<uuid>` →
/// `page_<uuid>`. v1 state.json files are auto-migrated on read. Drop in
/// next minor release.
#[derive(Debug, Default, Deserialize)]
struct LegacyKnowledgeStateV1 {
    #[serde(deserialize_with = "folders::deserialize_pages")]
    concepts: HashMap<String, PageFileState>,
}

/// Rename may rewrite the projection state, so unknown or malformed state
/// entries must fail closed instead of being silently discarded by serde.
fn parse_rename_projection_state(bytes: &[u8]) -> Result<KnowledgeState, WenlanError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| WenlanError::Conflict("page_projection_state_invalid".into()))?;
    let object = value
        .as_object()
        .ok_or_else(|| WenlanError::Conflict("page_projection_state_invalid".into()))?;
    if object
        .keys()
        .any(|key| !["schema_version", "pages"].contains(&key.as_str()))
    {
        return Err(WenlanError::Conflict(
            "page_projection_state_invalid".into(),
        ));
    }
    let pages = object
        .get("pages")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| WenlanError::Conflict("page_projection_state_invalid".into()))?;
    const PAGE_KEYS: &[&str] = &[
        "file",
        "content_sha256",
        "version",
        "last_written",
        "title",
        "description",
        "space",
    ];
    for entry in pages.values() {
        let entry = entry
            .as_object()
            .ok_or_else(|| WenlanError::Conflict("page_projection_state_invalid".into()))?;
        if entry.keys().any(|key| !PAGE_KEYS.contains(&key.as_str())) {
            return Err(WenlanError::Conflict(
                "page_projection_state_invalid".into(),
            ));
        }
    }

    let mut state = serde_json::from_slice::<KnowledgeState>(bytes)
        .map_err(|_| WenlanError::Conflict("page_projection_state_invalid".into()))?;
    if state.pages.len() != pages.len() || pages.keys().any(|id| !state.pages.contains_key(id)) {
        return Err(WenlanError::Conflict(
            "page_projection_state_invalid".into(),
        ));
    }
    if state.schema_version == 0 {
        state.schema_version = KNOWLEDGE_STATE_SCHEMA_V2;
    }
    if state.schema_version != KNOWLEDGE_STATE_SCHEMA_V2 {
        return Err(WenlanError::Conflict(
            "page_projection_state_invalid".into(),
        ));
    }
    let mut paths = std::collections::HashSet::new();
    for entry in state.pages.values() {
        folders::validate_page_path(&entry.file)
            .map_err(|_| WenlanError::Conflict("page_projection_state_invalid".into()))?;
        if !paths.insert(folders::collision_key(&entry.file)) {
            return Err(WenlanError::Conflict(
                "page_projection_state_invalid".into(),
            ));
        }
    }
    Ok(state)
}

/// Reserved OKF v0.2 root document (spec 2026-09-16-okf-projection.md change
/// 4). Not a page: no `origin_id`, so `sources::page_watcher` already leaves
/// it alone, and `lint::pages::traversal::scope_for` excludes it from
/// `EntryScope::PageMarkdown`.
const INDEX_FILE: &str = "index.md";
/// The other filename OKF reserves at every level of the hierarchy (spec 3.1:
/// a directory's update history). Wenlan does not write one, but the spec says
/// a concept document may not take the name, so a page titled "Log" gets
/// `log-page.md` the same way a page titled "Index" gets `index-page.md`.
const LOG_FILE: &str = "log.md";
/// Bounded frontmatter read for building `index.md` from projected files —
/// generous enough for title/description/space/tags, small next to a page body.
const INDEX_FRONTMATTER_SCAN_BYTES: u64 = 8 * 1024;
/// Bounded WHOLE-file read for deciding whether `index.md` is ours to replace.
/// The generated index is one line per page, so this covers a corpus far
/// larger than any real vault; past it the file is left alone rather than
/// claimed on the strength of a prefix.
const INDEX_OWNERSHIP_SCAN_BYTES: u64 = 8 * 1024 * 1024;

/// Whether a projected filename is the OKF root document. Case-insensitive,
/// like the default macOS and Windows filesystems.
pub(crate) fn is_index_file(name: &str) -> bool {
    name.eq_ignore_ascii_case(INDEX_FILE)
}

/// Whether a projected filename is one OKF reserves (spec 3.1). Neither may
/// be a concept document. Case-insensitive, like `is_index_file`.
pub(crate) fn is_reserved_okf_filename(name: &str) -> bool {
    is_index_file(name) || name.eq_ignore_ascii_case(LOG_FILE)
}

/// The filename stem for a page slug. `index.md` and `log.md` are reserved by
/// OKF and are never a page's file, so a page titled "Index" gets `index-page`
/// and one titled "Log" gets `log-page`.
pub(crate) fn page_stem_clear_of_reserved(slug: String) -> String {
    if is_reserved_okf_filename(&format!("{slug}.md")) {
        format!("{slug}-page")
    } else {
        slug
    }
}
/// Set after the first warning that a user's own `index.md` blocked the
/// generated one, so a busy projection logs it once rather than per write.
static USER_INDEX_SKIP_WARNED: AtomicBool = AtomicBool::new(false);

pub struct KnowledgeWriter {
    path: PathBuf,
    tracker: std::sync::Arc<crate::page_projection_tracker::PageProjectionTracker>,
    reap_orphan_stubs: bool,
    write_provenance: bool,
    protect_external_edits: bool,
}

impl KnowledgeWriter {
    pub fn new(path: PathBuf, database: &crate::db::MemoryDB) -> Self {
        Self {
            path,
            tracker: database.page_projection_tracker(),
            reap_orphan_stubs: true,
            write_provenance: true,
            protect_external_edits: true,
        }
    }

    fn new_repair(path: PathBuf, database: &crate::db::MemoryDB) -> Self {
        Self {
            path,
            tracker: database.page_projection_tracker(),
            reap_orphan_stubs: false,
            write_provenance: false,
            protect_external_edits: false,
        }
    }

    pub fn begin_projection_write(&self) -> KnowledgeProjectionWriteRef<'_> {
        KnowledgeProjectionWriteRef {
            writer: self,
            guard: self.tracker.begin_write(),
        }
    }

    #[cfg(test)]
    fn new_for_test(path: PathBuf) -> Self {
        Self {
            path,
            tracker: crate::page_projection_tracker::PageProjectionTracker::new(),
            reap_orphan_stubs: true,
            write_provenance: true,
            protect_external_edits: true,
        }
    }

    /// A writer shaped like the reconcile repair path: no stub projection, no
    /// index regeneration of its own.
    #[cfg(test)]
    fn new_for_test_repair(path: PathBuf) -> Self {
        Self {
            path,
            tracker: crate::page_projection_tracker::PageProjectionTracker::new(),
            reap_orphan_stubs: false,
            write_provenance: false,
            protect_external_edits: false,
        }
    }

    #[cfg(test)]
    fn begin_test_write(&self) -> crate::page_projection_tracker::PageProjectionWriteGuard {
        self.tracker.begin_write()
    }

    #[cfg(test)]
    fn write_page_for_test(&self, page: &Page) -> Result<String, WenlanError> {
        let guard = self.begin_test_write();
        self.write_page(&guard, page)
    }

    #[cfg(all(test, unix))]
    fn write_page_after_open_for_test<F>(
        &self,
        page: &Page,
        after_open: F,
    ) -> Result<String, WenlanError>
    where
        F: FnOnce() -> Result<(), WenlanError>,
    {
        let guard = self.begin_test_write();
        self.validate_guard(&guard)?;
        create_projection_root_nofollow(&self.path)?;
        KnowledgeProjectionWrite::with_projection_capabilities(&self.path, |capabilities| {
            after_open()?;
            self.write_page_with_lock_held(capabilities, &guard, page, true)
        })
    }

    #[cfg(test)]
    fn remove_page_for_test(&self, page_id: &str) -> Result<(), WenlanError> {
        let guard = self.begin_test_write();
        self.remove_page(&guard, page_id)
    }

    #[cfg(test)]
    fn reconcile_for_test(&self, pages: &[Page]) -> Result<ReconcileStats, WenlanError> {
        let guard = self.begin_test_write();
        self.reconcile(&guard, pages)
    }

    pub fn write_page(
        &self,
        guard: &crate::page_projection_tracker::PageProjectionWriteGuard,
        page: &Page,
    ) -> Result<String, WenlanError> {
        self.write_page_with_regenerate_index(guard, page, true)
    }

    /// Place a new projection directly in a folder. Existing mapping always wins.
    pub fn write_page_in_folder(
        &self,
        guard: &crate::page_projection_tracker::PageProjectionWriteGuard,
        page: &Page,
        initial_folder: Option<&str>,
    ) -> Result<String, WenlanError> {
        self.validate_guard(guard)?;
        if let Some(folder) = initial_folder {
            validate_knowledge_folder_path(folder)?;
        }
        create_projection_root_nofollow(&self.path)?;
        KnowledgeProjectionWrite::with_projection_capabilities(&self.path, |capabilities| {
            self.write_page_with_lock_held_and_folder(capabilities, guard, page, initial_folder)
        })
    }

    fn write_page_with_lock_held_and_folder(
        &self,
        capabilities: &ProjectionCapabilities,
        guard: &crate::page_projection_tracker::PageProjectionWriteGuard,
        page: &Page,
        initial_folder: Option<&str>,
    ) -> Result<String, WenlanError> {
        self.write_page_with_lock_held_and_hook_in_folder(
            capabilities,
            guard,
            page,
            || Ok(()),
            true,
            initial_folder,
        )
    }

    /// [`Self::write_page`], with the `index.md` regeneration made optional.
    ///
    /// `reconcile` rewrites potentially many behind pages in one pass and
    /// regenerates the index once at the end (see `reconcile` below) — asking
    /// every per-page rewrite to also regenerate it would turn an O(1)
    /// regeneration into O(pages rewritten) for no benefit, since only the
    /// final state matters.
    fn write_page_with_regenerate_index(
        &self,
        guard: &crate::page_projection_tracker::PageProjectionWriteGuard,
        page: &Page,
        regenerate_index: bool,
    ) -> Result<String, WenlanError> {
        self.validate_guard(guard)?;
        create_projection_root_nofollow(&self.path)?;
        KnowledgeProjectionWrite::with_projection_capabilities(&self.path, |capabilities| {
            self.write_page_with_lock_held(capabilities, guard, page, regenerate_index)
        })
    }

    fn write_page_with_lock_held(
        &self,
        capabilities: &ProjectionCapabilities,
        guard: &crate::page_projection_tracker::PageProjectionWriteGuard,
        page: &Page,
        regenerate_index: bool,
    ) -> Result<String, WenlanError> {
        self.write_page_with_lock_held_and_hook(
            capabilities,
            guard,
            page,
            || Ok(()),
            regenerate_index,
        )
    }

    fn write_page_with_lock_held_and_hook<F>(
        &self,
        capabilities: &ProjectionCapabilities,
        guard: &crate::page_projection_tracker::PageProjectionWriteGuard,
        page: &Page,
        after_target_write: F,
        regenerate_index: bool,
    ) -> Result<String, WenlanError>
    where
        F: FnOnce() -> Result<(), WenlanError>,
    {
        self.write_page_with_lock_held_and_hook_in_folder(
            capabilities,
            guard,
            page,
            after_target_write,
            regenerate_index,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn write_page_with_lock_held_and_hook_in_folder<F>(
        &self,
        capabilities: &ProjectionCapabilities,
        guard: &crate::page_projection_tracker::PageProjectionWriteGuard,
        page: &Page,
        after_target_write: F,
        regenerate_index: bool,
        initial_folder: Option<&str>,
    ) -> Result<String, WenlanError>
    where
        F: FnOnce() -> Result<(), WenlanError>,
    {
        self.validate_guard(guard)?;
        let mut state = folders::read_state_strict(&capabilities.wenlan)?;
        let mapped = state.pages.get(&page.id).map(|entry| entry.file.as_str());
        let claimants = folders::origin_paths(&capabilities.root, &page.id)?;
        if claimants.len() > 1
            || claimants
                .first()
                .is_some_and(|path| mapped.is_some_and(|mapped| mapped != path))
        {
            return Err(WenlanError::Conflict(
                "page_projection_identity_ambiguous".into(),
            ));
        }
        let folder = match mapped {
            Some(path) => path.rsplit_once('/').map_or("", |(folder, _)| folder),
            None => initial_folder.unwrap_or(""),
        };
        let directory = if mapped.is_some() {
            folders::open_existing_folder(&capabilities.root, folder)?
        } else {
            folders::open_folder(&capabilities.root, folder)?
        };
        let filename =
            self.unique_filename_in_folder(&directory, &page.id, &page.title, &state, folder)?;
        let leaving_reserved = mapped
            .is_some_and(|path| is_reserved_okf_filename(path.rsplit('/').next().unwrap_or(path)));
        if !leaving_reserved && claimants.first().is_some_and(|path| path != &filename) {
            return Err(WenlanError::Conflict(
                "page_projection_identity_ambiguous".into(),
            ));
        }
        let file_path = self.path.join(&filename);
        let (target_directory, target_name) = folders::page_parent(&capabilities.root, &filename)?;

        let content = render_markdown(page);
        // Write to a temp file in the same capability directory, then rename
        // over the target: rename within one directory is atomic, so a reader
        // (Obsidian, the fs watcher) never observes a half-written page file.
        // Truncating the target in place — what `write_regular_nofollow` does —
        // cannot give that: it leaves a window where the file is short or empty.
        //
        // The symlink and regular-file guarantees are preserved: the temp is
        // opened `create_new` + nofollow, and an existing target is checked to be
        // a regular file before it is replaced.
        let temp_filename = format!(
            ".{}.{}.{}.tmp",
            page.id,
            std::process::id(),
            TEMP_FILE_SEQ.fetch_add(1, Ordering::Relaxed)
        );
        if self.protect_external_edits {
            write_page_preserving_external_edits(
                capabilities,
                &filename,
                &temp_filename,
                page,
                content.as_bytes(),
                state.pages.get(&page.id),
            )?;
        } else {
            // Explicit repair operations carry their own captured-byte guards.
            write_page_atomically_nofollow(
                &target_directory,
                &target_name,
                &temp_filename,
                content.as_bytes(),
            )?;
        }
        after_target_write()?;

        if self.write_provenance {
            // Project read-only source stubs so [[mem_*]] resolves in Obsidian.
            if let Err(e) = crate::export::provenance::project_stubs_for_page_in(
                &capabilities.root,
                &page.id,
                &page.source_memory_ids,
            ) {
                log::warn!("[knowledge] stub projection failed for {}: {e}", page.id);
            }

            // Manifest is a cosmetic projection index, updated load-modify-save
            // without a lock. HTTP page-write handlers can run concurrently with
            // scheduler distillation, so a lost update is possible — but the blast
            // radius is one stub and it self-heals: a spuriously-GC'd stub for a
            // live page is regenerated on that page's next write_page (project runs
            // before this update), and a leaked orphan is reaped on the next GC. A
            // real lock is a P2 concern, unjustified for a regenerable nav aid.
            let mut manifest =
                crate::export::provenance::StubManifest::load_from(&capabilities.root);
            manifest.record(&page.id, &page.source_memory_ids);
            let _ = manifest.save_to(&capabilities.root);
            if self.reap_orphan_stubs {
                let _ =
                    crate::export::provenance::gc_orphan_stubs_in(&capabilities.root, &manifest);
            }
        }

        // A page leaving a reserved name for its own (see `unique_filename_cap`).
        let left_reserved_copy = state
            .pages
            .get(&page.id)
            .map(|entry| entry.file.clone())
            .filter(|old| {
                is_reserved_okf_filename(old.rsplit('/').next().unwrap_or(old)) && *old != filename
            });
        state.pages.insert(
            page.id.clone(),
            PageFileState {
                file: filename,
                content_sha256: Some(projection_digest(content.as_bytes())),
                version: page.version,
                last_written: page.last_modified.clone(),
                title: Some(page.title.clone()),
                description: page.summary.clone(),
                space: page.space.clone(),
            },
        );
        self.save_state_cap(&capabilities.wenlan, &state)?;
        let relocated_off_reserved = left_reserved_copy.is_some();
        if let Some(old) = left_reserved_copy {
            Self::remove_reserved_name_copy_left_by(&capabilities.root, &old, &page.id);
        }

        // A relocation forces the index even for a writer that normally skips
        // it: the entry that just moved is the one link in `index.md` that is
        // now wrong, and the repair path has no later pass that would fix it.
        if relocated_off_reserved || (self.write_provenance && regenerate_index) {
            if let Err(e) = Self::regenerate_index_cap(capabilities, &state) {
                log::warn!("[knowledge] index.md regeneration failed: {e}");
            }
        }

        Ok(file_path.to_string_lossy().to_string())
    }

    /// Repair the markdown projection from the DB, which is authoritative.
    ///
    /// `write_page` deliberately skips fsync, so a crash can leave a page's
    /// file missing, holding the previous version's bytes (rename undone), or
    /// zero-length (rename durable, bytes not). This brings those forward
    /// through the same atomic `write_page` path, and sweeps `.tmp` leftovers
    /// from writes that died between write and rename.
    ///
    /// A stale stamp is a repair candidate, never permission to overwrite.
    /// The writer checks its last byte digest and preserves external edits.
    /// Legacy files without a digest are upgraded only if their prose already
    /// matches the canonical page. Unknown differing prose is left intact.
    /// Pages without a state entry are skipped to avoid duplicate filenames.
    pub fn reconcile(
        &self,
        guard: &crate::page_projection_tracker::PageProjectionWriteGuard,
        pages: &[Page],
    ) -> Result<ReconcileStats, WenlanError> {
        self.validate_guard(guard)?;
        let mut stats = ReconcileStats {
            temp_files_removed: self.sweep_temp_leftovers(),
            ..ReconcileStats::default()
        };

        let state =
            KnowledgeProjectionWrite::with_projection_capabilities(&self.path, |capabilities| {
                folders::read_state_strict(&capabilities.wenlan)
            })?;
        for page in pages {
            let Some(entry) = state.pages.get(&page.id) else {
                continue;
            };
            stats.checked += 1;
            if !self.projection_is_behind(&entry.file, page) {
                continue;
            }
            match self.write_page_with_regenerate_index(guard, page, false) {
                Ok(_) => stats.rewritten += 1,
                Err(e) => {
                    log::warn!("[reconcile] repair failed for {}: {e}", page.id);
                    stats.errors += 1;
                }
            }
        }
        // Regenerate unconditionally, not only when a page above was rewritten:
        // a fresh knowledge root reconciled for the first time has no index.md
        // at all yet, even though no page was individually behind.
        if self.write_provenance {
            if let Err(e) =
                KnowledgeProjectionWrite::with_projection_capabilities(&self.path, |capabilities| {
                    let state = folders::read_state_strict(&capabilities.wenlan)?;
                    Self::regenerate_index_cap(capabilities, &state)
                })
            {
                log::warn!("[reconcile] index.md regeneration failed: {e}");
            }
        }
        Ok(stats)
    }

    /// Whether `filename` is missing, unreadable, or stamped with a version
    /// older than the DB's. Unreadable counts as behind: the repair attempt
    /// will surface the real error rather than silently skipping the page.
    fn projection_is_behind(&self, filename: &str, page: &Page) -> bool {
        let raw = (|| {
            let root = folders::read_root(&self.path)?;
            let (directory, name) = folders::page_parent(&root, filename)?;
            let mut budget = RepairReadBudget::new();
            let bytes = read_regular_nofollow(&directory, OsStr::new(&name), &mut budget)?;
            String::from_utf8(bytes)
                .map_err(|_| WenlanError::Conflict("page_projection_target_invalid".into()))
        })();
        let Ok(raw) = raw else {
            return true;
        };
        let (fm, body) = crate::sources::obsidian::extract_frontmatter(&raw);
        if projected_origin_version(&fm) < page.version {
            return true;
        }
        // Legacy-shaped file from before the OKF frontmatter existed: no
        // `type` key at all. Treat it as behind -- and safe to rewrite -- only
        // when its body matches what we'd render today; a body mismatch means
        // an offline edit is sitting on top of it, and the watcher must pick
        // that up first rather than have reconcile clobber it.
        if fm.fields.contains_key("type") {
            return false;
        }
        let rendered = render_markdown(page);
        let (_, rendered_body) = crate::sources::obsidian::extract_frontmatter(&rendered);
        body == rendered_body
    }

    /// Remove `write_page` temp files orphaned by a crash between write and
    /// rename. Safe to run unconditionally at startup: the daemon holds the
    /// port by this point, so no live writer owns any of these.
    fn sweep_temp_leftovers(&self) -> usize {
        let Ok(root) = folders::read_root(&self.path) else {
            return 0;
        };
        let Ok((folders, _)) = folders::scan_tree(&root) else {
            return 0;
        };
        std::iter::once(String::new())
            .chain(folders.into_iter().map(|folder| folder.path))
            .filter_map(|path| folders::open_existing_folder(&root, &path).ok())
            .map(|directory| {
                let Ok(entries) = directory.entries() else {
                    return 0;
                };
                entries
                    .flatten()
                    .filter(|entry| {
                        let name = entry.file_name();
                        is_write_page_temp_file(&name.to_string_lossy())
                            && directory.symlink_metadata(&name).is_ok_and(|metadata| {
                                metadata.is_file() && !metadata.file_type().is_symlink()
                            })
                            && directory.remove_file(&name).is_ok()
                    })
                    .count()
            })
            .sum()
    }

    fn unique_filename_in_folder(
        &self,
        root: &Dir,
        page_id: &str,
        title: &str,
        state: &KnowledgeState,
        folder: &str,
    ) -> Result<String, WenlanError> {
        if let Some(existing) = state.pages.get(page_id) {
            // A page projected at a reserved name before it was reserved
            // moves to a free name on its next write, so the OKF index can
            // take `index.md` back and no concept document sits at `log.md`.
            // `write_page` removes the copy it leaves behind
            // (`remove_reserved_name_copy_left_by`).
            //
            // EVERY writer does this, the repair writer included. Leaving a
            // concept document at a reserved name is the defect; which writer
            // happens to reach the page next is an accident, and a repair that
            // rewrites `log.md` in place would keep the violation alive
            // indefinitely. `write_page` regenerates the index whenever a
            // relocation happened, so a writer that otherwise skips the index
            // does not leave it pointing at the old name.
            if !is_reserved_okf_filename(existing.file.rsplit('/').next().unwrap_or(&existing.file))
            {
                return Ok(existing.file.clone());
            }
        }
        let mut base = page_stem_clear_of_reserved(slugify(title));
        if base.is_empty() {
            base = "note".into();
        }
        while base.len() > 200 {
            base.pop();
        }
        if folders::validate_new_page_path(&format!("{base}.md")).is_err() {
            base.push_str("-page");
        }
        let mut candidate = format!("{base}.md");
        let mut n = 2;
        let taken: std::collections::HashSet<String> = state
            .pages
            .iter()
            .filter(|(id, _)| id.as_str() != page_id)
            .map(|(_, state)| folders::collision_key(&state.file))
            .collect();
        loop {
            let relative = if folder.is_empty() {
                candidate.clone()
            } else {
                format!("{folder}/{candidate}")
            };
            folders::validate_new_page_path(&relative)?;
            let collides_state = taken.contains(&folders::collision_key(&relative));
            let collides_case = match folders::ensure_case_available(root, &candidate, true) {
                Ok(()) => false,
                Err(WenlanError::Conflict(_)) => true,
                Err(error) => return Err(error),
            };
            let collides_disk = match root.symlink_metadata(&candidate) {
                Ok(_) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(WenlanError::Io(error)),
            };
            if !collides_state && !collides_case {
                if !collides_disk {
                    return Ok(relative);
                }
                // Installation may have completed before its first state
                // entry was saved. Reuse its exact id instead of forking a
                // duplicate; the protected writer still checks its prose.
                if self.protect_external_edits
                    && Self::read_origin_id(root, &OsString::from(&candidate)).as_deref()
                        == Some(page_id)
                {
                    return Ok(relative);
                }
            }
            candidate = format!("{base}-{n}.md");
            n += 1;
            if n > 10_000 {
                return Err(WenlanError::Conflict("knowledge_path_collision".into()));
            }
        }
    }

    /// Resolve the filename currently recorded in `state.json` for a page,
    /// or `None` if the page has no projection yet. Used by the PUT route's
    /// rollback path so a failed DB update can restore the prior md bytes
    /// instead of orphaning the file.
    pub fn page_filename(&self, page_id: &str) -> Option<String> {
        self.load_state().pages.get(page_id).map(|s| s.file.clone())
    }

    /// Verify live flat-root Markdown filenames for an already authorized set
    /// of page IDs. Reads state once, then only those files; never scans, creates
    /// directories, repairs state, or guesses a filename from a title. Missing,
    /// unsafe, unreadable, or mismatched projections are absent from the map.
    pub fn live_page_filenames(&self, page_ids: &[&str]) -> HashMap<String, String> {
        let mut found = HashMap::new();
        if page_ids.is_empty() {
            return found;
        }
        let Some((parent, basename)) = self.path.parent().zip(self.path.file_name()) else {
            return found;
        };
        let Ok(parent) = Dir::open_ambient_dir(parent, cap_std::ambient_authority()) else {
            return found;
        };
        let Ok(root) = parent.open_dir_nofollow(Path::new(basename)) else {
            return found;
        };
        let Ok(control) = root.open_dir_nofollow(".wenlan") else {
            return found;
        };
        // Refuse nonregular state before opening, and bound the existing
        // capability read. This read path deliberately takes no writer lock.
        let Ok(metadata) = control.symlink_metadata("state.json") else {
            return found;
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return found;
        }
        let Ok(state) = folders::read_state_strict(&control) else {
            return found;
        };
        for &page_id in page_ids {
            let Some(entry) = state.pages.get(page_id) else {
                continue;
            };
            let name = &entry.file;
            let Ok((directory, basename)) = folders::page_parent(&root, name) else {
                continue;
            };
            let Ok(metadata) = directory.symlink_metadata(&basename) else {
                continue;
            };
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                continue;
            }
            if Self::read_origin_id(&directory, &OsString::from(basename)).as_deref()
                == Some(page_id)
            {
                found.insert(page_id.to_string(), name.clone());
            }
        }
        found
    }

    pub fn remove_page(
        &self,
        guard: &crate::page_projection_tracker::PageProjectionWriteGuard,
        page_id: &str,
    ) -> Result<(), WenlanError> {
        self.validate_guard(guard)?;
        KnowledgeProjectionWrite::with_projection_capabilities(&self.path, |capabilities| {
            let mut state = folders::read_state_strict(&capabilities.wenlan)?;

            if let Some(entry) = state.pages.remove(page_id) {
                // Delete the file *before* persisting state. If the file remove
                // fails we keep the state entry so the daemon can retry; if the
                // state save fails we have at most a stale empty entry pointing
                // at a missing file (detectable, recoverable) instead of an
                // orphan file with no DB or state reference.
                let (directory, name) = folders::page_parent(&capabilities.root, &entry.file)?;
                match directory.symlink_metadata(&name) {
                    Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                        directory.remove_file(&name)?;
                    }
                    Ok(_) => {
                        return Err(WenlanError::Conflict(
                            "page_projection_target_invalid".to_string(),
                        ))
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(WenlanError::Io(error)),
                }
                self.save_state_cap(&capabilities.wenlan, &state)?;

                let mut manifest =
                    crate::export::provenance::StubManifest::load_from(&capabilities.root);
                manifest.forget_page(page_id);
                let _ = manifest.save_to(&capabilities.root);
                let _ =
                    crate::export::provenance::gc_orphan_stubs_in(&capabilities.root, &manifest);

                if let Err(e) = Self::regenerate_index_cap(capabilities, &state) {
                    log::warn!("[knowledge] index.md regeneration failed: {e}");
                }
            }

            Ok(())
        })
    }

    /// `projection_directory_invariant` — the enforcement address the M5 reader
    /// manifest records for `wenlan pages` (binding spec section 5).
    ///
    /// `wenlan pages` reads Markdown straight off this directory
    /// (`crates/wenlan-cli/src/commands/pages.rs`), so neither frontmatter nor
    /// wire negotiation can reach it: what the directory *contains* is the whole
    /// enforcement. After the cutover it holds supported pages only, and this
    /// pass is what makes that true — it removes the file of every page the
    /// automatic reader may not see.
    ///
    /// Removal, not a write-time skip, is the load-bearing half. Migration 99
    /// backfills every pre-existing page to `provisional`, so the moment the
    /// cutover advances, the directory is already full of files no later write
    /// would ever touch; a writer that merely declined to project new
    /// provisional pages would leave every one of them readable. That is
    /// exactly the weakening section 9 names.
    ///
    /// Inert at cutover generation 0 by construction rather than by a branch of
    /// its own: `page_visibility` short-circuits every page to `Full` there, so
    /// the pass finds nothing to remove and PR-B changes no behavior.
    ///
    /// PR-C: the enumeration is the directory, not `state.json`.
    ///
    /// `load_state` returns `KnowledgeState::default()` on any read error and
    /// `parse_state` falls back to `unwrap_or_default()` on malformed JSON.
    /// Either yields an empty page map, so this pass used to return `Ok(0)` --
    /// reporting success -- while every `.md` file stayed on disk and
    /// `wenlan pages` enumerated that directory independently. State saves
    /// truncate before writing, so a crash mid-save is a realistic way to get
    /// there. A control whose failure mode is a clean success message is not a
    /// control.
    ///
    /// So the pass enumerates the union of `state.json`'s keys and the page IDs
    /// recovered from the `origin_id:` frontmatter of the `.md` files actually
    /// present. Both halves are needed: the scan finds files `state.json` forgot,
    /// and `state.json` finds pages whose file is already gone but whose entry
    /// would otherwise be resurrected by the next write.
    ///
    /// ponytail: the scan reads a bounded frontmatter prefix per file, once per
    /// pass, off the request path. State is now loaded once and saved once
    /// instead of per page, which also retires the old O(n²) note.
    pub async fn enforce_projection_directory_invariant(
        &self,
        database: &crate::db::MemoryDB,
        guard: &crate::page_projection_tracker::PageProjectionWriteGuard,
    ) -> Result<usize, WenlanError> {
        self.validate_guard(guard)?;
        let mut projected: Vec<String> = self.projected_files()?.into_keys().collect();
        projected.sort();
        if projected.is_empty() {
            return Ok(0);
        }
        // `Automatic` is the only honest grant for a directory: the reader is a
        // filesystem, it declares no contract and carries no human gesture.
        let visible = database
            .page_visibility(&crate::truth_contract::TruthGrant::Automatic, &projected)
            .await?;
        // A missing verdict is not permission. `page_visibility` is total over
        // its input, so this only fires if that ever stops being true.
        let evict: Vec<String> = projected
            .into_iter()
            .filter(|id| visible.get(id) != Some(&crate::truth_contract::Visibility::Full))
            .collect();
        if evict.is_empty() {
            return Ok(0);
        }
        self.evict_projected_pages(guard, &evict)
    }

    /// Every page this projection directory would lose at `generation`, without
    /// losing it.
    ///
    /// The point of a dry run is that it is computed the same way the real thing
    /// is, so this shares the invariant's enumeration exactly: the union of
    /// `state.json`'s keys and the IDs recovered from the `.md` frontmatter
    /// actually on disk. A plan built from a different enumeration would be a
    /// forecast of a different operation.
    ///
    /// `generation` is *hypothetical*. The database is still at whatever it is
    /// at -- typically 0, where `page_visibility` short-circuits everything to
    /// `Full` -- so the verdict is computed through the pure
    /// [`crate::truth_contract::visible_at`] rather than through the live gate.
    /// Asking the live gate would answer "nothing would be evicted" and be
    /// useless, which is the trap this method exists to avoid.
    pub async fn plan_truth_cutover(
        &self,
        database: &crate::db::MemoryDB,
        generation: i64,
    ) -> Result<CutoverPlan, WenlanError> {
        let on_disk = self.projected_files()?;
        let mut projected: Vec<String> = on_disk.keys().cloned().collect();
        projected.sort();

        let states = database.page_truth_states(&projected).await?;
        let mut evictions = Vec::new();
        for page_id in &projected {
            let truth = states.get(page_id).copied().unwrap_or_default();
            let visibility = crate::truth_contract::visible_at(
                generation,
                &crate::truth_contract::TruthGrant::Automatic,
                page_id,
                truth.support,
                truth.human_reviewed,
            );
            if visibility != crate::truth_contract::Visibility::Full {
                evictions.push(CutoverEviction {
                    page_id: page_id.clone(),
                    files: on_disk.get(page_id).cloned().unwrap_or_default(),
                });
            }
        }

        // The digest covers everything the decision rests on: which generation,
        // which pages, whether each is supported, and which file each would cost.
        // Anything that changes between the dry run and the apply changes this
        // string, and the apply refuses. It is deliberately not a digest of the
        // eviction list alone -- a page flipping from unsupported to supported
        // shortens that list without the operator ever seeing the new plan.
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        sha2::Digest::update(&mut hasher, generation.to_string().as_bytes());
        for page_id in &projected {
            let truth = states.get(page_id).copied().unwrap_or_default();
            let file = on_disk
                .get(page_id)
                .filter(|files| !files.is_empty())
                .map(|files| files.join(","))
                .unwrap_or_else(|| "-".to_string());
            // Both axes, not just support: a page becoming human-reviewed
            // shortens the eviction list, and the operator has to see the new
            // plan rather than apply one taken before the review.
            sha2::Digest::update(
                &mut hasher,
                format!(
                    "\n{page_id}\t{:?}\t{}\t{file}",
                    truth.support, truth.human_reviewed
                )
                .as_bytes(),
            );
        }
        Ok(CutoverPlan {
            generation,
            projected: projected.len(),
            evictions,
            digest: hex::encode(sha2::Digest::finalize(hasher)),
        })
    }

    /// Advance the cutover, fenced and directory-first.
    ///
    /// The order is the whole design:
    ///
    /// 1. take the lease, which stops every page writer;
    /// 2. remove the files the plan named, *before* the database says anything
    ///    has changed;
    /// 3. commit the generation, then release the fence;
    /// 4. reconcile through the ordinary invariant, which now sees the live
    ///    generation and catches anything the plan missed.
    ///
    /// Directory-first is deliberate. A legacy reader may briefly see a page
    /// missing that the database still calls supported; it must never see stale
    /// provisional prose after the cutover. Omission is recoverable, false trust
    /// is not.
    ///
    /// `expected_digest` is the operator's dry run. It is re-planned here rather
    /// than trusted, so an apply that races a distillation cycle refuses instead
    /// of deleting files nobody reviewed.
    ///
    /// Any failure between (1) and (3) aborts the lease, which returns the fence
    /// to `off` at a **new** epoch and lets writers back in.
    pub async fn run_truth_cutover(
        &self,
        database: &crate::db::MemoryDB,
        generation: i64,
        expected_digest: &str,
    ) -> Result<CutoverPlan, WenlanError> {
        // Take the lease FIRST, then re-plan under it. Planning first leaves a
        // window between the plan and the fence in which a page can flip
        // supported -- and that page's file would then be deleted on the
        // strength of a plan that predates the flip. Once the fence reads
        // `preparing` no page write can land, so a plan taken after it cannot
        // go stale underneath the eviction.
        let lease = database.begin_cutover().await?;

        // The lease is linear, so every path below spends it exactly once. That
        // is why this reads as a chain of `match` rather than one fallible block:
        // handing it to `abort_cutover` consumes it, and the compiler will not
        // let a later line reach for it again.
        let plan = match self.plan_truth_cutover(database, generation).await {
            Ok(plan) => plan,
            Err(error) => {
                let _ = database.abort_cutover(lease).await;
                return Err(error);
            }
        };
        if plan.digest != expected_digest {
            let _ = database.abort_cutover(lease).await;
            return Err(WenlanError::Conflict(format!(
                "the projection changed since the dry run (planned {expected_digest}, \
                 now {}); re-run the dry run and read it before applying",
                plan.digest
            )));
        }

        let evict: Vec<String> = plan.evictions.iter().map(|e| e.page_id.clone()).collect();
        if !evict.is_empty() {
            let guard = database.begin_page_projection_write();
            if let Err(error) = self.evict_projected_pages(&guard, &evict) {
                // Best effort: if the abort also fails the fence stays
                // `preparing`, which refuses page writes. That is the safe side
                // to be stuck on, and the daemon releases it at next startup.
                let _ = database.abort_cutover(lease).await;
                return Err(error);
            }
        }
        // Spends the lease. A failure here leaves the fence at `preparing` on
        // purpose -- there is no lease left to abort with, and the startup
        // release is the recovery path.
        database.commit_cutover(lease, generation).await?;

        let guard = database.begin_page_projection_write();
        self.enforce_projection_directory_invariant(database, &guard)
            .await?;
        Ok(plan)
    }

    /// Every page this projection accounts for, with every file holding it.
    ///
    /// The union of two half-truths. `state.json` knows pages whose `.md` the
    /// scan cannot attribute -- someone edited the `origin_id` out -- and the
    /// scan knows files `state.json` forgot. A page can appear with no files at
    /// all: a stale entry, still worth evicting, because the next write would
    /// resurrect it.
    ///
    /// Files are a list, not one name, because nothing on disk enforces one file
    /// per page. A sync conflict copies the whole `.md`, `origin_id` frontmatter
    /// and all, so two files can claim the same page. Keeping only one of them
    /// archived one copy and left the other readable -- exactly the disclosure
    /// this pass exists to close. Sorted, so the cutover digest is stable across
    /// passes; `HashMap` iteration order and directory order are both arbitrary.
    fn projected_files(&self) -> Result<HashMap<String, Vec<String>>, WenlanError> {
        if !self.path.exists() {
            return Ok(HashMap::new());
        }
        KnowledgeProjectionWrite::with_projection_capabilities(&self.path, |capabilities| {
            let mut found: HashMap<String, Vec<String>> = HashMap::new();
            for path in folders::scan_tree(&capabilities.root)?.1 {
                let (directory, name) = folders::page_parent(&capabilities.root, &path)?;
                if let Some(page_id) = Self::read_origin_id(&directory, &OsString::from(name)) {
                    found.entry(page_id).or_default().push(path);
                }
            }
            for (page_id, entry) in folders::read_state_strict(&capabilities.wenlan)?.pages {
                let slot = found.entry(page_id).or_default();
                let (directory, name) = folders::page_parent(&capabilities.root, &entry.file)?;
                if directory.symlink_metadata(&name).is_ok() {
                    slot.push(entry.file);
                }
            }
            for files in found.values_mut() {
                files.sort();
                files.dedup();
            }
            Ok(found)
        })
    }

    /// The `origin_id` in a projected file's frontmatter, if it has one.
    ///
    /// Bounded read: frontmatter sits at the head of the file, and a page body
    /// can be arbitrarily long. Non-UTF-8 and unreadable files read as "not a
    /// page we wrote", which is the safe answer -- `write_page` always emits
    /// UTF-8 with this key.
    fn read_origin_id(root: &Dir, name: &OsString) -> Option<String> {
        const FRONTMATTER_SCAN_BYTES: u64 = 8 * 1024;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let mut file = root.open_with(name, &options).ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut head = Vec::new();
        (&mut file)
            .take(FRONTMATTER_SCAN_BYTES)
            .read_to_end(&mut head)
            .ok()?;
        // Lossy, deliberately. The cap can land mid-character -- any page with
        // multi-byte prose past 8 KiB will eventually split one -- and
        // `read_to_string` fails the entire read when it does. That failure is
        // silent and fail-OPEN: the file drops out of the scan, so a page
        // `state.json` forgot keeps a readable `.md` through the eviction that
        // exists to remove it. Frontmatter sits at the head and the key is
        // ASCII, so a replacement character in the truncated tail costs nothing.
        let head = String::from_utf8_lossy(&head);
        // Frontmatter only. A body line that happens to start with `origin_id:`
        // is prose, not a claim, so the search stops at the closing delimiter.
        let mut lines = head.lines();
        if lines.next() != Some("---") {
            return None;
        }
        lines
            .take_while(|line| *line != "---")
            .find_map(|line| line.strip_prefix("origin_id:"))
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
    }

    /// Delete the files, under one lock, loading and saving state once.
    ///
    /// The removal has to work for a page `state.json` does not know about --
    /// that is the whole point of scanning the directory. `remove_page` returns
    /// `Ok(())` without touching the disk when the ID is absent from state, so
    /// routing an unknown page through it would report a clean eviction while
    /// leaving the file exactly where it was. Worse than the bug it replaced.
    fn evict_projected_pages(
        &self,
        guard: &crate::page_projection_tracker::PageProjectionWriteGuard,
        page_ids: &[String],
    ) -> Result<usize, WenlanError> {
        self.validate_guard(guard)?;
        let projected = self.projected_files()?;
        KnowledgeProjectionWrite::with_projection_capabilities(&self.path, |capabilities| {
            let mut state = folders::read_state_strict(&capabilities.wenlan)?;
            let mut manifest =
                crate::export::provenance::StubManifest::load_from(&capabilities.root);
            let mut removed = 0usize;
            let mut stuck: Vec<&str> = Vec::new();
            // Named in the error too. A partial eviction saves state and moves
            // files for the pages that succeeded, but the caller aborts the
            // ceremony and leaves the generation where it was -- so "it failed"
            // is true and "nothing moved" is not. An operator who is only told
            // which pages STAYED cannot find the ones that left.
            let mut evicted: Vec<&str> = Vec::new();

            // Tracked apart from `removed` because a page can need the state
            // written without any file moving. Gating the save on `removed`
            // alone discarded the stale-entry cleanup below whenever EVERY
            // evicted page was file-less -- the one case that branch exists for.
            let mut state_dirty = false;
            for page_id in page_ids {
                let files = projected.get(page_id).map(Vec::as_slice).unwrap_or(&[]);
                if files.is_empty() {
                    // Known to `state.json` a moment ago and to nothing now, or
                    // an entry with no file. Nothing on disk to evict.
                    if state.pages.remove(page_id).is_some() {
                        state_dirty = true;
                    }
                    manifest.forget_page(page_id);
                    continue;
                }
                // Every file, not the first one that matches. A page with two
                // copies loses both or the eviction did not happen.
                let mut moved = 0usize;
                for filename in files {
                    match Self::archive_projected_file(&capabilities.root, filename) {
                        Ok(()) => moved += 1,
                        Err(error) => {
                            // Do not abort. This is a disclosure boundary, and
                            // the cost of stopping is that every page after this
                            // one keeps a file `wenlan pages` can read -- so
                            // maximum removal beats a clean exit.
                            log::error!(
                                "[truth] projection invariant could not archive page {page_id} \
                                 ({filename}), its file is still readable by `wenlan pages`: \
                                 {error}"
                            );
                        }
                    }
                }
                if moved == files.len() {
                    // Only forget the page once its files are actually gone.
                    // A retained entry is what makes the next pass retry.
                    state.pages.remove(page_id);
                    manifest.forget_page(page_id);
                    state_dirty = true;
                    removed += 1;
                    evicted.push(page_id);
                } else {
                    stuck.push(page_id);
                }
            }

            if state_dirty {
                self.save_state_cap(&capabilities.wenlan, &state)?;
                let _ = manifest.save_to(&capabilities.root);
                let _ =
                    crate::export::provenance::gc_orphan_stubs_in(&capabilities.root, &manifest);
                if let Err(e) = Self::regenerate_index_cap(capabilities, &state) {
                    log::warn!("[knowledge] index.md regeneration failed: {e}");
                }
            }
            if !stuck.is_empty() {
                // Loud at the caller too, not only in the per-page log lines:
                // the count is what a health check can act on.
                //
                // Both lists, because the caller aborts the ceremony on this
                // error and leaves the generation unchanged. The pages named in
                // `evicted` are already in archive/ and already gone from
                // `state.json`, and at generation 0 nothing re-projects them --
                // so an error that reported only `stuck` would say the ceremony
                // did nothing while some of the vault had already moved.
                let moved = if evicted.is_empty() {
                    "none".to_string()
                } else {
                    evicted.join(", ")
                };
                return Err(WenlanError::Conflict(format!(
                    "projection invariant evicted {removed} page(s) but {} could not be removed \
                     and remain readable on disk: {}. Already moved into archive/ (the \
                     generation did NOT advance, so these do not come back on their own): {moved}",
                    stuck.len(),
                    stuck.join(", ")
                )));
            }
            Ok(removed)
        })
    }

    /// Move one projected file into `archive/`, refusing anything that is not a
    /// plain file.
    ///
    /// A move, not an unlink. Hiding a page is a retrieval decision -- it says
    /// the prose is not backed by its evidence -- and that does not license
    /// destroying a file the person may have edited by hand, in a directory that
    /// is very often their own vault. The database can rebuild a page; it cannot
    /// rebuild what someone typed into the projection.
    ///
    /// `archive/` sits in the projection root in plain sight, and it works
    /// because `wenlan pages` reads one directory level and `.md` files only:
    /// an archived page leaves Wenlan's own reader, which is what the cutover
    /// is about, while staying somewhere a human can find it. It also keeps
    /// itself out of the recursive projection scan for the same reason a
    /// directory has no `.md` extension, so an archived page is not rediscovered
    /// and re-archived on the next pass.
    ///
    /// Same guarantees `remove_page` gives: no symlink is followed, and a
    /// directory or special file where a page was expected is a conflict rather
    /// than something to move. Already-absent is success -- the invariant cares
    /// that the file is gone from the projection root, not that this pass is the
    /// one that moved it. A file being moved OFF a reserved OKF name does not
    /// carry that name into `archive/`, which is a level of the hierarchy too;
    /// [`crate::export::archive::archive_file_from`] owns that rule.
    fn archive_projected_file(root: &Dir, filename: &str) -> Result<(), WenlanError> {
        let (directory, name) = folders::page_parent(root, filename)?;
        crate::export::archive::archive_file_from(root, &directory, &name)
    }

    fn validate_guard(
        &self,
        guard: &crate::page_projection_tracker::PageProjectionWriteGuard,
    ) -> Result<(), WenlanError> {
        if guard.belongs_to(&self.tracker) {
            Ok(())
        } else {
            Err(WenlanError::VectorDb(
                "page projection write guard does not own this writer".to_string(),
            ))
        }
    }

    fn load_state(&self) -> KnowledgeState {
        (|| {
            let root = folders::read_root(&self.path)?;
            let control = root.open_dir_nofollow(".wenlan")?;
            folders::read_state_strict(&control)
        })()
        .unwrap_or_default()
    }

    #[cfg(test)]
    fn save_state(&self, state: &KnowledgeState) -> Result<(), WenlanError> {
        let state_path = self.path.join(".wenlan/state.json");
        let data = serde_json::to_string_pretty(state)?;
        std::fs::write(&state_path, data)?;
        Ok(())
    }

    fn save_state_cap(&self, wenlan: &Dir, state: &KnowledgeState) -> Result<(), WenlanError> {
        let data = serde_json::to_vec_pretty(state)?;
        let sequence = PROJECTION_STATE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary = format!(".projection-state-{}-{sequence}.tmp", std::process::id());
        write_page_atomically_nofollow(wenlan, "state.json", &temporary, &data)
    }

    /// Fallback for a legacy `state.json` entry with no `title` field: read
    /// title/description/space straight from the file's frontmatter, the way
    /// `regenerate_index_cap` always used to. `None` means the file is
    /// missing or its frontmatter can't be read this pass -- best-effort, the
    /// same as the rest of index regeneration.
    fn read_index_fields_from_file(
        root: &Dir,
        filename: &str,
    ) -> Option<(String, String, Option<String>)> {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let (directory, name) = folders::page_parent(root, filename).ok()?;
        let mut file = directory.open_with(&name, &options).ok()?;
        let mut head = Vec::new();
        (&mut file)
            .take(INDEX_FRONTMATTER_SCAN_BYTES)
            .read_to_end(&mut head)
            .ok()?;
        // Lossy: the scan cap can land mid-character past 8 KiB, and
        // frontmatter keys/values of interest here sit at the head.
        let head = String::from_utf8_lossy(&head);
        let (fm, _body) = crate::sources::obsidian::extract_frontmatter(&head);
        let title = fm.get_str("title").unwrap_or(filename).to_string();
        let description = fm.get_str("description").unwrap_or("").to_string();
        let space = fm.get_str("space").map(str::to_string);
        Some((title, description, space))
    }

    /// After a page moves off a reserved name (`index.md`, `log.md`), clear
    /// the copy it left there, so the OKF index can take `index.md` back and
    /// no concept document is left at either. `state` recorded the page at
    /// that file, so
    /// a regular file there whose `origin_id` is this page is Wenlan's own
    /// projection. Anything else is kept: a note the user put there since, a
    /// symlink, or a file this pass cannot read. Ownership comes from `state`,
    /// not from the file alone, because a user's note made by copying a page
    /// file carries that page's `origin_id` too.
    ///
    /// The copy is archived rather than unlinked. `origin_id` says Wenlan
    /// wrote the file; it does not say the bytes are still Wenlan's, because
    /// an edit made in place in the vault leaves the frontmatter alone. The
    /// database can rebuild the page at its new name, so nothing is lost by
    /// moving the old file aside, and [`Self::archive_projected_file`] already
    /// makes the argument for why that asymmetry decides it. This fires once
    /// per page, the first time it leaves a reserved name, so `archive/` gains
    /// one file for a migration that happens to at most the page named "Index"
    /// and the page named "Log".
    fn remove_reserved_name_copy_left_by(root: &Dir, file: &str, page_id: &str) {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let Ok((directory, name)) = folders::page_parent(root, file) else {
            return;
        };
        let Ok(mut handle) = directory.open_with(&name, &options) else {
            return;
        };
        if !handle.metadata().is_ok_and(|meta| meta.is_file()) {
            return;
        }
        let mut head = Vec::new();
        if (&mut handle)
            .take(INDEX_FRONTMATTER_SCAN_BYTES)
            .read_to_end(&mut head)
            .is_err()
        {
            return;
        }
        drop(handle);
        let head = String::from_utf8_lossy(&head);
        let (fm, _body) = crate::sources::obsidian::extract_frontmatter(&head);
        if fm.get_str("origin_id").map(str::trim) != Some(page_id) {
            return;
        }
        if let Err(e) = Self::archive_projected_file(root, file) {
            log::warn!("[knowledge] could not archive {file} after page {page_id} moved: {e}");
        }
    }

    /// Whether the projection may write `index.md`: the name is free, or the
    /// file there is one this projection could have produced -- frontmatter
    /// whose only key is `okf_version`, AND a body of nothing but the headings
    /// and entry lines the renderer emits. Anything else is left alone: a note
    /// the user made at `index.md` (Obsidian users often keep a home note
    /// there), a page still projected there, a symlink, a directory, or a file
    /// this pass cannot read.
    ///
    /// The frontmatter test alone was not enough, and this is the same mistake
    /// the rest of the arc exists to remove: `okf_version` on its own is the
    /// STANDARD OKF root-index shape, not a Wenlan signature, so a person who
    /// hand-wrote a conforming root index had it overwritten. Prose typed into
    /// Wenlan's own generated index was overwritten for the same reason. The
    /// body is the only evidence available without a recorded digest.
    ///
    /// Residual, and deliberate: an edit that still looks machine-generated
    /// (a reordered or deleted entry line) is still replaced. Closing that
    /// needs a digest of the last written index in `state.json`, which is a
    /// schema change and a second state save on every page write.
    fn index_file_is_replaceable(root: &Dir) -> bool {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let mut file = match root.open_with(INDEX_FILE, &options) {
            Ok(file) => file,
            Err(e) => return e.kind() == std::io::ErrorKind::NotFound,
        };
        let mut head = Vec::new();
        // One byte past the limit, so a file too big to read whole is told
        // apart from one that exactly fills the budget. An index this pass
        // cannot read whole is an index it cannot claim.
        let readable = (&mut file)
            .take(INDEX_OWNERSHIP_SCAN_BYTES + 1)
            .read_to_end(&mut head)
            .is_ok()
            && head.len() as u64 <= INDEX_OWNERSHIP_SCAN_BYTES;
        drop(file);
        if !readable {
            return false;
        }
        let head = String::from_utf8_lossy(&head);
        let (fm, body) = crate::sources::obsidian::extract_frontmatter(&head);
        fm.fields.len() == 1 && fm.has("okf_version") && index_body_is_generated(body)
    }

    /// Regenerate the OKF `index.md` root document from the frontmatter of the
    /// files currently projected (per `state`), grouped by `space` (`##
    /// Unfiled` for pages without one), each section's entries sorted by
    /// title. Read from the projected files rather than a `Page` list because
    /// `write_page`/`remove_page` only ever have the one page they are
    /// touching in hand; `reconcile` has the full set but shares this helper
    /// for one code path.
    ///
    /// Best-effort like the stub/manifest projection beside it: a page whose
    /// frontmatter can't be read this pass is dropped from the index rather
    /// than failing the write that triggered the regeneration.
    fn regenerate_index_cap(
        capabilities: &ProjectionCapabilities,
        state: &KnowledgeState,
    ) -> Result<(), WenlanError> {
        // De-duped and sorted by filename: nothing on disk enforces one state
        // entry per file (a sync conflict can leave two ids pointing at one
        // file), and `HashMap` iteration order is not stable.
        let mut entries: Vec<&PageFileState> = state.pages.values().collect();
        entries.sort_unstable_by(|a, b| a.file.cmp(&b.file));
        entries.dedup_by(|a, b| a.file == b.file);

        // Raw `(title, file, description, space)` rows; sanitizing, grouping
        // and sorting live in `render_index_for_entries` so the OKF bundle
        // shares them.
        let mut raw_entries: Vec<(String, String, String, Option<String>)> = Vec::new();

        for state_entry in entries {
            // `title: Some(_)` means this entry was written by this build and
            // already carries what the index needs — no file open/parse. A
            // `None` title is a `state.json` entry from before these fields
            // existed, so fall back to the file-head read this function used
            // to always do, for that one file only.
            let (title, description, space) = match &state_entry.title {
                Some(title) => (
                    title.clone(),
                    state_entry.description.clone().unwrap_or_default(),
                    state_entry.space.clone(),
                ),
                None => {
                    match Self::read_index_fields_from_file(&capabilities.root, &state_entry.file) {
                        Some(fields) => fields,
                        None => continue,
                    }
                }
            };
            raw_entries.push((title, state_entry.file.clone(), description, space));
        }
        // The projection links at the vault root (`/file`); the OKF bundle
        // reuses this renderer with `/pages/`.
        let bytes = render_index_for_entries(raw_entries, "/");

        if !Self::index_file_is_replaceable(&capabilities.root) {
            // Warn once per process: this runs on every page write, and the
            // user's note staying put is the intended outcome, not a fault.
            if !USER_INDEX_SKIP_WARNED.swap(true, Ordering::Relaxed) {
                log::warn!(
                    "[knowledge] leaving {INDEX_FILE} alone: it is not the index Wenlan \
                     generates (move or rename it to let Wenlan write the OKF index)"
                );
            }
            return Ok(());
        }

        let temp_filename = format!(
            ".index.{}.{}.tmp",
            std::process::id(),
            TEMP_FILE_SEQ.fetch_add(1, Ordering::Relaxed)
        );
        write_page_atomically_nofollow(
            &capabilities.root,
            INDEX_FILE,
            &temp_filename,
            bytes.as_bytes(),
        )
    }
}

/// Collapse `\r`/`\n` in an `index.md` field to a single space and squeeze
/// runs of whitespace, so a title/description/space that carries a newline
/// (typed by a user, or from an offline edit) can't split `index.md` into
/// extra lines or bullets.
pub(crate) fn sanitize_index_field(s: &str) -> String {
    s.replace(['\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Escape `[` and `]` in text used as markdown link text, so a title
/// containing a bracket can't break the `[title](...)` link syntax.
pub(crate) fn escape_index_link_text(s: &str) -> String {
    s.replace('[', "\\[").replace(']', "\\]")
}

/// Whether every non-blank line of an `index.md` body is one the index
/// renderer could have emitted: a `## <space>` heading or a
/// `* [title](/file)` entry line. Prose, a different heading level, a
/// checklist, a table, a wikilink -- anything a person would add -- fails, and
/// the caller then leaves the file alone.
///
/// Deliberately a shape test, not a re-render: the index on disk describes the
/// state BEFORE the write that triggered this pass, so it is expected to
/// differ from what the pass is about to write. Only its shape is invariant.
///
/// The prefixes alone are not enough. `* [` is also how Markdown checklists
/// start, so `* [ ] buy milk` in somebody's home note passed a `starts_with`
/// test and their list was replaced by the generated index. An entry line has
/// to be shaped like one: `* [title](/file.md)`, optionally ` - description`.
///
/// Headings-only is rejected too. Every generated index that has a heading has
/// at least one entry under it, so a file of bare headings is somebody's
/// outline, not ours.
fn index_body_is_generated(body: &str) -> bool {
    let mut entries = 0usize;
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        if line.starts_with("## ") {
            continue;
        }
        if !index_line_is_entry(line) {
            return false;
        }
        entries += 1;
    }
    // An index with no pages in it has an empty body and stays ours.
    entries > 0 || body.trim().is_empty()
}

/// One rendered entry line: `* [title](/file.md)`, optionally ` - description`.
/// Anything this cannot parse is treated as not ours, which is the safe
/// direction: the file is left alone.
fn index_line_is_entry(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("* [") else {
        return false;
    };
    let Some(link) = rest.find("](") else {
        return false;
    };
    let after = &rest[link + 2..];
    let Some(close) = after.find(')') else {
        return false;
    };
    let tail = &after[close + 1..];
    tail.is_empty() || tail.starts_with(" - ")
}

/// One index line. The ` - ` separator is written only when the page has a
/// description: a page with an empty summary otherwise rendered as
/// `* [Title](/page.md) - `, a separator pointing at nothing, in every
/// projection index and every exported bundle.
fn index_entry_line(title: &str, link_prefix: &str, filename: &str, description: &str) -> String {
    let title = escape_index_link_text(title);
    let filename = filename
        .split('/')
        .map(|component| {
            component
                .as_bytes()
                .iter()
                .map(|byte| {
                    if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'.' | b'_' | b'~') {
                        (*byte as char).to_string()
                    } else {
                        format!("%{byte:02X}")
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/");
    if description.is_empty() {
        format!("* [{title}]({link_prefix}{filename})\n")
    } else {
        format!("* [{title}]({link_prefix}{filename}) - {description}\n")
    }
}

/// Pure OKF `index.md` renderer shared by the projection and the OKF export
/// bundle. Entries must already be sanitized and title-sorted within their
/// group; `link_prefix` is `"/"` for the projection (links at the vault
/// root) and `"/pages/"` for the bundle.
pub(crate) fn render_index_markdown(
    by_space: &std::collections::BTreeMap<String, Vec<(String, String, String)>>,
    unfiled: &[(String, String, String)],
    link_prefix: &str,
) -> String {
    let mut out = String::from("---\nokf_version: \"0.2\"\n---\n\n");
    for (space, entries) in by_space {
        out.push_str(&format!("## {space}\n\n"));
        for (title, filename, description) in entries {
            out.push_str(&index_entry_line(title, link_prefix, filename, description));
        }
        out.push('\n');
    }
    if !unfiled.is_empty() {
        out.push_str("## Unfiled\n\n");
        for (title, filename, description) in unfiled {
            out.push_str(&index_entry_line(title, link_prefix, filename, description));
        }
        out.push('\n');
    }
    format!("{}\n", out.trim_end())
}

/// Sanitize, group by space (`## Unfiled` last) and title-sort raw
/// `(title, file, description, space)` rows, then render them with
/// [`render_index_markdown`]. The projection builds its rows from
/// `state.json`; the OKF bundle builds them from its page list — both flow
/// through this one grouping so the two indexes cannot drift.
pub(crate) fn render_index_for_entries(
    rows: Vec<(String, String, String, Option<String>)>,
    link_prefix: &str,
) -> String {
    let mut by_space: std::collections::BTreeMap<String, Vec<(String, String, String)>> =
        std::collections::BTreeMap::new();
    let mut unfiled: Vec<(String, String, String)> = Vec::new();
    for (title, file, description, space) in rows {
        let entry = (
            sanitize_index_field(&title),
            file,
            sanitize_index_field(&description),
        );
        match space.map(|s| sanitize_index_field(&s)) {
            Some(space) if !space.is_empty() => {
                by_space.entry(space).or_default().push(entry);
            }
            _ => unfiled.push(entry),
        }
    }
    for entries in by_space.values_mut() {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
    }
    unfiled.sort_by(|a, b| a.0.cmp(&b.0));
    render_index_markdown(&by_space, &unfiled, link_prefix)
}

pub struct KnowledgeProjectionWriteRef<'writer> {
    writer: &'writer KnowledgeWriter,
    guard: crate::page_projection_tracker::PageProjectionWriteGuard,
}

impl KnowledgeProjectionWriteRef<'_> {
    pub fn write_page(&self, page: &Page) -> Result<String, WenlanError> {
        self.writer.write_page(&self.guard, page)
    }

    pub fn write_page_in_folder(
        &self,
        page: &Page,
        initial_folder: Option<&str>,
    ) -> Result<String, WenlanError> {
        self.writer
            .write_page_in_folder(&self.guard, page, initial_folder)
    }

    /// Project a page only if an automatic reader may see it.
    ///
    /// Borrowed-writer counterpart to [`KnowledgeProjectionWrite::write_page_gated`]
    /// — same contract, for callers holding a [`KnowledgeProjectionWriteRef`]
    /// instead of an owned [`KnowledgeProjectionWrite`]. `Ok(None)` means the
    /// page was not projected, and that is the contract working rather than a
    /// failure -- callers log it and carry on.
    pub async fn write_page_gated(
        &self,
        database: &crate::db::MemoryDB,
        page: &Page,
    ) -> Result<Option<String>, WenlanError> {
        let Some(permit) = crate::truth_adapter::page_write_permit(database, &page.id).await?
        else {
            log::info!(
                "[truth] not projecting page {} — an automatic reader may not see it, and \
                 `wenlan pages` reads this directory with no gate in front of it",
                page.id
            );
            return Ok(None);
        };
        let folder = database.page_initial_folder_path(&page.id).await?;
        check_permit(&permit, page)?;
        self.writer
            .write_page_in_folder(&self.guard, page, Some(&folder))
            .map(Some)
    }

    /// Project a page against a permit already in hand.
    ///
    /// Borrowed-writer counterpart to
    /// [`KnowledgeProjectionWrite::write_page_permitted`]. The permit's page is
    /// checked against the page being written: a permit for page A is not
    /// permission to write page B.
    pub fn write_page_permitted(
        &self,
        permit: &crate::truth_adapter::PagePermit,
        page: &Page,
    ) -> Result<String, WenlanError> {
        check_permit(permit, page)?;
        self.write_page(page)
    }

    pub fn remove_page(&self, page_id: &str) -> Result<(), WenlanError> {
        self.writer.remove_page(&self.guard, page_id)
    }
}

/// Check a write permit against the page it authorizes, without touching the
/// filesystem. Shared by [`KnowledgeProjectionWrite::write_page_permitted`]
/// and [`KnowledgeProjectionWriteRef::write_page_permitted`] so the mismatch
/// message can't drift between the two owning forms of a projection write.
fn check_permit(permit: &crate::truth_adapter::PagePermit, page: &Page) -> Result<(), WenlanError> {
    if permit.page_id() != page.id {
        return Err(WenlanError::Validation(format!(
            "page projection permit is for {}, not {}",
            permit.page_id(),
            page.id
        )));
    }
    Ok(())
}

/// One page the cutover would stop projecting, and the file that costs.
///
/// `files` is empty when `state.json` names a page whose `.md` is already gone.
/// Reporting it anyway is the point: the entry is still there to be resurrected
/// by the next write, so it is part of what the ceremony cleans up even though
/// no bytes move.
///
/// It is a list rather than one name because nothing on disk enforces one file
/// per page -- see [`KnowledgeWriter::projected_files`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutoverEviction {
    pub page_id: String,
    pub files: Vec<String>,
}

/// What advancing to a generation would do to the projection directory.
///
/// Produced by [`KnowledgeWriter::plan_truth_cutover`] and re-produced inside
/// [`KnowledgeWriter::run_truth_cutover`], which refuses when the two digests
/// differ. The operator reads the eviction list; the machine reads the digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutoverPlan {
    pub generation: i64,
    /// Every page the directory currently accounts for, evicted or not.
    pub projected: usize,
    pub evictions: Vec<CutoverEviction>,
    pub digest: String,
}

pub struct KnowledgeProjectionWrite {
    writer: KnowledgeWriter,
    guard: crate::page_projection_tracker::PageProjectionWriteGuard,
}

pub(crate) struct LockedRepairProjection<'a> {
    write: &'a KnowledgeProjectionWrite,
    capabilities: &'a ProjectionCapabilities,
}

pub(crate) struct LockedProjection<'a> {
    capabilities: &'a ProjectionCapabilities,
}

/// Repair-only projection ownership that can safely remain alive across
/// awaited database work. It owns the tracker guard, capability roots, and
/// advisory file lock; it never borrows `PROJECTION_WRITE_LOCK`.
pub(crate) struct OwnedRepairProjectionSession {
    write: KnowledgeProjectionWrite,
    capabilities: ProjectionCapabilities,
}

pub(crate) struct RepairReadBudget {
    remaining: u64,
}

impl RepairReadBudget {
    pub(crate) fn new() -> Self {
        Self {
            remaining: REPAIR_CAPABILITY_READ_MAX_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProjectionFileIdentity {
    device: u64,
    inode: u64,
}

fn projection_file_identity(
    metadata: &cap_std::fs::Metadata,
) -> Result<ProjectionFileIdentity, WenlanError> {
    use cap_fs_ext::MetadataExt as _;
    Ok(ProjectionFileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

impl LockedProjection<'_> {
    pub(crate) fn scan_page_root_controlled(
        &self,
        include_body_digests: bool,
        control: &crate::lint::pages::fs::PageScanControl,
    ) -> Result<crate::lint::pages::fs::PageScan, WenlanError> {
        crate::lint::pages::fs::scan_page_root_capability_controlled(
            &self.capabilities.root,
            include_body_digests,
            control,
        )
        .map_err(|error| WenlanError::Validation(format!("repair projection scan: {error}")))
    }

    pub(crate) fn capture_stale_page_projection_current(
        &self,
        page_id: &str,
        source_path: &str,
        quarantine_path: &str,
    ) -> Result<crate::repair::StoredRollbackArtifact, WenlanError> {
        crate::repair::capture_stale_page_projection_current_locked(
            self,
            page_id,
            source_path,
            quarantine_path,
        )
    }

    #[cfg(test)]
    pub(crate) fn read_relative_regular_nofollow(
        &self,
        relative: &str,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>, WenlanError> {
        let mut budget = RepairReadBudget {
            remaining: max_bytes,
        };
        self.read_relative_regular_nofollow_inner(relative, &mut budget, || Ok(()))
    }

    #[cfg(test)]
    pub(crate) fn read_relative_regular_nofollow_with_after_metadata<F>(
        &self,
        relative: &str,
        max_bytes: u64,
        after_metadata: F,
    ) -> Result<Option<Vec<u8>>, WenlanError>
    where
        F: FnOnce() -> Result<(), WenlanError>,
    {
        let mut budget = RepairReadBudget {
            remaining: max_bytes,
        };
        self.read_relative_regular_nofollow_inner(relative, &mut budget, after_metadata)
    }

    pub(crate) fn read_relative_regular_nofollow_budget(
        &self,
        relative: &str,
        budget: &mut RepairReadBudget,
    ) -> Result<Option<Vec<u8>>, WenlanError> {
        self.read_relative_regular_nofollow_inner(relative, budget, || Ok(()))
    }

    fn read_relative_regular_nofollow_inner<F>(
        &self,
        relative: &str,
        budget: &mut RepairReadBudget,
        after_metadata: F,
    ) -> Result<Option<Vec<u8>>, WenlanError>
    where
        F: FnOnce() -> Result<(), WenlanError>,
    {
        crate::repair::validate_projection_relative_path(relative)?;
        let relative = Path::new(relative);
        let mut components = relative.components().peekable();
        let mut directory = self.capabilities.root.try_clone()?;
        while let Some(component) = components.next() {
            let Component::Normal(component) = component else {
                return Err(WenlanError::Conflict("repair_target_stale".to_string()));
            };
            if components.peek().is_none() {
                return read_optional_regular_nofollow_bounded(
                    &directory,
                    component,
                    budget,
                    after_metadata,
                );
            }
            directory = match directory.open_dir_nofollow(Path::new(component)) {
                Ok(directory) => directory,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(WenlanError::Io(error)),
            };
        }
        Err(WenlanError::Conflict("repair_target_stale".to_string()))
    }

    pub(crate) fn orphaned_baseline_nofollow(
        &self,
        budget: &mut RepairReadBudget,
    ) -> Result<Option<Vec<(String, String)>>, WenlanError> {
        match self.capabilities.wenlan.symlink_metadata("orphaned") {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                let orphaned = self.capabilities.wenlan.open_dir_nofollow("orphaned")?;
                ensure_orphaned_private(&orphaned)?;
                Ok(Some(orphaned_baseline_nofollow(&orphaned, None, budget)?))
            }
            Ok(_) => Err(WenlanError::Conflict("repair_target_stale".to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(WenlanError::Io(error)),
        }
    }

    pub(crate) fn capture_rename_page_projection(
        &self,
        page_id: &str,
    ) -> Result<(String, Vec<wenlan_types::repair::RepairRollbackFileEntry>), WenlanError> {
        let mut budget = RepairReadBudget::new();
        let state_bytes = self
            .read_relative_regular_nofollow_budget(".wenlan/state.json", &mut budget)?
            .ok_or_else(|| WenlanError::Conflict("repair_target_stale".to_string()))?;
        let state = serde_json::from_slice::<KnowledgeState>(&state_bytes)
            .map_err(|_| WenlanError::Conflict("repair_target_stale".to_string()))?;
        if state.schema_version != KNOWLEDGE_STATE_SCHEMA_V2 {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        let target_path = state
            .pages
            .get(page_id)
            .map(|entry| entry.file.clone())
            .ok_or_else(|| WenlanError::Conflict("repair_target_stale".to_string()))?;
        let normalized = crate::lint::pages::fs::normalize_target_path(&target_path)
            .map_err(|_| WenlanError::Conflict("repair_target_stale".to_string()))?
            .as_str()
            .to_string();
        let target = Path::new(&normalized);
        if normalized != target_path
            || target.extension().and_then(|value| value.to_str()) != Some("md")
        {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        let target_bytes = self
            .read_relative_regular_nofollow_budget(&normalized, &mut budget)
            .map_err(|error| match error {
                WenlanError::Io(_) => WenlanError::Conflict("repair_target_stale".to_string()),
                other => other,
            })?
            .ok_or_else(|| WenlanError::Conflict("repair_target_stale".to_string()))?;
        let scan = self.scan_page_root_controlled(
            true,
            &crate::lint::pages::fs::PageScanControl::with_timeout(std::time::Duration::from_secs(
                30,
            )),
        )?;
        if matches!(
            scan.raw_state.kind,
            crate::lint::pages::state::RawStateKind::Malformed
        ) {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        if !crate::lint::pages::state_checks::projection_target_is_exclusive_page_markdown(
            &scan,
            page_id,
            &normalized,
        ) {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        let entries = vec![
            wenlan_types::repair::RepairRollbackFileEntry::file(
                ".wenlan/state.json".to_string(),
                state_bytes,
            )
            .map_err(|error| WenlanError::Validation(error.to_string()))?,
            wenlan_types::repair::RepairRollbackFileEntry::file(normalized.clone(), target_bytes)
                .map_err(|error| WenlanError::Validation(error.to_string()))?,
        ];
        Ok((normalized, entries))
    }

    /// Capture a page projection when one is already indexed in state.json.
    /// A missing state file or a state file with no entry for this page means
    /// there is no projection to update; malformed state and broken indexed
    /// files remain errors so a rename cannot silently overwrite user data.
    pub(crate) fn capture_optional_page_projection(
        &self,
        page_id: &str,
    ) -> Result<Option<(String, Vec<wenlan_types::repair::RepairRollbackFileEntry>)>, WenlanError>
    {
        let mut budget = RepairReadBudget::new();
        let Some(state_bytes) =
            self.read_relative_regular_nofollow_budget(".wenlan/state.json", &mut budget)?
        else {
            return Ok(None);
        };
        let state = parse_rename_projection_state(&state_bytes)?;
        if !state.pages.contains_key(page_id) {
            return Ok(None);
        }
        self.capture_rename_page_projection(page_id).map(Some)
    }

    /// Capture and prove that an indexed projection is still the canonical
    /// version recorded by the DB. A syntactically safe path alone does not
    /// authorize overwriting a user's unsynced Markdown edits.
    pub(crate) fn capture_checked_page_projection(
        &self,
        page: &Page,
    ) -> Result<Option<(String, Vec<wenlan_types::repair::RepairRollbackFileEntry>)>, WenlanError>
    {
        let Some((path, entries)) = self.capture_optional_page_projection(&page.id)? else {
            return Ok(None);
        };
        if entries.len() != 2 {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        let state_bytes = hex::decode(entries[0].content_hex())
            .map_err(|_| WenlanError::Conflict("repair_target_stale".to_string()))?;
        let state: serde_json::Value = serde_json::from_slice(&state_bytes)
            .map_err(|_| WenlanError::Conflict("repair_target_stale".to_string()))?;
        let stored_page = state
            .get("pages")
            .and_then(serde_json::Value::as_object)
            .and_then(|pages| pages.get(&page.id));
        let expected_state = page_file_state_value(page, &path);
        let target_bytes = hex::decode(entries[1].content_hex())
            .map_err(|_| WenlanError::Conflict("repair_target_stale".to_string()))?;
        let expected_markdown = render_markdown_for(page);
        let state_matches = stored_page.is_some_and(|stored| {
            if stored == &expected_state {
                return true;
            }
            // `content_sha256` was added after older PageFileState entries
            // were already on disk. A missing hash is compatible only when
            // the actual projection bytes still match the current DB render;
            // present hashes and every other known field must match exactly.
            let mut expected_without_hash = expected_state.clone();
            expected_without_hash
                .as_object_mut()
                .expect("PageFileState serializes as an object")
                .remove("content_sha256");
            stored.get("content_sha256").is_none() && stored == &expected_without_hash
        });
        if !state_matches || target_bytes.as_slice() != expected_markdown.as_bytes() {
            return Err(WenlanError::Conflict(
                "page projection contains unsynced edits".to_string(),
            ));
        }
        Ok(Some((path, entries)))
    }

    fn restore_rename_page_projection(
        &self,
        target_path: &str,
        entries: &[wenlan_types::repair::RepairRollbackFileEntry],
    ) -> Result<(), WenlanError> {
        self.restore_rename_page_projection_inner(target_path, entries, || Ok(()), || Ok(()))
    }

    #[cfg(test)]
    fn restore_rename_page_projection_with_checkpoints<F, G>(
        &self,
        target_path: &str,
        entries: &[wenlan_types::repair::RepairRollbackFileEntry],
        before_target_restore: F,
        after_target_restore: G,
    ) -> Result<(), WenlanError>
    where
        F: FnOnce() -> Result<(), WenlanError>,
        G: FnOnce() -> Result<(), WenlanError>,
    {
        self.restore_rename_page_projection_inner(
            target_path,
            entries,
            before_target_restore,
            after_target_restore,
        )
    }

    fn restore_rename_page_projection_inner<F, G>(
        &self,
        target_path: &str,
        entries: &[wenlan_types::repair::RepairRollbackFileEntry],
        before_target_restore: F,
        after_target_restore: G,
    ) -> Result<(), WenlanError>
    where
        F: FnOnce() -> Result<(), WenlanError>,
        G: FnOnce() -> Result<(), WenlanError>,
    {
        if entries.len() != 2
            || entries[0].relative_path() != ".wenlan/state.json"
            || entries[1].relative_path() != target_path
            || entries
                .iter()
                .any(|entry| entry.kind() != wenlan_types::repair::RepairRollbackFileKind::File)
        {
            return Err(WenlanError::Validation(
                "repair_projection_rollback_invalid".to_string(),
            ));
        }
        let state_bytes = hex::decode(entries[0].content_hex()).map_err(|_| {
            WenlanError::Validation("repair_projection_rollback_invalid".to_string())
        })?;
        let target_bytes = hex::decode(entries[1].content_hex()).map_err(|_| {
            WenlanError::Validation("repair_projection_rollback_invalid".to_string())
        })?;
        before_target_restore()?;
        let (target_directory, target_name) =
            folders::page_parent(&self.capabilities.root, target_path)?;
        write_regular_nofollow(&target_directory, &target_name, &target_bytes)?;
        after_target_restore()?;
        let mut budget = RepairReadBudget::new();
        let (_, state_mode) = read_state_nofollow(&self.capabilities.wenlan, &mut budget)?;
        write_state_atomically(&self.capabilities.wenlan, &state_bytes, state_mode)?;
        #[cfg(unix)]
        {
            sync_dir_capability(&self.capabilities.wenlan)?;
            sync_dir_capability(&self.capabilities.root)?;
        }
        Ok(())
    }
}

impl LockedRepairProjection<'_> {
    fn locked_projection(&self) -> LockedProjection<'_> {
        LockedProjection {
            capabilities: self.capabilities,
        }
    }

    /// Refresh the generated root index after a committed title write. This
    /// reuses the same no-follow capability and ownership check as page writes.
    pub(crate) fn refresh_index(&self) -> Result<(), WenlanError> {
        let state = folders::read_state_strict(&self.capabilities.wenlan)?;
        KnowledgeWriter::regenerate_index_cap(self.capabilities, &state)
    }

    pub(crate) fn scan_page_root_controlled(
        &self,
        include_body_digests: bool,
        control: &crate::lint::pages::fs::PageScanControl,
    ) -> Result<crate::lint::pages::fs::PageScan, WenlanError> {
        self.locked_projection()
            .scan_page_root_controlled(include_body_digests, control)
    }

    pub(crate) fn capture_stale_page_projection_current(
        &self,
        page_id: &str,
        source_path: &str,
        quarantine_path: &str,
    ) -> Result<crate::repair::StoredRollbackArtifact, WenlanError> {
        self.locked_projection()
            .capture_stale_page_projection_current(page_id, source_path, quarantine_path)
    }

    /// Project a page against a permit already in hand.
    ///
    /// Repair-lock counterpart to the two [`write_page_permitted`] forms above,
    /// sharing their permit/page check. It exists because the permit cannot be
    /// taken *here*: the repair path holds the connection mutex around this
    /// closure, and [`crate::truth_adapter::page_write_permit`] needs that same
    /// connection. The caller asks first, then hands the answer in.
    ///
    /// [`write_page_permitted`]: KnowledgeProjectionWrite::write_page_permitted
    pub(crate) fn write_page_permitted(
        &self,
        permit: &crate::truth_adapter::PagePermit,
        page: &Page,
    ) -> Result<String, WenlanError> {
        check_permit(permit, page)?;
        self.write.writer.write_page_with_lock_held(
            self.capabilities,
            &self.write.guard,
            page,
            true,
        )
    }

    /// The permitted form of [`Self::write_page_with_after_target_write`].
    ///
    /// The repair path renames a page's title and rewrites its `.md` in the same
    /// receipted step, which is a production page-prose write like any other and
    /// needs the same permit in front of it.
    #[cfg(test)]
    pub(crate) fn write_page_with_after_target_write_permitted<F>(
        &self,
        permit: &crate::truth_adapter::PagePermit,
        page: &Page,
        after_target_write: F,
    ) -> Result<String, WenlanError>
    where
        F: FnOnce() -> Result<(), WenlanError>,
    {
        check_permit(permit, page)?;
        self.write.writer.write_page_with_lock_held_and_hook(
            self.capabilities,
            &self.write.guard,
            page,
            after_target_write,
            true,
        )
    }

    pub(crate) fn capture_rename_page_projection(
        &self,
        page_id: &str,
    ) -> Result<(String, Vec<wenlan_types::repair::RepairRollbackFileEntry>), WenlanError> {
        self.locked_projection()
            .capture_rename_page_projection(page_id)
    }

    pub(crate) fn capture_checked_page_projection(
        &self,
        page: &Page,
    ) -> Result<Option<(String, Vec<wenlan_types::repair::RepairRollbackFileEntry>)>, WenlanError>
    {
        self.locked_projection()
            .capture_checked_page_projection(page)
    }

    pub(crate) fn restore_rename_page_projection(
        &self,
        target_path: &str,
        entries: &[wenlan_types::repair::RepairRollbackFileEntry],
    ) -> Result<(), WenlanError> {
        self.locked_projection()
            .restore_rename_page_projection(target_path, entries)
    }

    #[cfg(test)]
    pub(crate) fn restore_rename_page_projection_with_checkpoints<F, G>(
        &self,
        target_path: &str,
        entries: &[wenlan_types::repair::RepairRollbackFileEntry],
        before_target_restore: F,
        after_target_restore: G,
    ) -> Result<(), WenlanError>
    where
        F: FnOnce() -> Result<(), WenlanError>,
        G: FnOnce() -> Result<(), WenlanError>,
    {
        self.locked_projection()
            .restore_rename_page_projection_with_checkpoints(
                target_path,
                entries,
                before_target_restore,
                after_target_restore,
            )
    }

    #[cfg(test)]
    pub(crate) fn quarantine_stale_page(
        &self,
        page_id: &str,
        source_path: &str,
        quarantine_path: &str,
    ) -> Result<(), WenlanError> {
        let approved =
            self.capture_stale_page_projection_current(page_id, source_path, quarantine_path)?;
        self.pin_stale_page_projection(page_id, source_path, quarantine_path, &approved, page_id)?
            .quarantine()
    }

    pub(crate) fn pin_stale_page_projection<'lock>(
        &'lock self,
        page_id: &str,
        source_path: &str,
        quarantine_path: &str,
        approved: &crate::repair::StoredRollbackArtifact,
        stage_owner: &str,
    ) -> Result<PinnedStalePageProjection<'lock>, WenlanError> {
        crate::repair::validate_projection_relative_path(source_path)?;
        crate::repair::validate_projection_relative_path(quarantine_path)?;
        let (approved_source_path, approved_quarantine_path) =
            crate::repair::stale_page_projection_paths(approved)?;
        if approved.source_id != page_id
            || approved_source_path != source_path
            || approved_quarantine_path != quarantine_path
        {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        let approved_state_bytes = rollback_row_bytes(approved, ".wenlan/state.json")?;
        let approved_source_bytes = rollback_row_bytes(approved, source_path)?;
        let approved_quarantine = approved
            .rows
            .iter()
            .find(|row| row[0] == quarantine_path)
            .ok_or_else(|| {
                WenlanError::Validation("repair_projection_rollback_invalid".to_string())
            })?;
        if approved_quarantine[1] != "missing" || !approved_quarantine[2].is_empty() {
            return Err(WenlanError::Validation(
                "repair_projection_rollback_invalid".to_string(),
            ));
        }
        let approved_orphaned = crate::repair::stale_page_projection_orphaned_baseline(approved)?;
        let quarantine = Path::new(quarantine_path);
        if quarantine.parent() != Some(Path::new(".wenlan/orphaned")) {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        let quarantine_name = quarantine
            .file_name()
            .ok_or_else(|| WenlanError::Conflict("repair_target_stale".to_string()))?
            .to_os_string();
        let (source_parent, source_name) =
            open_relative_parent_nofollow(&self.capabilities.root, Path::new(source_path))?;
        let stage_name = projection_unlink_stage_name(stage_owner);

        let mut read_budget = RepairReadBudget::new();
        let (expected_state_bytes, state_mode) =
            read_state_nofollow(&self.capabilities.wenlan, &mut read_budget)?;
        if expected_state_bytes != approved_state_bytes {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        let next_state_bytes =
            crate::lint::pages::state::remove_unique_page_member(&expected_state_bytes, page_id)
                .map_err(|()| WenlanError::Conflict("repair_target_stale".to_string()))?;
        let (expected_source_bytes, expected_source_identity) =
            read_regular_identity_nofollow_bounded(&source_parent, &source_name, &mut read_budget)?;
        if expected_source_bytes != approved_source_bytes {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        let stage_owner_bytes = projection_stage_owner_bytes(
            stage_owner,
            page_id,
            source_path,
            &expected_source_bytes,
        )?;
        let orphaned = match (
            approved_orphaned.as_ref(),
            self.capabilities.wenlan.symlink_metadata("orphaned"),
        ) {
            (Some(approved_baseline), Ok(metadata))
                if metadata.is_dir() && !metadata.file_type().is_symlink() =>
            {
                let orphaned = self.capabilities.wenlan.open_dir_nofollow("orphaned")?;
                ensure_orphaned_private(&orphaned)?;
                if orphaned_baseline_nofollow(&orphaned, None, &mut read_budget)?
                    != *approved_baseline
                {
                    return Err(WenlanError::Conflict("repair_target_stale".to_string()));
                }
                orphaned
            }
            (None, Ok(metadata)) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                let orphaned = self.capabilities.wenlan.open_dir_nofollow("orphaned")?;
                ensure_orphaned_private(&orphaned)?;
                if !orphaned_baseline_nofollow(&orphaned, None, &mut read_budget)?.is_empty() {
                    return Err(WenlanError::Conflict("repair_target_stale".to_string()));
                }
                orphaned
            }
            (None, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                #[cfg(unix)]
                {
                    use cap_std::fs::DirBuilderExt as _;
                    let mut builder = cap_std::fs::DirBuilder::new();
                    builder.mode(0o700);
                    self.capabilities
                        .wenlan
                        .create_dir_with("orphaned", &builder)?;
                }
                #[cfg(not(unix))]
                self.capabilities.wenlan.create_dir("orphaned")?;
                let orphaned = self.capabilities.wenlan.open_dir_nofollow("orphaned")?;
                ensure_orphaned_private(&orphaned)?;
                orphaned
            }
            (_, Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(WenlanError::Io(error))
            }
            _ => return Err(WenlanError::Conflict("repair_target_stale".to_string())),
        };
        match orphaned.symlink_metadata(&quarantine_name) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Err(WenlanError::Conflict("repair_target_stale".to_string())),
            Err(error) => return Err(WenlanError::Io(error)),
        }

        let root = self.capabilities.root.try_clone()?;
        let wenlan = self.capabilities.wenlan.try_clone()?;
        let stage = match OwnedProjectionStage::open_existing(
            &wenlan,
            stage_name.clone(),
            stage_owner_bytes.clone(),
        )? {
            Some(stage) => {
                if !matches!(
                    inspect_owned_projection_stage(Some(&stage), &mut read_budget)?,
                    StageContents::Present {
                        source: None,
                        rollback_quarantine: None,
                        state: None,
                    }
                ) {
                    return Err(WenlanError::Conflict("repair_target_stale".to_string()));
                }
                stage
            }
            None => OwnedProjectionStage::create_noclobber(&wenlan, stage_name, stage_owner_bytes)?,
        };
        Ok(PinnedStalePageProjection {
            root,
            wenlan,
            source_parent,
            source_name,
            source_path: source_path.to_string(),
            stage: Some(stage),
            orphaned: Some(orphaned),
            quarantine_name,
            quarantine_path: quarantine_path.to_string(),
            expected_state_bytes,
            next_state_bytes,
            expected_source_bytes,
            expected_source_identity,
            expected_orphaned_baseline: approved_orphaned.unwrap_or_default(),
            state_mode,
            mutation_started: true,
            _lock: std::marker::PhantomData,
        })
    }

    pub(crate) fn recover_stale_page_projection(
        &self,
        rollback: &crate::repair::StoredRollbackArtifact,
        stage_owner: &str,
        restore_post: bool,
    ) -> Result<StalePageProjectionRecoveryState, WenlanError> {
        let (source_path, quarantine_path) = crate::repair::stale_page_projection_paths(rollback)?;
        let state = rollback_row_bytes(rollback, ".wenlan/state.json")?;
        let source = rollback_row_bytes(rollback, &source_path)?;
        let quarantine = rollback
            .rows
            .iter()
            .find(|row| row[0] == quarantine_path)
            .ok_or_else(|| {
                WenlanError::Validation("repair_projection_rollback_invalid".to_string())
            })?;
        if quarantine[1] != "missing" || !quarantine[2].is_empty() {
            return Err(WenlanError::Validation(
                "repair_projection_rollback_invalid".to_string(),
            ));
        }
        let exact_post =
            crate::lint::pages::state::remove_unique_page_member(&state, &rollback.source_id)
                .map_err(|()| {
                    WenlanError::Validation("repair_projection_rollback_invalid".to_string())
                })?;
        let (source_parent, source_name) =
            match open_relative_parent_nofollow(&self.capabilities.root, Path::new(&source_path)) {
                Ok(value) => value,
                Err(_) => return Ok(StalePageProjectionRecoveryState::Unknown),
            };
        let quarantine = Path::new(&quarantine_path);
        if quarantine.parent() != Some(Path::new(".wenlan/orphaned")) {
            return Err(WenlanError::Validation(
                "repair_projection_rollback_invalid".to_string(),
            ));
        }
        let quarantine_name = quarantine
            .file_name()
            .ok_or_else(|| {
                WenlanError::Validation("repair_projection_rollback_invalid".to_string())
            })?
            .to_os_string();
        let orphaned_baseline = crate::repair::stale_page_projection_orphaned_baseline(rollback)?;
        let stage_owner_bytes =
            projection_stage_owner_bytes(stage_owner, &rollback.source_id, &source_path, &source)?;
        let mut recovery = match StalePageProjectionRecovery::open(
            self.capabilities,
            source_parent,
            source_name,
            projection_unlink_stage_name(stage_owner),
            stage_owner_bytes,
            quarantine_name,
            state,
            exact_post,
            source,
            orphaned_baseline,
        ) {
            Ok(recovery) => recovery,
            Err(_) => return Ok(StalePageProjectionRecoveryState::Unknown),
        };
        recovery.normalize_state_stage()?;
        let state = recovery.classify()?;
        if matches!(
            state,
            StalePageProjectionRecoveryState::PreparedStage
                | StalePageProjectionRecoveryState::AfterLink
                | StalePageProjectionRecoveryState::AfterStage
                | StalePageProjectionRecoveryState::RestoredSource
                | StalePageProjectionRecoveryState::AfterUnlink
                | StalePageProjectionRecoveryState::PostStaged
                | StalePageProjectionRecoveryState::QuarantineStaged
                | StalePageProjectionRecoveryState::OriginalCleanup
        ) || (restore_post && matches!(state, StalePageProjectionRecoveryState::Post))
        {
            recovery.restore_original()?;
            return recovery.classify();
        }
        Ok(state)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StalePageProjectionRecoveryState {
    Original,
    PreparedStage,
    AfterLink,
    AfterStage,
    RestoredSource,
    AfterUnlink,
    PostStaged,
    Post,
    QuarantineStaged,
    OriginalCleanup,
    Unknown,
}

struct StalePageProjectionRecovery {
    root: Dir,
    wenlan: Dir,
    source_parent: Dir,
    source_name: OsString,
    stage: Option<OwnedProjectionStage>,
    orphaned: Option<Dir>,
    quarantine_name: OsString,
    original_state: Vec<u8>,
    post_state: Vec<u8>,
    source_bytes: Vec<u8>,
    state_mode: ProjectionStateMode,
    orphaned_baseline: Option<Vec<(String, String)>>,
}

impl StalePageProjectionRecovery {
    #[allow(clippy::too_many_arguments)]
    fn open(
        capabilities: &ProjectionCapabilities,
        source_parent: Dir,
        source_name: OsString,
        stage_name: OsString,
        stage_owner_bytes: Vec<u8>,
        quarantine_name: OsString,
        original_state: Vec<u8>,
        post_state: Vec<u8>,
        source_bytes: Vec<u8>,
        orphaned_baseline: Option<Vec<(String, String)>>,
    ) -> Result<Self, WenlanError> {
        let orphaned = match capabilities.wenlan.symlink_metadata("orphaned") {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                let directory = capabilities.wenlan.open_dir_nofollow("orphaned")?;
                ensure_orphaned_private(&directory)?;
                Some(directory)
            }
            Ok(_) => return Err(WenlanError::Conflict("repair_target_stale".to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(WenlanError::Io(error)),
        };
        let stage = match OwnedProjectionStage::open_existing(
            &capabilities.wenlan,
            stage_name.clone(),
            stage_owner_bytes.clone(),
        )? {
            Some(stage) => stage,
            None => OwnedProjectionStage::create_noclobber(
                &capabilities.wenlan,
                stage_name,
                stage_owner_bytes,
            )?,
        };
        let mut state_budget = RepairReadBudget::new();
        let state_mode = match read_optional_projection_state_identity_nofollow(
            &capabilities.wenlan,
            OsStr::new("state.json"),
            &mut state_budget,
        )? {
            Some((_, mode, _)) => mode,
            None => {
                let (_, mode, _) = read_projection_state_identity_nofollow(
                    &stage.directory,
                    OsStr::new(PROJECTION_STATE_STAGE_FILE),
                    &mut state_budget,
                )?;
                mode
            }
        };
        Ok(Self {
            root: capabilities.root.try_clone()?,
            wenlan: capabilities.wenlan.try_clone()?,
            source_parent,
            source_name,
            stage: Some(stage),
            orphaned,
            quarantine_name,
            original_state,
            post_state,
            source_bytes,
            state_mode,
            orphaned_baseline,
        })
    }

    fn normalize_state_stage(&mut self) -> Result<(), WenlanError> {
        let stage = self
            .stage
            .as_ref()
            .ok_or_else(|| WenlanError::Conflict("repair_apply_recovery_required".to_string()))?;
        let mut staged_budget = RepairReadBudget::new();
        let Some((staged, staged_mode, staged_identity)) =
            read_optional_projection_state_identity_nofollow(
                &stage.directory,
                OsStr::new(PROJECTION_STATE_STAGE_FILE),
                &mut staged_budget,
            )?
        else {
            return Ok(());
        };
        if staged_mode != self.state_mode
            || (staged != self.original_state && staged != self.post_state)
        {
            return Ok(());
        }

        let mut public_budget = RepairReadBudget::new();
        let public = read_optional_projection_state_identity_nofollow(
            &self.wenlan,
            OsStr::new("state.json"),
            &mut public_budget,
        )?;
        match public {
            None => {
                match stage.directory.hard_link(
                    PROJECTION_STATE_STAGE_FILE,
                    &self.wenlan,
                    "state.json",
                ) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        return Ok(())
                    }
                    Err(error) => return Err(WenlanError::Io(error)),
                }
                let mut restored_budget = RepairReadBudget::new();
                let (restored, restored_mode, restored_identity) =
                    read_projection_state_identity_nofollow(
                        &self.wenlan,
                        OsStr::new("state.json"),
                        &mut restored_budget,
                    )?;
                if restored != staged
                    || restored_mode != staged_mode
                    || restored_identity != staged_identity
                {
                    return Err(WenlanError::Conflict(
                        "repair_apply_recovery_required".to_string(),
                    ));
                }
            }
            Some((public, public_mode, _))
                if public_mode == self.state_mode
                    && ((staged == self.original_state && public == self.post_state)
                        || (staged == self.post_state && public == self.original_state)) => {}
            Some(_) => return Ok(()),
        }

        let mut final_budget = RepairReadBudget::new();
        let (final_staged, final_mode, final_identity) = read_projection_state_identity_nofollow(
            &stage.directory,
            OsStr::new(PROJECTION_STATE_STAGE_FILE),
            &mut final_budget,
        )?;
        if final_staged != staged || final_mode != staged_mode || final_identity != staged_identity
        {
            return Err(WenlanError::Conflict(
                "repair_apply_recovery_required".to_string(),
            ));
        }
        stage.directory.remove_file(PROJECTION_STATE_STAGE_FILE)?;
        #[cfg(unix)]
        {
            sync_dir_capability(&stage.directory)?;
            sync_dir_capability(&self.wenlan)?;
            sync_dir_capability(&self.root)?;
        }
        Ok(())
    }

    fn classify(&self) -> Result<StalePageProjectionRecoveryState, WenlanError> {
        let mut read_budget = RepairReadBudget::new();
        let (state, current_mode) = match read_state_nofollow(&self.wenlan, &mut read_budget) {
            Ok(value) => value,
            Err(_) => return Ok(StalePageProjectionRecoveryState::Unknown),
        };
        if current_mode != self.state_mode {
            return Ok(StalePageProjectionRecoveryState::Unknown);
        }
        let source = match read_optional_regular_nofollow(
            &self.source_parent,
            &self.source_name,
            &mut read_budget,
        ) {
            Ok(value) => value,
            Err(_) => return Ok(StalePageProjectionRecoveryState::Unknown),
        };
        let (stage, staged_quarantine, stage_prepared) =
            match inspect_owned_projection_stage(self.stage.as_ref(), &mut read_budget) {
                Ok(StageContents::Absent) => (None, None, false),
                Ok(StageContents::Present {
                    source,
                    rollback_quarantine,
                    state: None,
                }) => (source, rollback_quarantine, true),
                Ok(StageContents::Present { state: Some(_), .. }) => {
                    return Ok(StalePageProjectionRecoveryState::Unknown)
                }
                Err(_) => return Ok(StalePageProjectionRecoveryState::Unknown),
            };
        let quarantine = match &self.orphaned {
            Some(orphaned) => match read_optional_regular_nofollow(
                orphaned,
                &self.quarantine_name,
                &mut read_budget,
            ) {
                Ok(value) => value,
                Err(_) => return Ok(StalePageProjectionRecoveryState::Unknown),
            },
            None => None,
        };
        let orphaned_shape_matches = match (&self.orphaned_baseline, &self.orphaned) {
            (None, None) => true,
            (None, Some(orphaned)) => {
                orphaned_baseline_nofollow(orphaned, Some(&self.quarantine_name), &mut read_budget)?
                    .is_empty()
            }
            (Some(expected), Some(orphaned)) => {
                orphaned_baseline_nofollow(orphaned, Some(&self.quarantine_name), &mut read_budget)?
                    == *expected
            }
            (Some(_), None) => false,
        };
        if !orphaned_shape_matches {
            return Ok(StalePageProjectionRecoveryState::Unknown);
        }
        let source_original = source.as_deref() == Some(self.source_bytes.as_slice());
        let source_absent = source.is_none();
        let stage_original = stage.as_deref() == Some(self.source_bytes.as_slice());
        let stage_absent = stage.is_none() && !stage_prepared;
        let stage_empty = stage.is_none() && stage_prepared;
        let quarantine_original = quarantine.as_deref() == Some(self.source_bytes.as_slice());
        let quarantine_absent = quarantine.is_none();
        let staged_quarantine_original =
            staged_quarantine.as_deref() == Some(self.source_bytes.as_slice());
        let staged_quarantine_absent = staged_quarantine.is_none();
        let restored_links_match = if source_original && stage_original && quarantine_original {
            let identities = (|| {
                let source = projection_file_identity(
                    &self.source_parent.symlink_metadata(&self.source_name)?,
                )?;
                let stage = projection_file_identity(
                    &self
                        .stage
                        .as_ref()
                        .ok_or_else(|| {
                            WenlanError::Conflict("repair_apply_recovery_required".to_string())
                        })?
                        .directory
                        .symlink_metadata(PROJECTION_UNLINK_STAGE_FILE)?,
                )?;
                let quarantine = projection_file_identity(
                    &self
                        .orphaned
                        .as_ref()
                        .ok_or_else(|| {
                            WenlanError::Conflict("repair_apply_recovery_required".to_string())
                        })?
                        .symlink_metadata(&self.quarantine_name)?,
                )?;
                Ok::<_, WenlanError>(source == stage && stage == quarantine)
            })();
            identities.unwrap_or(false)
        } else {
            false
        };
        Ok(
            if state == self.original_state
                && source_original
                && (stage_absent || stage_empty)
                && quarantine_absent
                && staged_quarantine_absent
            {
                StalePageProjectionRecoveryState::Original
            } else if state == self.original_state
                && source_original
                && stage_empty
                && quarantine_absent
                && staged_quarantine_absent
            {
                StalePageProjectionRecoveryState::PreparedStage
            } else if state == self.original_state
                && source_original
                && (stage_absent || stage_empty)
                && quarantine_absent
                && staged_quarantine_absent
                && self.orphaned_baseline.is_none()
                && self.orphaned.is_some()
            {
                StalePageProjectionRecoveryState::OriginalCleanup
            } else if state == self.original_state
                && source_original
                && (stage_absent || stage_empty)
                && quarantine_original
                && staged_quarantine_absent
            {
                StalePageProjectionRecoveryState::AfterLink
            } else if state == self.original_state
                && source_absent
                && stage_original
                && quarantine_original
                && staged_quarantine_absent
            {
                StalePageProjectionRecoveryState::AfterStage
            } else if state == self.original_state
                && source_original
                && stage_original
                && quarantine_original
                && staged_quarantine_absent
                && restored_links_match
            {
                StalePageProjectionRecoveryState::RestoredSource
            } else if state == self.original_state
                && source_absent
                && (stage_absent || stage_empty)
                && quarantine_original
                && staged_quarantine_absent
            {
                StalePageProjectionRecoveryState::AfterUnlink
            } else if state == self.post_state
                && source_absent
                && stage_original
                && quarantine_original
                && staged_quarantine_absent
            {
                StalePageProjectionRecoveryState::PostStaged
            } else if state == self.post_state
                && source_absent
                && (stage_absent || stage_empty)
                && quarantine_original
                && staged_quarantine_absent
            {
                StalePageProjectionRecoveryState::Post
            } else if state == self.original_state
                && source_original
                && stage_empty
                && quarantine_absent
                && staged_quarantine_original
            {
                StalePageProjectionRecoveryState::QuarantineStaged
            } else {
                StalePageProjectionRecoveryState::Unknown
            },
        )
    }

    fn restore_original(&mut self) -> Result<(), WenlanError> {
        let state = self.classify()?;
        let mut read_budget = RepairReadBudget::new();
        if !matches!(
            state,
            StalePageProjectionRecoveryState::PreparedStage
                | StalePageProjectionRecoveryState::AfterLink
                | StalePageProjectionRecoveryState::AfterStage
                | StalePageProjectionRecoveryState::RestoredSource
                | StalePageProjectionRecoveryState::AfterUnlink
                | StalePageProjectionRecoveryState::PostStaged
                | StalePageProjectionRecoveryState::Post
                | StalePageProjectionRecoveryState::QuarantineStaged
                | StalePageProjectionRecoveryState::OriginalCleanup
        ) {
            return Err(WenlanError::Conflict(
                "repair_apply_recovery_required".to_string(),
            ));
        }
        if matches!(
            state,
            StalePageProjectionRecoveryState::Post | StalePageProjectionRecoveryState::PostStaged
        ) {
            write_state_atomically_if_matches(
                &self.wenlan,
                self.stage.as_ref().ok_or_else(|| {
                    WenlanError::Conflict("repair_apply_recovery_required".to_string())
                })?,
                &self.post_state,
                &self.original_state,
                self.state_mode,
                &mut read_budget,
            )?;
            #[cfg(unix)]
            {
                sync_dir_capability(&self.wenlan)?;
                sync_dir_capability(&self.root)?;
            }
            let expected = if state == StalePageProjectionRecoveryState::PostStaged {
                StalePageProjectionRecoveryState::AfterStage
            } else {
                StalePageProjectionRecoveryState::AfterUnlink
            };
            if self.classify()? != expected {
                return Err(WenlanError::Conflict(
                    "repair_apply_recovery_required".to_string(),
                ));
            }
        }
        if matches!(
            state,
            StalePageProjectionRecoveryState::AfterStage
                | StalePageProjectionRecoveryState::PostStaged
        ) {
            let stage = self.stage.as_ref().ok_or_else(|| {
                WenlanError::Conflict("repair_apply_recovery_required".to_string())
            })?;
            stage
                .directory
                .hard_link(
                    PROJECTION_UNLINK_STAGE_FILE,
                    &self.source_parent,
                    &self.source_name,
                )
                .map_err(|error| {
                    WenlanError::Io(std::io::Error::new(
                        error.kind(),
                        format!("repair source recovery no-clobber hard link failed: {error}"),
                    ))
                })?;
            let mut stage_budget = RepairReadBudget::new();
            let (source, source_identity) = read_regular_identity_nofollow_bounded(
                &self.source_parent,
                &self.source_name,
                &mut stage_budget,
            )?;
            let (stage_bytes, stage_identity) = read_regular_identity_nofollow_bounded(
                &stage.directory,
                OsStr::new(PROJECTION_UNLINK_STAGE_FILE),
                &mut stage_budget,
            )?;
            if source != self.source_bytes
                || stage_bytes != self.source_bytes
                || source_identity != stage_identity
            {
                return Err(WenlanError::Conflict(
                    "repair_apply_recovery_required".to_string(),
                ));
            }
            stage.directory.remove_file(PROJECTION_UNLINK_STAGE_FILE)?;
            #[cfg(unix)]
            {
                sync_dir_capability(&self.source_parent)?;
                sync_dir_capability(&self.wenlan)?;
                sync_dir_capability(&self.root)?;
            }
            if self.classify()? != StalePageProjectionRecoveryState::AfterLink {
                return Err(WenlanError::Conflict(
                    "repair_apply_recovery_required".to_string(),
                ));
            }
        }
        if state == StalePageProjectionRecoveryState::RestoredSource {
            let stage = self.stage.as_ref().ok_or_else(|| {
                WenlanError::Conflict("repair_apply_recovery_required".to_string())
            })?;
            let source_identity =
                projection_file_identity(&self.source_parent.symlink_metadata(&self.source_name)?)?;
            let stage_identity = projection_file_identity(
                &stage
                    .directory
                    .symlink_metadata(PROJECTION_UNLINK_STAGE_FILE)?,
            )?;
            let orphaned = self.orphaned.as_ref().ok_or_else(|| {
                WenlanError::Conflict("repair_apply_recovery_required".to_string())
            })?;
            let quarantine_identity =
                projection_file_identity(&orphaned.symlink_metadata(&self.quarantine_name)?)?;
            if source_identity != stage_identity || stage_identity != quarantine_identity {
                return Err(WenlanError::Conflict(
                    "repair_apply_recovery_required".to_string(),
                ));
            }
            stage.directory.remove_file(PROJECTION_UNLINK_STAGE_FILE)?;
            #[cfg(unix)]
            sync_dir_capability(&stage.directory)?;
            if self.classify()? != StalePageProjectionRecoveryState::AfterLink {
                return Err(WenlanError::Conflict(
                    "repair_apply_recovery_required".to_string(),
                ));
            }
        }
        if matches!(
            state,
            StalePageProjectionRecoveryState::AfterUnlink | StalePageProjectionRecoveryState::Post
        ) {
            write_new_file_nofollow(&self.source_parent, &self.source_name, &self.source_bytes)?;
            #[cfg(unix)]
            {
                sync_dir_capability(&self.source_parent)?;
                sync_dir_capability(&self.root)?;
            }
            if self.classify()? != StalePageProjectionRecoveryState::AfterLink {
                return Err(WenlanError::Conflict(
                    "repair_apply_recovery_required".to_string(),
                ));
            }
        }
        if state == StalePageProjectionRecoveryState::QuarantineStaged {
            self.stage
                .as_ref()
                .ok_or_else(|| WenlanError::Conflict("repair_apply_recovery_required".to_string()))?
                .delete_staged_quarantine_if_exact(&self.source_bytes, None)?;
        } else if !matches!(
            state,
            StalePageProjectionRecoveryState::PreparedStage
                | StalePageProjectionRecoveryState::OriginalCleanup
        ) {
            let orphaned = self.orphaned.as_ref().ok_or_else(|| {
                WenlanError::Conflict("repair_apply_recovery_required".to_string())
            })?;
            self.stage
                .as_ref()
                .ok_or_else(|| WenlanError::Conflict("repair_apply_recovery_required".to_string()))?
                .move_quarantine_for_exact_delete(
                    orphaned,
                    &self.quarantine_name,
                    &self.source_bytes,
                    None,
                )?;
            sync_projection_dirs(&self.root, &self.wenlan, &self.source_parent, orphaned)?;
        }
        if let Some(stage) = self.stage.as_ref() {
            stage.remove_if_empty(&self.wenlan)?;
        }
        Ok(())
    }
}

fn orphaned_baseline_nofollow(
    orphaned: &Dir,
    excluded: Option<&OsStr>,
    budget: &mut RepairReadBudget,
) -> Result<Vec<(String, String)>, WenlanError> {
    let mut baseline = Vec::new();
    for entry in orphaned.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        charge_repair_read_budget(
            budget,
            ORPHAN_BASELINE_ENTRY_BUDGET_BYTES
                .checked_add(os_str_storage_len(&name)?)
                .ok_or_else(repair_projection_rollback_too_large)?,
        )?;
        if excluded.is_some_and(|excluded| excluded == name) {
            continue;
        }
        let name_string = name
            .to_str()
            .ok_or_else(|| WenlanError::Conflict("repair_target_stale".to_string()))?
            .to_string();
        baseline.push((
            name_string,
            hash_regular_nofollow_bounded(orphaned, &name, budget)?,
        ));
    }
    baseline.sort();
    Ok(baseline)
}

fn rollback_row_bytes(
    rollback: &crate::repair::StoredRollbackArtifact,
    path: &str,
) -> Result<Vec<u8>, WenlanError> {
    let row = rollback
        .rows
        .iter()
        .find(|row| row[0] == path && row[1] == "file_hex")
        .ok_or_else(|| WenlanError::Validation("repair_projection_rollback_invalid".to_string()))?;
    hex::decode(&row[2])
        .map_err(|_| WenlanError::Validation("repair_projection_rollback_invalid".to_string()))
}

pub(crate) fn projection_unlink_stage_name(owner: &str) -> OsString {
    format!(
        ".projection-unlink-{}",
        hex::encode(sha2::Sha256::digest(owner.as_bytes()))
    )
    .into()
}

#[derive(Serialize)]
struct ProjectionStageOwner<'a> {
    format_version: u32,
    manifest_id: &'a str,
    page_id: &'a str,
    source_path: &'a str,
    source_digest: String,
}

pub(crate) fn projection_stage_owner_bytes(
    manifest_id: &str,
    page_id: &str,
    source_path: &str,
    source_bytes: &[u8],
) -> Result<Vec<u8>, WenlanError> {
    serde_json::to_vec(&ProjectionStageOwner {
        format_version: 1,
        manifest_id,
        page_id,
        source_path,
        source_digest: hex::encode(sha2::Sha256::digest(source_bytes)),
    })
    .map_err(WenlanError::from)
}

struct OwnedProjectionStage {
    directory: Dir,
    owner_bytes: Vec<u8>,
}

enum StageContents {
    Absent,
    Present {
        source: Option<Vec<u8>>,
        rollback_quarantine: Option<Vec<u8>>,
        state: Option<Vec<u8>>,
    },
}

fn inspect_owned_projection_stage(
    stage: Option<&OwnedProjectionStage>,
    budget: &mut RepairReadBudget,
) -> Result<StageContents, WenlanError> {
    let Some(stage) = stage else {
        return Ok(StageContents::Absent);
    };
    let mut source = None;
    let mut rollback_quarantine = None;
    let mut state = None;
    let mut owner_seen = false;
    for entry in stage.directory.entries()? {
        let entry = entry?;
        match entry.file_name().to_str() {
            Some(PROJECTION_STAGE_OWNER_FILE) if !owner_seen => {
                if read_regular_nofollow(
                    &stage.directory,
                    OsStr::new(PROJECTION_STAGE_OWNER_FILE),
                    budget,
                )? != stage.owner_bytes
                {
                    return Err(WenlanError::Conflict("repair_target_stale".to_string()));
                }
                owner_seen = true;
            }
            Some(PROJECTION_UNLINK_STAGE_FILE) if source.is_none() => {
                source = Some(read_regular_nofollow(
                    &stage.directory,
                    OsStr::new(PROJECTION_UNLINK_STAGE_FILE),
                    budget,
                )?);
            }
            Some(PROJECTION_ROLLBACK_QUARANTINE_STAGE_FILE) if rollback_quarantine.is_none() => {
                rollback_quarantine = Some(read_regular_nofollow(
                    &stage.directory,
                    OsStr::new(PROJECTION_ROLLBACK_QUARANTINE_STAGE_FILE),
                    budget,
                )?);
            }
            Some(PROJECTION_STATE_STAGE_FILE) if state.is_none() => {
                state = Some(read_regular_nofollow(
                    &stage.directory,
                    OsStr::new(PROJECTION_STATE_STAGE_FILE),
                    budget,
                )?);
            }
            _ => return Err(WenlanError::Conflict("repair_target_stale".to_string())),
        }
    }
    if !owner_seen {
        return Err(WenlanError::Conflict("repair_target_stale".to_string()));
    }
    Ok(StageContents::Present {
        source,
        rollback_quarantine,
        state,
    })
}

impl OwnedProjectionStage {
    fn create_noclobber(
        wenlan: &Dir,
        name: OsString,
        owner_bytes: Vec<u8>,
    ) -> Result<Self, WenlanError> {
        let create_result = {
            #[cfg(unix)]
            {
                use cap_std::fs::DirBuilderExt as _;
                let mut builder = cap_std::fs::DirBuilder::new();
                builder.mode(0o700);
                wenlan.create_dir_with(Path::new(&name), &builder)
            }
            #[cfg(not(unix))]
            {
                wenlan.create_dir(Path::new(&name))
            }
        };
        match create_result {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(WenlanError::Conflict("repair_target_stale".to_string()))
            }
            Err(error) => return Err(WenlanError::Io(error)),
        }

        let directory = wenlan.open_dir_nofollow(Path::new(&name))?;
        ensure_orphaned_private(&directory)?;
        write_new_file_nofollow(
            &directory,
            OsStr::new(PROJECTION_STAGE_OWNER_FILE),
            &owner_bytes,
        )?;
        #[cfg(unix)]
        sync_dir_capability(&directory)?;
        Ok(Self {
            directory,
            owner_bytes,
        })
    }

    fn open_existing(
        wenlan: &Dir,
        name: OsString,
        owner_bytes: Vec<u8>,
    ) -> Result<Option<Self>, WenlanError> {
        match wenlan.symlink_metadata(Path::new(&name)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(WenlanError::Io(error)),
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                let directory = wenlan.open_dir_nofollow(Path::new(&name))?;
                ensure_orphaned_private(&directory)?;
                let mut budget = RepairReadBudget::new();
                if read_regular_nofollow(
                    &directory,
                    OsStr::new(PROJECTION_STAGE_OWNER_FILE),
                    &mut budget,
                )? != owner_bytes
                {
                    return Err(WenlanError::Conflict("repair_target_stale".to_string()));
                }
                Ok(Some(Self {
                    directory,
                    owner_bytes,
                }))
            }
            Ok(_) => Err(WenlanError::Conflict("repair_target_stale".to_string())),
        }
    }

    fn remove_if_empty(&self, _wenlan: &Dir) -> Result<(), WenlanError> {
        let mut budget = RepairReadBudget::new();
        if !matches!(
            inspect_owned_projection_stage(Some(self), &mut budget)?,
            StageContents::Present {
                source: None,
                rollback_quarantine: None,
                state: None,
            }
        ) {
            return Err(WenlanError::Conflict(
                "repair_apply_recovery_required".to_string(),
            ));
        }
        #[cfg(unix)]
        sync_dir_capability(&self.directory)?;
        Ok(())
    }

    fn delete_staged_quarantine_if_exact(
        &self,
        expected_bytes: &[u8],
        expected_identity: Option<ProjectionFileIdentity>,
    ) -> Result<(), WenlanError> {
        let mut budget = RepairReadBudget::new();
        let (bytes, identity) = read_regular_identity_nofollow_bounded(
            &self.directory,
            OsStr::new(PROJECTION_ROLLBACK_QUARANTINE_STAGE_FILE),
            &mut budget,
        )?;
        if bytes != expected_bytes || expected_identity.is_some_and(|expected| expected != identity)
        {
            return Err(WenlanError::Conflict(
                "repair_apply_recovery_required".to_string(),
            ));
        }
        self.directory
            .remove_file(PROJECTION_ROLLBACK_QUARANTINE_STAGE_FILE)?;
        #[cfg(unix)]
        sync_dir_capability(&self.directory)?;
        Ok(())
    }

    fn move_quarantine_for_exact_delete(
        &self,
        orphaned: &Dir,
        quarantine_name: &OsStr,
        expected_bytes: &[u8],
        expected_identity: Option<ProjectionFileIdentity>,
    ) -> Result<(), WenlanError> {
        match self
            .directory
            .symlink_metadata(PROJECTION_ROLLBACK_QUARANTINE_STAGE_FILE)
        {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(WenlanError::Conflict(
                    "repair_apply_recovery_required".to_string(),
                ))
            }
            Err(error) => return Err(WenlanError::Io(error)),
        }
        orphaned.rename(
            Path::new(quarantine_name),
            &self.directory,
            Path::new(PROJECTION_ROLLBACK_QUARANTINE_STAGE_FILE),
        )?;
        #[cfg(unix)]
        {
            sync_dir_capability(orphaned)?;
            sync_dir_capability(&self.directory)?;
        }

        let mut budget = RepairReadBudget::new();
        let moved = read_regular_identity_nofollow_bounded(
            &self.directory,
            OsStr::new(PROJECTION_ROLLBACK_QUARANTINE_STAGE_FILE),
            &mut budget,
        );
        let exact = matches!(
            &moved,
            Ok((bytes, identity))
                if bytes == expected_bytes
                    && expected_identity.is_none_or(|expected| expected == *identity)
        );
        if !exact {
            if let Ok((_, moved_identity)) = moved {
                match self.directory.hard_link(
                    PROJECTION_ROLLBACK_QUARANTINE_STAGE_FILE,
                    orphaned,
                    quarantine_name,
                ) {
                    Ok(()) => {
                        let restored =
                            projection_file_identity(&orphaned.symlink_metadata(quarantine_name)?)?;
                        if restored == moved_identity {
                            self.directory
                                .remove_file(PROJECTION_ROLLBACK_QUARANTINE_STAGE_FILE)?;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(WenlanError::Io(error)),
                }
            }
            return Err(WenlanError::Conflict(
                "repair_apply_recovery_required".to_string(),
            ));
        }
        self.delete_staged_quarantine_if_exact(expected_bytes, expected_identity)
    }
}

pub(crate) struct PinnedStalePageProjection<'lock> {
    root: Dir,
    wenlan: Dir,
    source_parent: Dir,
    source_name: OsString,
    source_path: String,
    stage: Option<OwnedProjectionStage>,
    orphaned: Option<Dir>,
    quarantine_name: OsString,
    quarantine_path: String,
    expected_state_bytes: Vec<u8>,
    next_state_bytes: Vec<u8>,
    expected_source_bytes: Vec<u8>,
    expected_source_identity: ProjectionFileIdentity,
    expected_orphaned_baseline: Vec<(String, String)>,
    state_mode: ProjectionStateMode,
    mutation_started: bool,
    _lock: std::marker::PhantomData<&'lock ProjectionCapabilities>,
}

impl PinnedStalePageProjection<'_> {
    fn orphaned(&self) -> &Dir {
        self.orphaned
            .as_ref()
            .expect("pinned orphaned directory retained until rollback")
    }

    fn stage(&self) -> Result<&OwnedProjectionStage, WenlanError> {
        self.stage
            .as_ref()
            .ok_or_else(|| WenlanError::Conflict("repair_apply_recovery_required".to_string()))
    }

    pub(crate) fn mutation_started(&self) -> bool {
        self.mutation_started
    }

    pub(crate) fn quarantine(&mut self) -> Result<(), WenlanError> {
        self.quarantine_inner(|| Ok(()), || Ok(()), || Ok(()), || Ok(()), || Ok(()))
    }

    #[cfg(test)]
    pub(crate) fn quarantine_with_hooks<B, S>(
        &mut self,
        before_link: B,
        before_source_stage: S,
    ) -> Result<(), WenlanError>
    where
        B: FnOnce() -> Result<(), WenlanError>,
        S: FnOnce() -> Result<(), WenlanError>,
    {
        self.quarantine_inner(
            before_link,
            || Ok(()),
            before_source_stage,
            || Ok(()),
            || Ok(()),
        )
    }

    #[cfg(test)]
    pub(crate) fn quarantine_with_after_link<A>(&mut self, after_link: A) -> Result<(), WenlanError>
    where
        A: FnOnce() -> Result<(), WenlanError>,
    {
        self.quarantine_inner(|| Ok(()), after_link, || Ok(()), || Ok(()), || Ok(()))
    }

    #[cfg(test)]
    pub(crate) fn quarantine_with_after_source_stage<A>(
        &mut self,
        after_source_stage: A,
    ) -> Result<(), WenlanError>
    where
        A: FnOnce() -> Result<(), WenlanError>,
    {
        self.quarantine_inner(
            || Ok(()),
            || Ok(()),
            || Ok(()),
            after_source_stage,
            || Ok(()),
        )
    }

    #[cfg(test)]
    pub(crate) fn quarantine_with_before_state_swap<B>(
        &mut self,
        before_state_swap: B,
    ) -> Result<(), WenlanError>
    where
        B: FnOnce() -> Result<(), WenlanError>,
    {
        self.quarantine_inner(
            || Ok(()),
            || Ok(()),
            || Ok(()),
            || Ok(()),
            before_state_swap,
        )
    }

    fn quarantine_inner<B, L, S, A, T>(
        &mut self,
        before_link: B,
        after_link: L,
        before_source_stage: S,
        after_source_stage: A,
        before_state_swap: T,
    ) -> Result<(), WenlanError>
    where
        B: FnOnce() -> Result<(), WenlanError>,
        L: FnOnce() -> Result<(), WenlanError>,
        S: FnOnce() -> Result<(), WenlanError>,
        A: FnOnce() -> Result<(), WenlanError>,
        T: FnOnce() -> Result<(), WenlanError>,
    {
        let mut initial_budget = RepairReadBudget::new();
        let (state_bytes, current_mode) = read_state_nofollow(&self.wenlan, &mut initial_budget)?;
        if state_bytes != self.expected_state_bytes || current_mode != self.state_mode {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        let (source_bytes, source_identity) = read_regular_identity_nofollow_bounded(
            &self.source_parent,
            &self.source_name,
            &mut initial_budget,
        )?;
        if source_bytes != self.expected_source_bytes
            || source_identity != self.expected_source_identity
        {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        before_link()?;
        let mut mutation_budget = RepairReadBudget::new();
        if orphaned_baseline_nofollow(self.orphaned(), None, &mut mutation_budget)?
            != self.expected_orphaned_baseline
        {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        let (source_bytes, source_identity) = read_regular_identity_nofollow_bounded(
            &self.source_parent,
            &self.source_name,
            &mut mutation_budget,
        )?;
        if source_bytes != self.expected_source_bytes
            || source_identity != self.expected_source_identity
        {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        match self.orphaned().symlink_metadata(&self.quarantine_name) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Err(WenlanError::Conflict("repair_target_stale".to_string())),
            Err(error) => return Err(WenlanError::Io(error)),
        }
        self.source_parent
            .hard_link(&self.source_name, self.orphaned(), &self.quarantine_name)
            .map_err(|error| {
                WenlanError::Io(std::io::Error::new(
                    error.kind(),
                    format!("repair quarantine no-clobber hard link failed: {error}"),
                ))
            })?;
        self.mutation_started = true;
        after_link()?;
        let mut linked_budget = RepairReadBudget::new();
        let linked = read_regular_identity_nofollow_bounded(
            self.orphaned(),
            &self.quarantine_name,
            &mut linked_budget,
        );
        let source_after_link = self
            .source_parent
            .symlink_metadata(&self.source_name)
            .and_then(|metadata| {
                projection_file_identity(&metadata)
                    .map_err(|error| std::io::Error::other(error.to_string()))
            });
        let link_is_exact = matches!(
            (&linked, &source_after_link),
            (Ok((linked_bytes, linked_identity)), Ok(source_identity))
                if linked_bytes == &self.expected_source_bytes
                    && *linked_identity == self.expected_source_identity
                    && *source_identity == self.expected_source_identity
        );
        if !link_is_exact {
            if let Ok((linked_bytes, linked_identity)) = &linked {
                if linked_bytes == &self.expected_source_bytes
                    && *linked_identity == self.expected_source_identity
                {
                    self.stage()?.move_quarantine_for_exact_delete(
                        self.orphaned(),
                        &self.quarantine_name,
                        &self.expected_source_bytes,
                        Some(self.expected_source_identity),
                    )?;
                    self.mutation_started = false;
                    return Err(WenlanError::Conflict("repair_target_stale".to_string()));
                }
            }
            return Err(WenlanError::Conflict(
                "repair_apply_recovery_required".to_string(),
            ));
        }
        let mut final_source_budget = RepairReadBudget::new();
        let (source_bytes, source_identity) = read_regular_identity_nofollow_bounded(
            &self.source_parent,
            &self.source_name,
            &mut final_source_budget,
        )?;
        if source_bytes != self.expected_source_bytes
            || source_identity != self.expected_source_identity
        {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        before_source_stage()?;
        if !matches!(
            inspect_owned_projection_stage(self.stage.as_ref(), &mut final_source_budget)?,
            StageContents::Present {
                source: None,
                rollback_quarantine: None,
                state: None,
            }
        ) {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        self.source_parent.rename(
            Path::new(&self.source_name),
            &self.stage()?.directory,
            Path::new(PROJECTION_UNLINK_STAGE_FILE),
        )?;
        #[cfg(unix)]
        {
            sync_dir_capability(&self.source_parent)?;
            sync_dir_capability(&self.stage()?.directory)?;
            sync_dir_capability(&self.root)?;
        }
        after_source_stage()?;
        let mut staged_budget = RepairReadBudget::new();
        let staged = read_regular_identity_nofollow_bounded(
            &self.stage()?.directory,
            OsStr::new(PROJECTION_UNLINK_STAGE_FILE),
            &mut staged_budget,
        );
        let source_after_stage = read_optional_regular_nofollow(
            &self.source_parent,
            &self.source_name,
            &mut staged_budget,
        );
        let stage_is_exact = matches!(
            (&staged, &source_after_stage),
            (Ok((staged_bytes, staged_identity)), Ok(None))
                if staged_bytes == &self.expected_source_bytes
                    && *staged_identity == self.expected_source_identity
        );
        if !stage_is_exact {
            self.restore_failed_source_stage(staged, source_after_stage)?;
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        let mut state_budget = RepairReadBudget::new();
        write_state_atomically_if_matches_with_hook(
            &self.wenlan,
            self.stage()?,
            &self.expected_state_bytes,
            &self.next_state_bytes,
            self.state_mode,
            &mut state_budget,
            before_state_swap,
        )?;
        let staged_metadata = self
            .stage()?
            .directory
            .symlink_metadata(PROJECTION_UNLINK_STAGE_FILE)?;
        if projection_file_identity(&staged_metadata)? != self.expected_source_identity {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
        self.stage()?
            .directory
            .remove_file(PROJECTION_UNLINK_STAGE_FILE)?;
        self.stage()?.remove_if_empty(&self.wenlan)?;
        sync_projection_dirs(
            &self.root,
            &self.wenlan,
            &self.source_parent,
            self.orphaned(),
        )?;
        Ok(())
    }

    fn restore_failed_source_stage(
        &mut self,
        staged: Result<(Vec<u8>, ProjectionFileIdentity), WenlanError>,
        source_after_stage: Result<Option<Vec<u8>>, WenlanError>,
    ) -> Result<(), WenlanError> {
        let (staged_bytes, staged_identity) = staged?;
        let source_after_stage = source_after_stage?;
        match source_after_stage {
            None => {
                self.stage()?
                    .directory
                    .hard_link(
                        PROJECTION_UNLINK_STAGE_FILE,
                        &self.source_parent,
                        &self.source_name,
                    )
                    .map_err(|error| {
                        WenlanError::Io(std::io::Error::new(
                            error.kind(),
                            format!("repair source restore no-clobber hard link failed: {error}"),
                        ))
                    })?;
                let mut restored_budget = RepairReadBudget::new();
                let (restored_bytes, restored_identity) = read_regular_identity_nofollow_bounded(
                    &self.source_parent,
                    &self.source_name,
                    &mut restored_budget,
                )?;
                if restored_bytes != staged_bytes || restored_identity != staged_identity {
                    return Err(WenlanError::Conflict(
                        "repair_apply_recovery_required".to_string(),
                    ));
                }
            }
            Some(_) => {
                return Err(WenlanError::Conflict(
                    "repair_apply_recovery_required".to_string(),
                ))
            }
        }
        let stage_metadata = self
            .stage()?
            .directory
            .symlink_metadata(PROJECTION_UNLINK_STAGE_FILE)?;
        if projection_file_identity(&stage_metadata)? != staged_identity {
            return Err(WenlanError::Conflict(
                "repair_apply_recovery_required".to_string(),
            ));
        }
        self.stage()?
            .directory
            .remove_file(PROJECTION_UNLINK_STAGE_FILE)?;

        self.stage()?.move_quarantine_for_exact_delete(
            self.orphaned(),
            &self.quarantine_name,
            &self.expected_source_bytes,
            Some(self.expected_source_identity),
        )?;
        self.stage()?.remove_if_empty(&self.wenlan)?;
        sync_projection_dirs(
            &self.root,
            &self.wenlan,
            &self.source_parent,
            self.orphaned(),
        )?;
        self.mutation_started = false;
        Ok(())
    }

    pub(crate) fn restore_snapshot(
        &mut self,
        rollback: &crate::repair::StoredRollbackArtifact,
    ) -> Result<(), WenlanError> {
        let (source_path, quarantine_path) = crate::repair::stale_page_projection_paths(rollback)?;
        if source_path != self.source_path || quarantine_path != self.quarantine_path {
            return Err(WenlanError::Validation(
                "repair_projection_rollback_invalid".to_string(),
            ));
        }
        let state = rollback
            .rows
            .iter()
            .find(|row| row[0] == ".wenlan/state.json" && row[1] == "file_hex")
            .ok_or_else(|| {
                WenlanError::Validation("repair_projection_rollback_invalid".to_string())
            })?;
        let source = rollback
            .rows
            .iter()
            .find(|row| row[0] == source_path && row[1] == "file_hex")
            .ok_or_else(|| {
                WenlanError::Validation("repair_projection_rollback_invalid".to_string())
            })?;
        let state_bytes = hex::decode(&state[2]).map_err(|_| {
            WenlanError::Validation("repair_projection_rollback_invalid".to_string())
        })?;
        let source_bytes = hex::decode(&source[2]).map_err(|_| {
            WenlanError::Validation("repair_projection_rollback_invalid".to_string())
        })?;

        let mut read_budget = RepairReadBudget::new();
        let (current_state, current_mode) = read_state_nofollow(&self.wenlan, &mut read_budget)?;
        if current_mode != self.state_mode
            || (current_state != state_bytes && current_state != self.next_state_bytes)
        {
            return Err(WenlanError::Conflict(
                "repair_projection_rollback_state_collision".to_string(),
            ));
        }
        let mut source_budget = RepairReadBudget::new();
        let source_exists = match read_optional_regular_nofollow(
            &self.source_parent,
            &self.source_name,
            &mut source_budget,
        ) {
            Ok(Some(bytes)) if bytes == source_bytes => true,
            Ok(None) => false,
            _ => {
                return Err(WenlanError::Conflict(
                    "repair_projection_rollback_source_collision".to_string(),
                ))
            }
        };
        let mut stage_budget = RepairReadBudget::new();
        let stage_identity =
            match inspect_owned_projection_stage(self.stage.as_ref(), &mut stage_budget)? {
                StageContents::Absent
                | StageContents::Present {
                    source: None,
                    rollback_quarantine: None,
                    state: None,
                } => None,
                StageContents::Present {
                    source: Some(bytes),
                    rollback_quarantine: None,
                    state: None,
                } if bytes == source_bytes => {
                    let metadata = self
                        .stage()?
                        .directory
                        .symlink_metadata(PROJECTION_UNLINK_STAGE_FILE)?;
                    Some(projection_file_identity(&metadata)?)
                }
                StageContents::Present { .. } => {
                    return Err(WenlanError::Conflict(
                        "repair_projection_rollback_source_collision".to_string(),
                    ))
                }
            };
        let stage_exists = stage_identity.is_some();
        let stage_prepared = self.stage.is_some();
        let mut quarantine_budget = RepairReadBudget::new();
        let quarantine_exists = match read_optional_regular_nofollow(
            self.orphaned(),
            &self.quarantine_name,
            &mut quarantine_budget,
        ) {
            Ok(Some(bytes)) if bytes == source_bytes => true,
            Ok(None) => false,
            _ => {
                return Err(WenlanError::Conflict(
                    "repair_projection_rollback_quarantine_collision".to_string(),
                ))
            }
        };
        let original = current_state == state_bytes;
        let post = current_state == self.next_state_bytes;
        if !matches!(
            (
                original,
                post,
                source_exists,
                quarantine_exists,
                stage_exists,
                stage_prepared
            ),
            (true, false, true, false, false, true)
                | (true, false, true, true, false, true)
                | (true, false, false, true, false, false)
                | (true, false, false, true, false, true)
                | (false, true, false, true, false, false)
                | (false, true, false, true, false, true)
                | (true, false, false, true, true, true)
                | (false, true, false, true, true, true)
        ) {
            return Err(WenlanError::Conflict(
                "repair_projection_rollback_state_collision".to_string(),
            ));
        }
        if post {
            write_state_atomically_if_matches(
                &self.wenlan,
                self.stage()?,
                &self.next_state_bytes,
                &state_bytes,
                self.state_mode,
                &mut read_budget,
            )?;
            #[cfg(unix)]
            {
                sync_dir_capability(&self.wenlan)?;
                sync_dir_capability(&self.root)?;
            }
        }
        if !source_exists {
            if let Some(stage_identity) = stage_identity {
                self.stage()?
                    .directory
                    .hard_link(
                        PROJECTION_UNLINK_STAGE_FILE,
                        &self.source_parent,
                        &self.source_name,
                    )
                    .map_err(|error| {
                        WenlanError::Io(std::io::Error::new(
                            error.kind(),
                            format!("repair source rollback no-clobber hard link failed: {error}"),
                        ))
                    })?;
                let restored = self.source_parent.symlink_metadata(&self.source_name)?;
                if projection_file_identity(&restored)? != stage_identity {
                    return Err(WenlanError::Conflict(
                        "repair_projection_rollback_source_collision".to_string(),
                    ));
                }
                self.stage()?
                    .directory
                    .remove_file(PROJECTION_UNLINK_STAGE_FILE)?;
            } else {
                write_new_file_nofollow(&self.source_parent, &self.source_name, &source_bytes)?;
            }
            #[cfg(unix)]
            {
                sync_dir_capability(&self.source_parent)?;
                sync_dir_capability(&self.root)?;
            }
        }
        if quarantine_exists {
            self.stage()?.move_quarantine_for_exact_delete(
                self.orphaned(),
                &self.quarantine_name,
                &source_bytes,
                Some(self.expected_source_identity),
            )?;
        }
        sync_projection_dirs(
            &self.root,
            &self.wenlan,
            &self.source_parent,
            self.orphaned(),
        )?;
        if let Some(stage) = self.stage.as_ref() {
            stage.remove_if_empty(&self.wenlan)?;
        }

        self.mutation_started = false;
        Ok(())
    }
}

fn open_relative_parent_nofollow(
    root: &Dir,
    relative: &Path,
) -> Result<(Dir, OsString), WenlanError> {
    let mut components = relative.components().peekable();
    let mut directory = root.try_clone()?;
    while let Some(component) = components.next() {
        let Component::Normal(component) = component else {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        };
        if components.peek().is_none() {
            return Ok((directory, component.to_os_string()));
        }
        directory = directory.open_dir_nofollow(Path::new(component))?;
    }
    Err(WenlanError::Conflict("repair_target_stale".to_string()))
}

fn create_projection_root_nofollow(path: &Path) -> Result<(), WenlanError> {
    let parent = path
        .parent()
        .ok_or_else(|| WenlanError::Validation("repair_projection_root_invalid".to_string()))?;
    let basename = path
        .file_name()
        .ok_or_else(|| WenlanError::Validation("repair_projection_root_invalid".to_string()))?;
    let parent = Dir::open_ambient_dir(parent, cap_std::ambient_authority())?;
    match parent.symlink_metadata(Path::new(basename)) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(WenlanError::Validation(
            "repair_projection_root_invalid".to_string(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match parent.create_dir(Path::new(basename)) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let metadata = parent.symlink_metadata(Path::new(basename))?;
                    if metadata.is_dir() && !metadata.file_type().is_symlink() {
                        Ok(())
                    } else {
                        Err(WenlanError::Validation(
                            "repair_projection_root_invalid".to_string(),
                        ))
                    }
                }
                Err(error) => Err(WenlanError::Io(error)),
            }
        }
        Err(error) => Err(WenlanError::Io(error)),
    }
}

fn repair_projection_rollback_too_large() -> WenlanError {
    WenlanError::Validation("repair_projection_rollback_too_large".to_string())
}

fn charge_repair_read_budget(
    budget: &mut RepairReadBudget,
    amount: u64,
) -> Result<(), WenlanError> {
    budget.remaining = budget
        .remaining
        .checked_sub(amount)
        .ok_or_else(repair_projection_rollback_too_large)?;
    Ok(())
}

fn os_str_storage_len(value: &OsStr) -> Result<u64, WenlanError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        u64::try_from(value.as_bytes().len()).map_err(|_| repair_projection_rollback_too_large())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        u64::try_from(value.encode_wide().count())
            .ok()
            .and_then(|units| units.checked_mul(2))
            .ok_or_else(repair_projection_rollback_too_large)
    }
    #[cfg(not(any(unix, windows)))]
    {
        u64::try_from(value.to_string_lossy().len())
            .map_err(|_| repair_projection_rollback_too_large())
    }
}

fn regular_read_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    options
}

fn read_regular_nofollow(
    directory: &Dir,
    name: &OsStr,
    budget: &mut RepairReadBudget,
) -> Result<Vec<u8>, WenlanError> {
    read_optional_regular_nofollow(directory, name, budget)?
        .ok_or_else(|| WenlanError::Io(std::io::Error::from(std::io::ErrorKind::NotFound)))
}

fn read_optional_regular_nofollow(
    directory: &Dir,
    name: &OsStr,
    budget: &mut RepairReadBudget,
) -> Result<Option<Vec<u8>>, WenlanError> {
    read_optional_regular_nofollow_bounded(directory, name, budget, || Ok(()))
}

fn read_optional_regular_nofollow_bounded(
    directory: &Dir,
    name: &OsStr,
    budget: &mut RepairReadBudget,
    after_metadata: impl FnOnce() -> Result<(), WenlanError>,
) -> Result<Option<Vec<u8>>, WenlanError> {
    let mut file = match directory.open_with(Path::new(name), &regular_read_options()) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(WenlanError::Io(error)),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(WenlanError::Conflict("repair_target_stale".to_string()));
    }
    if metadata.len() > budget.remaining {
        return Err(WenlanError::Validation(
            "repair_projection_rollback_too_large".to_string(),
        ));
    }
    after_metadata()?;
    let max_bytes = usize::try_from(budget.remaining)
        .map_err(|_| WenlanError::Validation("repair_projection_rollback_too_large".to_string()))?;
    let initial_capacity = usize::try_from(metadata.len())
        .unwrap_or(max_bytes)
        .min(max_bytes);
    let mut bytes = Vec::with_capacity(initial_capacity);
    let read_limit = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut limited = (&mut file).take(read_limit);
    let mut buffer = [0_u8; 8192];
    loop {
        let read = limited.read(&mut buffer)?;
        if read == 0 {
            return Ok(Some(bytes));
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|length| length > max_bytes)
        {
            return Err(WenlanError::Validation(
                "repair_projection_rollback_too_large".to_string(),
            ));
        }
        budget.remaining = budget
            .remaining
            .checked_sub(u64::try_from(read).map_err(|_| {
                WenlanError::Validation("repair_projection_rollback_too_large".to_string())
            })?)
            .ok_or_else(|| {
                WenlanError::Validation("repair_projection_rollback_too_large".to_string())
            })?;
        bytes.extend_from_slice(&buffer[..read]);
    }
}

fn read_regular_identity_nofollow_bounded(
    directory: &Dir,
    name: &OsStr,
    budget: &mut RepairReadBudget,
) -> Result<(Vec<u8>, ProjectionFileIdentity), WenlanError> {
    let mut file = directory.open_with(Path::new(name), &regular_read_options())?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > budget.remaining {
        return Err(WenlanError::Conflict("repair_target_stale".to_string()));
    }
    let identity = projection_file_identity(&metadata)?;
    let max_bytes = usize::try_from(budget.remaining)
        .map_err(|_| WenlanError::Validation("repair_projection_rollback_too_large".to_string()))?;
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(max_bytes)
            .min(max_bytes),
    );
    let mut limited = (&mut file).take(budget.remaining.saturating_add(1));
    let mut buffer = [0_u8; 8192];
    loop {
        let read = limited.read(&mut buffer)?;
        if read == 0 {
            return Ok((bytes, identity));
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|length| length > max_bytes)
        {
            return Err(WenlanError::Validation(
                "repair_projection_rollback_too_large".to_string(),
            ));
        }
        budget.remaining = budget
            .remaining
            .checked_sub(u64::try_from(read).map_err(|_| {
                WenlanError::Validation("repair_projection_rollback_too_large".to_string())
            })?)
            .ok_or_else(|| {
                WenlanError::Validation("repair_projection_rollback_too_large".to_string())
            })?;
        bytes.extend_from_slice(&buffer[..read]);
    }
}

fn hash_regular_nofollow_bounded(
    directory: &Dir,
    name: &OsStr,
    budget: &mut RepairReadBudget,
) -> Result<String, WenlanError> {
    let mut file = directory.open_with(Path::new(name), &regular_read_options())?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(WenlanError::Conflict("repair_target_stale".to_string()));
    }
    if metadata.len() > budget.remaining {
        return Err(WenlanError::Validation(
            "repair_projection_rollback_too_large".to_string(),
        ));
    }
    let read_limit = budget.remaining.saturating_add(1);
    let mut limited = (&mut file).take(read_limit);
    let mut digest = sha2::Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = limited.read(&mut buffer)?;
        if read == 0 {
            return Ok(hex::encode(digest.finalize()));
        }
        let read = u64::try_from(read).map_err(|_| {
            WenlanError::Validation("repair_projection_rollback_too_large".to_string())
        })?;
        budget.remaining = budget.remaining.checked_sub(read).ok_or_else(|| {
            WenlanError::Validation("repair_projection_rollback_too_large".to_string())
        })?;
        digest.update(&buffer[..usize::try_from(read).unwrap_or(buffer.len())]);
    }
}

fn read_state_nofollow(
    wenlan: &Dir,
    budget: &mut RepairReadBudget,
) -> Result<(Vec<u8>, ProjectionStateMode), WenlanError> {
    let (bytes, mode, _) =
        read_projection_state_identity_nofollow(wenlan, OsStr::new("state.json"), budget)?;
    Ok((bytes, mode))
}

fn read_optional_projection_state_identity_nofollow(
    directory: &Dir,
    name: &OsStr,
    budget: &mut RepairReadBudget,
) -> Result<Option<(Vec<u8>, ProjectionStateMode, ProjectionFileIdentity)>, WenlanError> {
    match directory.open_with(Path::new(name), &regular_read_options()) {
        Ok(file) => read_projection_state_identity_from_file(file, budget).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(WenlanError::Io(error)),
    }
}

fn read_projection_state_identity_nofollow(
    directory: &Dir,
    name: &OsStr,
    budget: &mut RepairReadBudget,
) -> Result<(Vec<u8>, ProjectionStateMode, ProjectionFileIdentity), WenlanError> {
    let file = directory.open_with(Path::new(name), &regular_read_options())?;
    read_projection_state_identity_from_file(file, budget)
}

fn read_projection_state_identity_from_file(
    mut file: cap_std::fs::File,
    budget: &mut RepairReadBudget,
) -> Result<(Vec<u8>, ProjectionStateMode, ProjectionFileIdentity), WenlanError> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.len() > crate::lint::pages::fs::STATE_MAX_BYTES
        || metadata.len() > budget.remaining
    {
        return Err(WenlanError::Conflict("repair_target_stale".to_string()));
    }
    let max_bytes = budget
        .remaining
        .min(crate::lint::pages::fs::STATE_MAX_BYTES);
    let mut limited = (&mut file).take(max_bytes.saturating_add(1));
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(0)
            .min(4 * 1024 * 1024),
    );
    let mut buffer = [0_u8; 8192];
    loop {
        let read = limited.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let next = bytes.len().checked_add(read).ok_or_else(|| {
            WenlanError::Validation("repair_projection_rollback_too_large".to_string())
        })?;
        if u64::try_from(next).unwrap_or(u64::MAX) > max_bytes {
            return Err(WenlanError::Validation(
                "repair_projection_rollback_too_large".to_string(),
            ));
        }
        budget.remaining = budget
            .remaining
            .checked_sub(u64::try_from(read).map_err(|_| {
                WenlanError::Validation("repair_projection_rollback_too_large".to_string())
            })?)
            .ok_or_else(|| {
                WenlanError::Validation("repair_projection_rollback_too_large".to_string())
            })?;
        bytes.extend_from_slice(&buffer[..read]);
    }
    let identity = projection_file_identity(&metadata)?;
    Ok((bytes, projection_state_mode(&metadata), identity))
}

fn projection_state_mode(metadata: &cap_std::fs::Metadata) -> ProjectionStateMode {
    #[cfg(unix)]
    {
        use cap_std::fs::PermissionsExt as _;
        metadata.permissions().mode()
    }
    #[cfg(not(unix))]
    {
        metadata.permissions().readonly()
    }
}

fn set_projection_state_mode(
    file: &cap_std::fs::File,
    mode: ProjectionStateMode,
) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use cap_std::fs::PermissionsExt as _;
        file.set_permissions(cap_std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let mut permissions = file.metadata()?.permissions();
        permissions.set_readonly(mode);
        file.set_permissions(permissions)
    }
}

fn projection_lock_is_contended(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        // LockFileEx reports ERROR_LOCK_VIOLATION when an immediately-failing
        // byte-range lock collides; Rust does not classify code 33 as WouldBlock.
        error.raw_os_error() == Some(33)
    }
    #[cfg(not(windows))]
    false
}

fn ensure_orphaned_private(orphaned: &Dir) -> Result<(), WenlanError> {
    let metadata = orphaned.dir_metadata()?;
    if !metadata.is_dir() {
        return Err(WenlanError::Conflict("repair_target_stale".to_string()));
    }
    #[cfg(unix)]
    {
        use cap_std::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(WenlanError::Conflict("repair_target_stale".to_string()));
        }
    }
    Ok(())
}

fn write_new_file_nofollow(directory: &Dir, name: &OsStr, bytes: &[u8]) -> Result<(), WenlanError> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let mut file = directory.open_with(Path::new(name), &options)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Atomic, symlink-refusing page write: temp file in the same capability
/// directory, fsync, then rename over the target.
///
/// `write_regular_nofollow` truncates the target and writes in place, so a
/// concurrent reader can observe a short or empty page. A rename within one
/// directory is atomic: a reader sees the old bytes or the new ones, never a
/// torn file. `reconcile` repairs the remaining failure mode (temp written,
/// rename never happened), which leaves the target's `origin_version` behind
/// the DB's.
fn projection_digest(bytes: &[u8]) -> String {
    format!("{:x}", sha2::Sha256::digest(bytes))
}

fn projection_bytes_replaceable(
    bytes: &[u8],
    page: &Page,
    baseline: Option<&PageFileState>,
) -> bool {
    if bytes.is_empty() {
        return true; // Retain the torn inode, then repair from DB.
    }
    let Ok(raw) = std::str::from_utf8(bytes) else {
        return false;
    };
    let (fm, body) = crate::sources::obsidian::extract_frontmatter(raw);
    let owned = fm
        .get_str("origin_id")
        .map_or(baseline.is_some(), |id| id == page.id);
    let not_newer = projected_origin_version(&fm) <= page.version;
    let unchanged = baseline.and_then(|entry| entry.content_sha256.as_deref())
        == Some(projection_digest(bytes).as_str());
    let rendered = render_markdown(page);
    let (_, rendered_body) = crate::sources::obsidian::extract_frontmatter(&rendered);
    let adopted = crate::export::provenance::canonicalize_page_body(body)
        == crate::export::provenance::canonicalize_page_body(rendered_body);
    owned && not_newer && (unchanged || adopted)
}

/// Move the old inode into a durable recovery directory before installing a
/// new file. A check followed by rename-over would lose an editor save that
/// landed between them. Here a replacement appearing at the original name
/// makes the create-only hard link fail, and the displaced inode remains
/// reachable even if an editor still has it open and writes to it later.
/// Readers can briefly see a missing path; they never see truncated prose.
fn write_page_preserving_external_edits(
    capabilities: &ProjectionCapabilities,
    name: &str,
    temporary: &str,
    page: &Page,
    bytes: &[u8],
    baseline: Option<&PageFileState>,
) -> Result<(), WenlanError> {
    write_page_preserving_external_edits_with_hook(
        capabilities,
        name,
        temporary,
        page,
        bytes,
        baseline,
        || Ok(()),
    )
}

#[allow(clippy::too_many_arguments)]
fn write_page_preserving_external_edits_with_hook(
    capabilities: &ProjectionCapabilities,
    name: &str,
    temporary: &str,
    page: &Page,
    bytes: &[u8],
    baseline: Option<&PageFileState>,
    after_move: impl FnOnce() -> Result<(), WenlanError>,
) -> Result<(), WenlanError> {
    let (directory, basename) = folders::page_parent(&capabilities.root, name)?;
    let root = &directory;
    let name = basename.as_str();
    // Avoid archiving an unchanged file on every receipt replay. State may
    // still need repairing if the previous write stopped after installation.
    let mut budget = RepairReadBudget::new();
    if let Some(current) = read_optional_regular_nofollow(root, OsStr::new(name), &mut budget)? {
        if current == bytes {
            return Ok(());
        }
        if !projection_bytes_replaceable(&current, page, baseline) {
            return Err(WenlanError::Conflict(format!(
                "page_projection_external_edit: preserved {name}"
            )));
        }
    }
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let mut file = root.open_with(temporary, &options)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    let result = (|| {
        match capabilities.wenlan.create_dir("projection-recovery") {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let recovery = capabilities
            .wenlan
            .open_dir_nofollow("projection-recovery")?;
        let previous = format!("{}.{}.md", page.id, uuid::Uuid::new_v4());
        // The source may change after the initial read. Validate the actual
        // inode moved, not the one observed before that race.
        match root.rename(name, &recovery, &previous) {
            Ok(()) => {
                // Test seam at the real race boundary; production is a no-op.
                let moved_hook = after_move();
                let mut budget = RepairReadBudget::new();
                let allowed = (|| {
                    moved_hook?;
                    let displaced =
                        read_regular_nofollow(&recovery, OsStr::new(&previous), &mut budget)?;
                    Ok::<_, WenlanError>(projection_bytes_replaceable(&displaced, page, baseline))
                })();
                if !matches!(allowed, Ok(true)) {
                    // Do not rename-over a new editor file while restoring.
                    // If one appeared, both it and the displaced file survive.
                    let _ = recovery.hard_link(&previous, root, name);
                    return Err(WenlanError::Conflict(format!(
                        "page_projection_external_edit: preserved {name} and .wenlan/projection-recovery/{previous}"
                    )));
                }
                if let Err(error) = root.hard_link(temporary, root, name) {
                    let _ = recovery.hard_link(&previous, root, name);
                    return Err(error.into());
                }
                // Retain the displaced inode: an external editor may still
                // write through an open handle after the new file is installed.
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                root.hard_link(temporary, root, name)?;
            }
            Err(error) => return Err(error.into()),
        }
        Ok(())
    })();
    let _ = root.remove_file(temporary);
    result
}

pub(crate) fn write_page_atomically_nofollow(
    directory: &Dir,
    name: &str,
    temporary: &str,
    bytes: &[u8],
) -> Result<(), WenlanError> {
    write_page_atomically_nofollow_with_hook(directory, name, temporary, bytes, || Ok(()))
}

pub(crate) fn write_page_atomically_nofollow_with_hook(
    directory: &Dir,
    name: &str,
    temporary: &str,
    bytes: &[u8],
    after_temp_write: impl FnOnce() -> std::io::Result<()>,
) -> Result<(), WenlanError> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        // A stopped process must not leave private state in a readable temp.
        options.mode(0o600);
    }
    let result = (|| {
        let permissions = match directory.open_with(name, &regular_read_options()) {
            Ok(current) => {
                let metadata = current.metadata()?;
                if !metadata.is_file() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "page_projection_target_invalid",
                    ));
                }
                Some(metadata.permissions())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        let mut file = directory.open_with(temporary, &options)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        after_temp_write()?;
        // Retain the existing nofollow regular-target check at installation.
        match directory.open_with(name, &regular_read_options()) {
            Ok(current) if !current.metadata()?.is_file() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "page_projection_target_invalid",
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        directory.rename(temporary, directory, name)?;
        Ok::<(), std::io::Error>(())
    })();
    if result.is_err() {
        let _ = directory.remove_file(temporary);
    }
    result.map_err(WenlanError::Io)
}

fn write_regular_nofollow(directory: &Dir, name: &str, bytes: &[u8]) -> Result<(), WenlanError> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create(true)
        .truncate(true)
        .follow(FollowSymlinks::No);
    let mut file = directory.open_with(name, &options)?;
    if !file.metadata()?.is_file() {
        return Err(WenlanError::Conflict(
            "page_projection_target_invalid".to_string(),
        ));
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn write_state_atomically(
    wenlan: &Dir,
    bytes: &[u8],
    mode: ProjectionStateMode,
) -> Result<(), WenlanError> {
    let sequence = PROJECTION_STATE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = format!(".projection-state-{}-{sequence}.tmp", std::process::id());
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let result = (|| {
        let mut file = wenlan.open_with(&temporary, &options)?;
        file.write_all(bytes)?;
        set_projection_state_mode(&file, mode)?;
        file.sync_all()?;
        let current = wenlan.open_with("state.json", &regular_read_options())?;
        if !current.metadata()?.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection state is not a regular file",
            ));
        }
        wenlan.rename(&temporary, wenlan, "state.json")?;
        Ok::<(), std::io::Error>(())
    })();
    if result.is_err() {
        let _ = wenlan.remove_file(&temporary);
    }
    result.map_err(WenlanError::Io)
}

fn write_state_atomically_if_matches(
    wenlan: &Dir,
    stage: &OwnedProjectionStage,
    expected: &[u8],
    replacement: &[u8],
    mode: ProjectionStateMode,
    budget: &mut RepairReadBudget,
) -> Result<(), WenlanError> {
    write_state_atomically_if_matches_with_hook(
        wenlan,
        stage,
        expected,
        replacement,
        mode,
        budget,
        || Ok(()),
    )
}

fn write_state_atomically_if_matches_with_hook<B>(
    wenlan: &Dir,
    stage: &OwnedProjectionStage,
    expected: &[u8],
    replacement: &[u8],
    mode: ProjectionStateMode,
    budget: &mut RepairReadBudget,
    before_swap: B,
) -> Result<(), WenlanError>
where
    B: FnOnce() -> Result<(), WenlanError>,
{
    let (current, current_mode, current_identity) =
        read_projection_state_identity_nofollow(wenlan, OsStr::new("state.json"), budget)?;
    if current != expected || current_mode != mode {
        return Err(WenlanError::Conflict(
            "repair_projection_rollback_state_collision".to_string(),
        ));
    }
    let mut stage_budget = RepairReadBudget::new();
    if matches!(
        inspect_owned_projection_stage(Some(stage), &mut stage_budget)?,
        StageContents::Present { state: Some(_), .. }
    ) {
        return Err(WenlanError::Conflict(
            "repair_apply_recovery_required".to_string(),
        ));
    }

    let sequence = PROJECTION_STATE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = format!(".projection-state-{}-{sequence}.tmp", std::process::id());
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let mut temporary_file = wenlan.open_with(&temporary, &options)?;
    temporary_file.write_all(replacement)?;
    set_projection_state_mode(&temporary_file, mode)?;
    temporary_file.sync_all()?;
    let temporary_identity = projection_file_identity(&temporary_file.metadata()?)?;
    drop(temporary_file);

    let result = (|| {
        before_swap()?;
        wenlan.rename("state.json", &stage.directory, PROJECTION_STATE_STAGE_FILE)?;
        #[cfg(unix)]
        {
            sync_dir_capability(wenlan)?;
            sync_dir_capability(&stage.directory)?;
        }

        let mut moved_budget = RepairReadBudget::new();
        let (moved, moved_mode, moved_identity) = read_projection_state_identity_nofollow(
            &stage.directory,
            OsStr::new(PROJECTION_STATE_STAGE_FILE),
            &mut moved_budget,
        )?;
        if moved != expected || moved_mode != mode || moved_identity != current_identity {
            match stage
                .directory
                .hard_link(PROJECTION_STATE_STAGE_FILE, wenlan, "state.json")
            {
                Ok(()) => {
                    let mut restored_budget = RepairReadBudget::new();
                    let (_, _, restored_identity) = read_projection_state_identity_nofollow(
                        wenlan,
                        OsStr::new("state.json"),
                        &mut restored_budget,
                    )?;
                    if restored_identity != moved_identity {
                        return Err(WenlanError::Conflict(
                            "repair_apply_recovery_required".to_string(),
                        ));
                    }
                    #[cfg(unix)]
                    sync_dir_capability(wenlan)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(WenlanError::Io(error)),
            }
            return Err(WenlanError::Conflict(
                "repair_apply_recovery_required".to_string(),
            ));
        }

        match wenlan.hard_link(&temporary, wenlan, "state.json") {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(WenlanError::Conflict(
                    "repair_apply_recovery_required".to_string(),
                ))
            }
            Err(error) => return Err(WenlanError::Io(error)),
        }
        let mut installed_budget = RepairReadBudget::new();
        let (installed, installed_mode, installed_identity) =
            read_projection_state_identity_nofollow(
                wenlan,
                OsStr::new("state.json"),
                &mut installed_budget,
            )?;
        if installed != replacement
            || installed_mode != mode
            || installed_identity != temporary_identity
        {
            return Err(WenlanError::Conflict(
                "repair_apply_recovery_required".to_string(),
            ));
        }

        let mut staged_budget = RepairReadBudget::new();
        let (staged, staged_mode, staged_identity) = read_projection_state_identity_nofollow(
            &stage.directory,
            OsStr::new(PROJECTION_STATE_STAGE_FILE),
            &mut staged_budget,
        )?;
        if staged != expected || staged_mode != mode || staged_identity != current_identity {
            return Err(WenlanError::Conflict(
                "repair_apply_recovery_required".to_string(),
            ));
        }
        stage.directory.remove_file(PROJECTION_STATE_STAGE_FILE)?;
        #[cfg(unix)]
        {
            sync_dir_capability(wenlan)?;
            sync_dir_capability(&stage.directory)?;
        }
        Ok(())
    })();

    let temporary_cleanup = wenlan.remove_file(&temporary);
    match (result, temporary_cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(WenlanError::Io(error)),
        (Err(error), _) => Err(error),
    }
}

fn sync_projection_dirs(
    root: &Dir,
    wenlan: &Dir,
    source_parent: &Dir,
    orphaned: &Dir,
) -> Result<(), WenlanError> {
    #[cfg(not(unix))]
    let _ = (root, wenlan, source_parent, orphaned);
    #[cfg(unix)]
    {
        sync_dir_capability(orphaned)?;
        sync_dir_capability(source_parent)?;
        sync_dir_capability(wenlan)?;
        sync_dir_capability(root)?;
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn sync_dir_capability(directory: &Dir) -> Result<(), WenlanError> {
    // cap-std opens directory capabilities with O_PATH on Linux. Cloning that
    // descriptor preserves O_PATH, which cannot be fsynced (EBADF). Reopen the
    // same directory through the capability to obtain a syncable descriptor.
    directory.open(".")?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn sync_dir_capability(_directory: &Dir) -> Result<(), WenlanError> {
    // Other platforms keep regular-file syncing; directory metadata sync is Unix-only.
    Ok(())
}

impl KnowledgeProjectionWrite {
    pub fn new(path: PathBuf, database: &crate::db::MemoryDB) -> Self {
        Self {
            writer: KnowledgeWriter::new(path, database),
            guard: database.begin_page_projection_write(),
        }
    }

    pub(crate) fn new_repair(path: PathBuf, database: &crate::db::MemoryDB) -> Self {
        Self {
            writer: KnowledgeWriter::new_repair(path, database),
            guard: database.begin_page_projection_write(),
        }
    }

    pub(crate) fn with_repair_lock<T, F>(
        path: PathBuf,
        database: &crate::db::MemoryDB,
        operation: F,
    ) -> Result<T, WenlanError>
    where
        F: FnOnce(&LockedRepairProjection<'_>) -> Result<T, WenlanError>,
    {
        let write = Self::new_repair(path, database);
        Self::with_projection_capabilities(&write.writer.path, |capabilities| {
            operation(&LockedRepairProjection {
                write: &write,
                capabilities,
            })
        })
    }

    pub(crate) fn with_projection_lock<T, F>(path: &Path, operation: F) -> Result<T, WenlanError>
    where
        F: FnOnce(&LockedProjection<'_>) -> Result<T, WenlanError>,
    {
        Self::with_projection_capabilities(path, |capabilities| {
            operation(&LockedProjection { capabilities })
        })
    }

    pub(crate) fn begin_owned_repair_session(
        path: PathBuf,
        database: &crate::db::MemoryDB,
    ) -> Result<OwnedRepairProjectionSession, WenlanError> {
        let write = Self::new_repair(path, database);
        let _write_lock = PROJECTION_WRITE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let capabilities = ProjectionCapabilities::open(&write.writer.path)?;
        folders::recover_move(&capabilities)?;
        Ok(OwnedRepairProjectionSession {
            write,
            capabilities,
        })
    }

    fn with_projection_capabilities<T, F>(path: &Path, operation: F) -> Result<T, WenlanError>
    where
        F: FnOnce(&ProjectionCapabilities) -> Result<T, WenlanError>,
    {
        let _write_lock = PROJECTION_WRITE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let capabilities = ProjectionCapabilities::open(path)?;
        folders::recover_move(&capabilities)?;
        operation(&capabilities)
    }

    pub fn write_page(&self, page: &Page) -> Result<String, WenlanError> {
        self.writer.write_page(&self.guard, page)
    }

    pub fn write_page_in_folder(
        &self,
        page: &Page,
        initial_folder: Option<&str>,
    ) -> Result<String, WenlanError> {
        self.writer
            .write_page_in_folder(&self.guard, page, initial_folder)
    }

    /// Project a page only if an automatic reader may see it.
    ///
    /// PR-C's write-time half. The removal pass is still the load-bearing one --
    /// migration 99 backfilled every pre-existing page to `provisional`, so a
    /// writer that merely declined new writes would leave the entire existing
    /// corpus readable, which is exactly the weakening the binding spec's section
    /// 9 names. But removal alone holds the invariant only across uptime: pages
    /// are projected at runtime, so after the cutover each newly written
    /// provisional page would be readable by `wenlan pages` until the next
    /// restart. The two together make it hold continuously.
    ///
    /// `Ok(None)` means the page was not projected, and that is the contract
    /// working rather than a failure -- callers log it and carry on. At
    /// generation 0 the permit is always granted, so every caller behaves exactly
    /// as it did before.
    pub async fn write_page_gated(
        &self,
        database: &crate::db::MemoryDB,
        page: &Page,
    ) -> Result<Option<String>, WenlanError> {
        let Some(permit) = crate::truth_adapter::page_write_permit(database, &page.id).await?
        else {
            log::info!(
                "[truth] not projecting page {} — an automatic reader may not see it, and \
                 `wenlan pages` reads this directory with no gate in front of it",
                page.id
            );
            return Ok(None);
        };
        let folder = database.page_initial_folder_path(&page.id).await?;
        check_permit(&permit, page)?;
        self.writer
            .write_page_in_folder(&self.guard, page, Some(&folder))
            .map(Some)
    }

    /// Project a page against a permit already in hand.
    ///
    /// The permit's page is checked against the page being written: a permit for
    /// page A is not permission to write page B, and nothing but this check
    /// stands between those two after a refactor moves one of them.
    pub fn write_page_permitted(
        &self,
        permit: &crate::truth_adapter::PagePermit,
        page: &Page,
    ) -> Result<String, WenlanError> {
        check_permit(permit, page)?;
        self.write_page(page)
    }

    pub fn remove_page(&self, page_id: &str) -> Result<(), WenlanError> {
        self.writer.remove_page(&self.guard, page_id)
    }

    /// Repair the projection from the DB.
    ///
    /// Deliberately NOT truth-gated. It rewrites files for pages already on
    /// disk, and the daemon runs
    /// [`Self::enforce_projection_directory_invariant`] immediately after it in
    /// the same startup block -- so a page this pass re-projects and the truth
    /// verdict hides is evicted in the same tick. Gating here as well would only
    /// move where the same file stops existing.
    pub fn reconcile(&self, pages: &[Page]) -> Result<ReconcileStats, WenlanError> {
        self.writer.reconcile(&self.guard, pages)
    }

    /// See [`KnowledgeWriter::enforce_projection_directory_invariant`].
    pub async fn enforce_projection_directory_invariant(
        &self,
        database: &crate::db::MemoryDB,
    ) -> Result<usize, WenlanError> {
        self.writer
            .enforce_projection_directory_invariant(database, &self.guard)
            .await
    }
}

impl OwnedRepairProjectionSession {
    pub(crate) fn locked(&self) -> LockedRepairProjection<'_> {
        LockedRepairProjection {
            write: &self.write,
            capabilities: &self.capabilities,
        }
    }
}

struct ProjectionCapabilities {
    root: Dir,
    wenlan: Dir,
    // Advisory only: processes which do not participate in this protocol can
    // still mutate the Page tree. Every Wenlan projection writer acquires it
    // after `PROJECTION_WRITE_LOCK`, so cooperating processes fail closed.
    // Normal writers still use their existing ambient filesystem operations;
    // participating in this lock does not make those operations capability-safe.
    _file_lock: std::fs::File,
}

impl ProjectionCapabilities {
    fn open(path: &Path) -> Result<Self, WenlanError> {
        let parent = path
            .parent()
            .ok_or_else(|| WenlanError::Validation("repair_projection_root_invalid".to_string()))?;
        let basename = path
            .file_name()
            .ok_or_else(|| WenlanError::Validation("repair_projection_root_invalid".to_string()))?;
        let ambient_parent = Dir::open_ambient_dir(parent, cap_std::ambient_authority())?;
        let root_metadata = ambient_parent.symlink_metadata(Path::new(basename))?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(WenlanError::Validation(
                "repair_projection_root_invalid".to_string(),
            ));
        }
        let root = ambient_parent.open_dir_nofollow(Path::new(basename))?;
        match root.symlink_metadata(".wenlan") {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(WenlanError::Conflict(
                    "repair_projection_control_invalid".to_string(),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                root.create_dir(".wenlan")?;
            }
            Err(error) => return Err(WenlanError::Io(error)),
        }
        let wenlan = root.open_dir_nofollow(".wenlan")?;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .follow(FollowSymlinks::No);
        let file_lock = wenlan.open_with(".projection.lock", &options)?.into_std();
        match file_lock.try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if projection_lock_is_contended(&error) => {
                return Err(WenlanError::Conflict("page_projection_locked".to_string()))
            }
            Err(error) => return Err(WenlanError::Io(error)),
        }
        Ok(Self {
            root,
            wenlan,
            _file_lock: file_lock,
        })
    }
}

/// Public projection of a page's markdown (frontmatter + body + the delimiter
/// Sources block), so callers outside this module (the watcher's
/// protected-block re-projection check) can compute the canonical projection
/// without duplicating it.
pub fn render_markdown_for(page: &Page) -> String {
    render_markdown(page)
}

/// The `state.json` entry `write_page` records for `page` at `file`, as JSON.
/// Callers that compare a captured `state.json` against the expected
/// post-write entry (the rename-repair recovery matcher) go through this so
/// new `PageFileState` fields stay in sync with the writer.
pub(crate) fn page_file_state_value(page: &Page, file: &str) -> serde_json::Value {
    serde_json::to_value(PageFileState {
        file: file.to_string(),
        content_sha256: Some(projection_digest(render_markdown(page).as_bytes())),
        version: page.version,
        last_written: page.last_modified.clone(),
        title: Some(page.title.clone()),
        description: page.summary.clone(),
        space: page.space.clone(),
    })
    .expect("PageFileState serializes")
}

/// Shared OKF page frontmatter (the lines between the `---` delimiters), so
/// the md projection and the OKF export bundle cannot drift apart.
///
/// `include_related` is true for the projection only: the bundle omits
/// `related:` because its converted body links already carry the edges.
/// Every other line is identical in both outputs.
pub(crate) fn page_frontmatter(
    page: &Page,
    include_related: bool,
    code_bases: &[CodeBasisLocator],
) -> String {
    use crate::export::provenance::{related_frontmatter, sources_frontmatter, yaml_quoted};
    let mut out = String::new();
    out.push_str(&format!("title: {}\n", yaml_quoted(&page.title)));
    out.push_str("type: page\n");
    if let Some(ref summary) = page.summary {
        out.push_str(&format!("description: {}\n", yaml_quoted(summary)));
    }
    if let Some(ref space) = page.space {
        out.push_str(&format!("tags: [{}]\n", yaml_quoted(space)));
        out.push_str(&format!("space: {}\n", yaml_quoted(space)));
    }
    out.push_str(&format!("origin_id: {}\n", page.id));
    out.push_str(&format!("origin_version: {}\n", page.version));
    let created_date: String = page.created_at.chars().take(10).collect();
    let modified_date: String = page.last_modified.chars().take(10).collect();
    out.push_str(&format!("created: {}\n", created_date));
    out.push_str(&format!("modified: {}\n", modified_date));
    // `generated.at` is the content's LAST meaningful change, not its birth:
    // OKF spec 5.2 says consumers read it "to tell a recent edit from a stale
    // fact". Stamping `created_at` reported a page distilled in April as
    // April-fresh however many times it had been rewritten since, which is the
    // one question the field exists to answer.
    out.push_str(&format!(
        "generated: {{by: {}, at: {}}}\n",
        yaml_quoted(&format!("wenlan/{}", crate::version())),
        yaml_quoted(&page.last_modified)
    ));
    let status = if page.stale_reason.is_some() {
        "draft"
    } else if page.status != "active" {
        "deprecated"
    } else {
        "stable"
    };
    out.push_str(&format!("status: {status}\n"));
    // `verified:` (review_status == "confirmed") is intentionally omitted: no
    // confirmation-timestamp column exists on `pages` (verified by grepping
    // migrations and `Page` for `confirmed_at`/`reviewed_at`), and the spec
    // forbids fabricating `at`. Add the family back once that column lands.
    // Read-only provenance projection (one-way; the watcher never reads it back).
    out.push_str(&sources_frontmatter(&page.source_memory_ids, code_bases));
    if include_related {
        let related = related_page_titles(&page.content);
        out.push_str(&related_frontmatter(&related));
    }
    out
}

fn render_markdown(page: &Page) -> String {
    use crate::export::provenance::render_sources_block;
    let mut out = String::new();

    // Frontmatter — OKF v0.2 conformant (see specs/2026-09-16-okf-projection.md).
    out.push_str("---\n");
    // The projection never lists code bases: their rows change outside page
    // writes, so a `repo://` line here would make the file read as edited.
    out.push_str(&page_frontmatter(page, true, &[]));
    out.push_str("---\n\n");

    // Body with wikilinks
    out.push_str(&convert_links_to_wikilinks(&page.content));

    // Export-only delimiter-wrapped Sources block, generated from DB truth.
    let block = render_sources_block(&page.source_memory_ids);
    if !block.is_empty() {
        out.push_str("\n\n");
        out.push_str(&block);
    } else {
        out.push('\n');
    }

    out
}

/// Page-to-page wikilink targets in the body, for the read-only `related:`
/// frontmatter. `mem_*` targets are excluded — those are provenance, not
/// topic links.
fn related_page_titles(content: &str) -> Vec<String> {
    let wikified = convert_links_to_wikilinks(content);
    crate::sources::obsidian::extract_wikilinks(&wikified)
        .into_iter()
        .map(|w| w.target)
        .filter(|t| !t.starts_with("mem_"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::Page;

    #[test]
    fn live_page_filenames_verifies_only_allowed_existing_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        writer.write_page_for_test(&page).unwrap();
        let filename = writer.page_filename(&page.id).unwrap();
        let state_path = dir.path().join(".wenlan/state.json");
        let state_before = std::fs::read(&state_path).unwrap();
        page.title = "Renamed in the database".into();
        let found = writer.live_page_filenames(&[page.id.as_str(), "not-authorized"]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[&page.id], filename);
        assert!(writer.live_page_filenames(&["not-authorized"]).is_empty());
        assert_eq!(std::fs::read(&state_path).unwrap(), state_before);
        std::fs::write(
            dir.path().join(&filename),
            "---\norigin_id: another-page\n---\nBody\n",
        )
        .unwrap();
        assert!(writer.live_page_filenames(&[&page.id]).is_empty());
        std::fs::remove_file(dir.path().join(&filename)).unwrap();
        assert!(writer.live_page_filenames(&[&page.id]).is_empty());
        std::fs::create_dir(dir.path().join(&filename)).unwrap();
        assert!(writer.live_page_filenames(&[&page.id]).is_empty());
    }

    #[test]
    fn live_page_filenames_rejects_unsafe_names_and_never_initializes_a_root() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let writer = KnowledgeWriter::new_for_test(missing.clone());
        assert!(writer.live_page_filenames(&["page_test"]).is_empty());
        assert!(!missing.exists());

        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let page = test_concept();
        writer.write_page_for_test(&page).unwrap();
        let mut state = writer.load_state();
        for filename in [
            "../outside.md",
            "/absolute.md",
            "folder/nested.md",
            "folder\\nested.md",
            "C:stream.md",
            ".hidden.md",
            "not-markdown.txt",
            "",
        ] {
            state.pages.get_mut(&page.id).unwrap().file = filename.into();
            writer.save_state(&state).unwrap();
            assert!(
                writer.live_page_filenames(&[&page.id]).is_empty(),
                "{filename}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn live_page_filenames_refuses_symlinked_files_roots_and_state() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("pages");
        let writer = KnowledgeWriter::new_for_test(root.clone());
        let page = test_concept();
        writer.write_page_for_test(&page).unwrap();
        let filename = writer.page_filename(&page.id).unwrap();
        let external = dir.path().join("external.md");
        std::fs::rename(root.join(&filename), &external).unwrap();
        symlink(&external, root.join(&filename)).unwrap();
        assert!(writer.live_page_filenames(&[&page.id]).is_empty());
        std::fs::remove_file(root.join(&filename)).unwrap();
        std::fs::rename(&external, root.join(&filename)).unwrap();

        let alias = dir.path().join("alias");
        symlink(&root, &alias).unwrap();
        assert!(KnowledgeWriter::new_for_test(alias)
            .live_page_filenames(&[&page.id])
            .is_empty());
        let state_path = root.join(".wenlan/state.json");
        let external_state = dir.path().join("state.json");
        std::fs::rename(&state_path, &external_state).unwrap();
        symlink(&external_state, &state_path).unwrap();
        assert!(writer.live_page_filenames(&[&page.id]).is_empty());
        assert!(!dir.path().join(".wenlan").exists());
    }

    pub(super) fn test_concept() -> Page {
        Page {
            id: "concept_test123".to_string(),
            title: "Rust Ownership".to_string(),
            summary: Some("Memory safety without GC".to_string()),
            content: "## Overview\nRust uses ownership for memory safety.\n\n## Related\n- [Borrowing](concept_borrow1)".to_string(),
            entity_id: None,
            space: Some("rust".to_string()),
            source_memory_ids: vec!["m1".to_string()],
            version: 2,
            status: "active".to_string(),
            created_at: "2026-04-01T00:00:00+00:00".to_string(),
            last_compiled: "2026-04-09T00:00:00+00:00".to_string(),
            last_modified: "2026-04-09T00:00:00+00:00".to_string(),
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
            creation_kind: "distilled".to_string(),
            review_status: "confirmed".to_string(),
            workspace: None,
            citations: Vec::new(),
            kind: "concept".to_string(),
            truth: None,
        }
    }

    #[cfg(windows)]
    #[test]
    #[allow(
        clippy::permissions_set_readonly_false,
        reason = "Windows test cleanup clears the DOS readonly attribute; the lint warns about Unix semantics"
    )]
    fn projection_state_mode_tracks_windows_readonly_attribute() {
        let root = tempfile::TempDir::new().unwrap();
        let state_path = root.path().join("state.json");
        std::fs::write(&state_path, b"{}").unwrap();
        let directory = Dir::open_ambient_dir(root.path(), cap_std::ambient_authority()).unwrap();

        let mut budget = RepairReadBudget::new();
        let (_, writable_mode, _) = read_projection_state_identity_nofollow(
            &directory,
            OsStr::new("state.json"),
            &mut budget,
        )
        .unwrap();
        assert!(!writable_mode);

        let mut permissions = std::fs::metadata(&state_path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&state_path, permissions).unwrap();
        let mut budget = RepairReadBudget::new();
        let (_, readonly_mode, _) = read_projection_state_identity_nofollow(
            &directory,
            OsStr::new("state.json"),
            &mut budget,
        )
        .unwrap();
        assert!(readonly_mode);

        let mut permissions = std::fs::metadata(&state_path).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&state_path, permissions).unwrap();
    }

    #[tokio::test]
    async fn repair_projection_lock_covers_the_whole_transaction() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let page_root = tempfile::TempDir::new().unwrap();

        KnowledgeProjectionWrite::with_repair_lock(
            page_root.path().to_path_buf(),
            &db,
            |_repair| {
                let excluded = std::thread::spawn(|| {
                    matches!(
                        PROJECTION_WRITE_LOCK.try_lock(),
                        Err(std::sync::TryLockError::WouldBlock)
                    )
                })
                .join()
                .unwrap();
                assert!(excluded);
                Ok(())
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn projection_file_lock_rejects_second_process() {
        const CHILD_ROOT: &str = "WENLAN_PROJECTION_LOCK_TEST_CHILD_ROOT";
        const CHILD_MODE: &str = "WENLAN_PROJECTION_LOCK_TEST_CHILD_MODE";
        if let Some(page_root) = std::env::var_os(CHILD_ROOT) {
            let page_root = PathBuf::from(page_root);
            let error = match std::env::var(CHILD_MODE).as_deref() {
                Ok("repair") => {
                    let (db, _dir) = crate::db::tests::test_db().await;
                    KnowledgeProjectionWrite::with_repair_lock(page_root, &db, |_repair| Ok(()))
                        .expect_err("a second repair process must not acquire the projection lock")
                }
                Ok("write") => KnowledgeWriter::new_for_test(page_root)
                    .write_page_for_test(&test_concept())
                    .expect_err("a normal writer must participate in the projection lock"),
                Ok("remove") => KnowledgeWriter::new_for_test(page_root)
                    .remove_page_for_test("page_absent")
                    .expect_err("a normal remover must participate in the projection lock"),
                mode => panic!("unexpected child lock mode: {mode:?}"),
            };
            assert_eq!(error.to_string(), "Conflict: page_projection_locked");
            return;
        }

        let (db, _dir) = crate::db::tests::test_db().await;
        let page_root = tempfile::TempDir::new().unwrap();
        KnowledgeProjectionWrite::with_repair_lock(
            page_root.path().to_path_buf(),
            &db,
            |_repair| {
                for mode in ["repair", "write", "remove"] {
                    let output = std::process::Command::new(std::env::current_exe().unwrap())
                        .args([
                            "--exact",
                            "export::knowledge::tests::projection_file_lock_rejects_second_process",
                            "--nocapture",
                        ])
                        .env(CHILD_ROOT, page_root.path())
                        .env(CHILD_MODE, mode)
                        .output()
                        .unwrap();
                    assert!(
                        output.status.success(),
                        "{mode} child lock probe failed:\nstdout:\n{}\nstderr:\n{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr),
                    );
                }
                Ok(())
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn repair_quarantine_rejects_duplicate_state_keys_before_mutation() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let cases = [
            br#"{"schema_version":2,"pages":{"page_other":{"file":"other.md"}},"pages":{"page_duplicate":{"file":"target.md"}}}"#
                .as_slice(),
            br#"{"schema_version":2,"pages":{"page_duplicate":{"file":"other.md"},"page_duplicate":{"file":"target.md"}}}"#
                .as_slice(),
            br#"{"schema_version":2,"pages":{"page_duplicate":{"file":"other.md","file":"target.md","version":1}}}"#
                .as_slice(),
            br#"{"schema_version":2,"pages":{"page_duplicate":{"file":"target.md","version":0,"version":1}}}"#
                .as_slice(),
        ];

        for raw_state in cases {
            let page_root = tempfile::TempDir::new().unwrap();
            std::fs::create_dir_all(page_root.path().join(".wenlan")).unwrap();
            std::fs::write(page_root.path().join(".wenlan/state.json"), raw_state).unwrap();
            std::fs::write(page_root.path().join("target.md"), b"target bytes").unwrap();

            let error = KnowledgeProjectionWrite::with_repair_lock(
                page_root.path().to_path_buf(),
                &db,
                |repair| {
                    repair.quarantine_stale_page(
                        "page_duplicate",
                        "target.md",
                        ".wenlan/orphaned/page_duplicate.md",
                    )
                },
            )
            .unwrap_err();

            assert!(error.to_string().contains("repair_target_stale"));
            assert_eq!(
                std::fs::read(page_root.path().join(".wenlan/state.json")).unwrap(),
                raw_state
            );
            assert_eq!(
                std::fs::read(page_root.path().join("target.md")).unwrap(),
                b"target bytes"
            );
            assert!(!page_root.path().join(".wenlan/orphaned").exists());
        }
    }

    #[tokio::test]
    async fn stale_projection_source_reappearing_after_stage_preserves_all_versions() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let page_root = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(page_root.path().join(".wenlan")).unwrap();
        let raw_state =
            br#"{"schema_version":2,"pages":{"page_reappears":{"file":"target.md","version":1}}}"#;
        let approved = b"---\norigin_id: page_reappears\norigin_version: 1\n---\napproved\n";
        let replacement = b"---\norigin_id: page_reappears\norigin_version: 1\n---\nreplacement\n";
        std::fs::write(page_root.path().join(".wenlan/state.json"), raw_state).unwrap();
        std::fs::write(page_root.path().join("target.md"), approved).unwrap();
        let rollback = crate::repair::capture_stale_page_projection_current(
            page_root.path(),
            "page_reappears",
            "target.md",
            ".wenlan/orphaned/page_reappears.md",
        )
        .unwrap();

        let error = KnowledgeProjectionWrite::with_repair_lock(
            page_root.path().to_path_buf(),
            &db,
            |repair| {
                let mut pinned = repair.pin_stale_page_projection(
                    "page_reappears",
                    "target.md",
                    ".wenlan/orphaned/page_reappears.md",
                    &rollback,
                    "manifest_source_reappears",
                )?;
                let source = page_root.path().join("target.md");
                let result = pinned.quarantine_with_after_source_stage(|| {
                    std::fs::write(&source, replacement)?;
                    Ok(())
                });
                assert!(pinned.mutation_started());
                result
            },
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Conflict: repair_apply_recovery_required"
        );
        assert_eq!(
            std::fs::read(page_root.path().join(".wenlan/state.json")).unwrap(),
            raw_state
        );
        assert_eq!(
            std::fs::read(page_root.path().join("target.md")).unwrap(),
            replacement
        );
        assert_eq!(
            std::fs::read(page_root.path().join(".wenlan/orphaned/page_reappears.md")).unwrap(),
            approved
        );
        let stage = page_root
            .path()
            .join(".wenlan")
            .join(projection_unlink_stage_name("manifest_source_reappears"));
        assert_eq!(std::fs::read(stage.join("source")).unwrap(), approved);
        assert!(stage.join(PROJECTION_STAGE_OWNER_FILE).is_file());
    }

    #[tokio::test]
    async fn stale_projection_foreign_quarantine_replacement_is_never_deleted() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let page_root = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(page_root.path().join(".wenlan")).unwrap();
        let raw_state = br#"{"schema_version":2,"pages":{"page_quarantine_race":{"file":"target.md","version":1}}}"#;
        let approved = b"---\norigin_id: page_quarantine_race\norigin_version: 1\n---\napproved\n";
        let foreign = b"foreign quarantine replacement";
        std::fs::write(page_root.path().join(".wenlan/state.json"), raw_state).unwrap();
        std::fs::write(page_root.path().join("target.md"), approved).unwrap();
        let rollback = crate::repair::capture_stale_page_projection_current(
            page_root.path(),
            "page_quarantine_race",
            "target.md",
            ".wenlan/orphaned/page_quarantine_race.md",
        )
        .unwrap();

        let error = KnowledgeProjectionWrite::with_repair_lock(
            page_root.path().to_path_buf(),
            &db,
            |repair| {
                let mut pinned = repair.pin_stale_page_projection(
                    "page_quarantine_race",
                    "target.md",
                    ".wenlan/orphaned/page_quarantine_race.md",
                    &rollback,
                    "manifest_quarantine_race",
                )?;
                let quarantine = page_root
                    .path()
                    .join(".wenlan/orphaned/page_quarantine_race.md");
                pinned.quarantine_with_after_link(|| {
                    let replacement = page_root.path().join("foreign-quarantine.tmp");
                    std::fs::write(&replacement, foreign)?;
                    std::fs::rename(replacement, quarantine)?;
                    Ok(())
                })
            },
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Conflict: repair_apply_recovery_required"
        );
        assert_eq!(
            std::fs::read(page_root.path().join(".wenlan/state.json")).unwrap(),
            raw_state
        );
        assert_eq!(
            std::fs::read(page_root.path().join("target.md")).unwrap(),
            approved
        );
        assert_eq!(
            std::fs::read(
                page_root
                    .path()
                    .join(".wenlan/orphaned/page_quarantine_race.md")
            )
            .unwrap(),
            foreign
        );
    }

    #[tokio::test]
    async fn stale_projection_state_replacement_between_compare_and_swap_is_preserved() {
        let (db, _dir) = crate::db::tests::test_db().await;
        let page_root = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(page_root.path().join(".wenlan")).unwrap();
        let raw_state =
            br#"{"schema_version":2,"pages":{"page_state_race":{"file":"target.md","version":1}}}"#;
        let foreign_state =
            br#"{"schema_version":2,"pages":{"foreign":{"file":"foreign.md","version":9}}}"#;
        let approved = b"---\norigin_id: page_state_race\norigin_version: 1\n---\napproved\n";
        std::fs::write(page_root.path().join(".wenlan/state.json"), raw_state).unwrap();
        std::fs::write(page_root.path().join("target.md"), approved).unwrap();
        let rollback = crate::repair::capture_stale_page_projection_current(
            page_root.path(),
            "page_state_race",
            "target.md",
            ".wenlan/orphaned/page_state_race.md",
        )
        .unwrap();

        let error = KnowledgeProjectionWrite::with_repair_lock(
            page_root.path().to_path_buf(),
            &db,
            |repair| {
                let mut pinned = repair.pin_stale_page_projection(
                    "page_state_race",
                    "target.md",
                    ".wenlan/orphaned/page_state_race.md",
                    &rollback,
                    "manifest_state_race",
                )?;
                let state = page_root.path().join(".wenlan/state.json");
                pinned.quarantine_with_before_state_swap(|| {
                    let replacement = page_root.path().join(".wenlan/foreign-state.tmp");
                    std::fs::write(&replacement, foreign_state)?;
                    std::fs::rename(replacement, state)?;
                    Ok(())
                })
            },
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Conflict: repair_apply_recovery_required"
        );
        assert_eq!(
            std::fs::read(page_root.path().join(".wenlan/state.json")).unwrap(),
            foreign_state
        );
        assert!(!page_root.path().join("target.md").exists());
        assert_eq!(
            std::fs::read(page_root.path().join(".wenlan/orphaned/page_state_race.md")).unwrap(),
            approved
        );
        let stage = page_root
            .path()
            .join(".wenlan")
            .join(projection_unlink_stage_name("manifest_state_race"));
        assert_eq!(std::fs::read(stage.join("source")).unwrap(), approved);
        assert_eq!(std::fs::read(stage.join("state")).unwrap(), foreign_state);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pinned_quarantine_and_rollback_ignore_ancestor_swap_canary() {
        use std::os::unix::fs::symlink;

        let (db, _dir) = crate::db::tests::test_db().await;
        let parent = tempfile::TempDir::new().unwrap();
        let page_root = parent.path().join("page-root");
        let pinned_root = parent.path().join("pinned-root");
        let external = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(page_root.join(".wenlan")).unwrap();
        let raw_state =
            br#"{"schema_version":2,"pages":{"page_stale":{"file":"target.md","version":1}}}"#;
        let source_bytes = b"---\norigin_id: page_stale\n---\noriginal\n";
        std::fs::write(page_root.join(".wenlan/state.json"), raw_state).unwrap();
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(
                page_root.join(".wenlan/state.json"),
                std::fs::Permissions::from_mode(0o640),
            )
            .unwrap();
        }
        std::fs::write(page_root.join("target.md"), source_bytes).unwrap();
        std::fs::write(external.path().join("canary"), b"external").unwrap();
        std::fs::create_dir_all(external.path().join(".wenlan/orphaned")).unwrap();
        std::fs::write(
            external.path().join(".wenlan/state.json"),
            b"external state",
        )
        .unwrap();
        std::fs::write(external.path().join("target.md"), b"external source").unwrap();
        std::fs::write(
            external.path().join(".wenlan/orphaned/page_stale.md"),
            b"external quarantine",
        )
        .unwrap();

        let before = crate::repair::capture_stale_page_projection_current(
            &page_root,
            "page_stale",
            "target.md",
            ".wenlan/orphaned/page_stale.md",
        )
        .unwrap();
        KnowledgeProjectionWrite::with_repair_lock(page_root.clone(), &db, |repair| {
            let mut pinned = repair.pin_stale_page_projection(
                "page_stale",
                "target.md",
                ".wenlan/orphaned/page_stale.md",
                &before,
                "page_stale",
            )?;
            std::fs::rename(&page_root, &pinned_root)?;
            symlink(external.path(), &page_root)?;

            pinned.quarantine()?;
            {
                use std::os::unix::fs::PermissionsExt as _;
                assert_eq!(
                    std::fs::metadata(pinned_root.join(".wenlan/state.json"))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o640,
                );
            }
            pinned.restore_snapshot(&before)?;
            Ok(())
        })
        .unwrap();

        assert_eq!(
            std::fs::read(external.path().join("canary")).unwrap(),
            b"external"
        );
        assert_eq!(
            std::fs::read(external.path().join(".wenlan/state.json")).unwrap(),
            b"external state"
        );
        assert_eq!(
            std::fs::read(external.path().join("target.md")).unwrap(),
            b"external source"
        );
        assert_eq!(
            std::fs::read(external.path().join(".wenlan/orphaned/page_stale.md")).unwrap(),
            b"external quarantine"
        );
        assert_eq!(
            std::fs::read(pinned_root.join(".wenlan/state.json")).unwrap(),
            raw_state
        );
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(pinned_root.join(".wenlan/state.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o640,
            );
        }
        assert_eq!(
            std::fs::read(pinned_root.join("target.md")).unwrap(),
            source_bytes
        );
        assert!(pinned_root.join(".wenlan/orphaned").is_dir());
        assert!(std::fs::read_dir(pinned_root.join(".wenlan/orphaned"))
            .unwrap()
            .next()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn projection_locked_capture_and_scan_ignore_ancestor_swap_canary() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::TempDir::new().unwrap();
        let page_root = parent.path().join("page-root");
        let pinned_root = parent.path().join("pinned-root");
        let external = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(page_root.join(".wenlan")).unwrap();
        let raw_state =
            br#"{"schema_version":2,"pages":{"page_stale":{"file":"target.md","version":1}}}"#;
        let source_bytes = b"---\norigin_id: page_stale\n---\noriginal\n";
        std::fs::write(page_root.join(".wenlan/state.json"), raw_state).unwrap();
        std::fs::write(page_root.join("target.md"), source_bytes).unwrap();
        std::fs::write(page_root.join("pinned-canary.md"), b"pinned").unwrap();
        std::fs::create_dir(page_root.join(".wenlan/orphaned")).unwrap();
        std::fs::write(
            page_root.join(".wenlan/orphaned/existing.md"),
            b"pinned orphan",
        )
        .unwrap();

        std::fs::create_dir_all(external.path().join(".wenlan")).unwrap();
        std::fs::write(
            external.path().join(".wenlan/state.json"),
            br#"{"schema_version":2,"pages":{"page_external":{"file":"external.md","version":1}}}"#,
        )
        .unwrap();
        std::fs::write(external.path().join("target.md"), b"external source").unwrap();
        std::fs::write(external.path().join("external-canary.md"), b"external").unwrap();
        std::fs::create_dir(external.path().join(".wenlan/orphaned")).unwrap();
        std::fs::write(
            external.path().join(".wenlan/orphaned/external.md"),
            b"external orphan",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(
                page_root.join(".wenlan/orphaned"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            std::fs::set_permissions(
                external.path().join(".wenlan/orphaned"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }

        KnowledgeProjectionWrite::with_projection_lock(&page_root, |projection| {
            std::fs::rename(&page_root, &pinned_root)?;
            symlink(external.path(), &page_root)?;

            let scan = projection.scan_page_root_controlled(
                true,
                &crate::lint::pages::fs::PageScanControl::with_timeout(
                    std::time::Duration::from_secs(30),
                ),
            )?;
            assert!(scan.entry("pinned-canary.md").is_some());
            assert!(scan.entry("external-canary.md").is_none());

            let captured = projection.capture_stale_page_projection_current(
                "page_stale",
                "target.md",
                ".wenlan/orphaned/page_stale.md",
            )?;
            assert_eq!(
                captured
                    .rows
                    .iter()
                    .find(|row| row[0] == ".wenlan/state.json")
                    .unwrap()[2],
                hex::encode(raw_state)
            );
            assert_eq!(
                captured
                    .rows
                    .iter()
                    .find(|row| row[0] == "target.md")
                    .unwrap()[2],
                hex::encode(source_bytes)
            );
            let orphaned_row = captured
                .rows
                .iter()
                .find(|row| row[0] == ".wenlan/orphaned")
                .unwrap();
            let baseline: Vec<(String, String)> =
                serde_json::from_slice(&hex::decode(&orphaned_row[2]).unwrap()).unwrap();
            assert_eq!(
                baseline,
                vec![(
                    "existing.md".to_string(),
                    hex::encode(sha2::Sha256::digest(b"pinned orphan"))
                )]
            );
            Ok(())
        })
        .unwrap();

        assert_eq!(
            std::fs::read(external.path().join("target.md")).unwrap(),
            b"external source"
        );
    }

    #[test]
    fn projection_capability_read_rejects_oversized_file_at_bound() {
        const CAPTURE_LIMIT: u64 = 16 * 1024 * 1024;

        let page_root = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(page_root.path().join(".wenlan")).unwrap();
        std::fs::write(
            page_root.path().join(".wenlan/state.json"),
            br#"{"schema_version":2,"pages":{}}"#,
        )
        .unwrap();
        let oversized = std::fs::File::create(page_root.path().join("oversized.md")).unwrap();
        oversized.set_len(CAPTURE_LIMIT + 1).unwrap();

        let error =
            KnowledgeProjectionWrite::with_projection_lock(page_root.path(), |projection| {
                projection
                    .read_relative_regular_nofollow("oversized.md", CAPTURE_LIMIT)
                    .map(|_| ())
            })
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("repair_projection_rollback_too_large"),
            "{error}"
        );
    }

    #[test]
    fn projection_capability_read_rejects_file_growth_after_metadata() {
        const CAPTURE_LIMIT: u64 = 1024;

        let page_root = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(page_root.path().join(".wenlan")).unwrap();
        std::fs::write(
            page_root.path().join(".wenlan/state.json"),
            br#"{"schema_version":2,"pages":{}}"#,
        )
        .unwrap();
        let growing_path = page_root.path().join("growing.md");
        let growing = std::fs::File::create(&growing_path).unwrap();
        growing.set_len(CAPTURE_LIMIT).unwrap();

        let error =
            KnowledgeProjectionWrite::with_projection_lock(page_root.path(), |projection| {
                projection
                    .read_relative_regular_nofollow_with_after_metadata(
                        "growing.md",
                        CAPTURE_LIMIT,
                        || {
                            std::fs::OpenOptions::new()
                                .write(true)
                                .open(&growing_path)?
                                .set_len(CAPTURE_LIMIT + 1)?;
                            Ok(())
                        },
                    )
                    .map(|_| ())
            })
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("repair_projection_rollback_too_large"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn projection_capture_rejects_aggregate_orphan_baseline_over_budget() {
        use std::os::unix::fs::PermissionsExt as _;

        let page_root = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(page_root.path().join(".wenlan/orphaned")).unwrap();
        std::fs::set_permissions(
            page_root.path().join(".wenlan/orphaned"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        std::fs::write(
            page_root.path().join(".wenlan/state.json"),
            br#"{"schema_version":2,"pages":{"page_stale_many_orphans":{"file":"source.md","version":1}}}"#,
        )
        .unwrap();
        std::fs::write(
            page_root.path().join("source.md"),
            b"---\norigin_id: page_stale_many_orphans\norigin_version: 1\n---\nbody\n",
        )
        .unwrap();
        for index in 0..17 {
            std::fs::File::create(
                page_root
                    .path()
                    .join(format!(".wenlan/orphaned/{index:02}.md")),
            )
            .unwrap()
            .set_len(1024 * 1024)
            .unwrap();
        }

        let error =
            KnowledgeProjectionWrite::with_projection_lock(page_root.path(), |projection| {
                projection.capture_stale_page_projection_current(
                    "page_stale_many_orphans",
                    "source.md",
                    ".wenlan/orphaned/page_stale_many_orphans.md",
                )
            })
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("repair_projection_rollback_too_large"),
            "{error}"
        );
        assert!(page_root.path().join("source.md").is_file());
        assert!(!page_root
            .path()
            .join(".wenlan/orphaned/page_stale_many_orphans.md")
            .exists());
    }

    #[cfg(unix)]
    #[test]
    fn projection_orphan_baseline_charges_many_empty_file_entries() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::TempDir::new().unwrap();
        let orphaned_path = root.path().join("orphaned");
        std::fs::create_dir(&orphaned_path).unwrap();
        std::fs::set_permissions(&orphaned_path, std::fs::Permissions::from_mode(0o700)).unwrap();
        for index in 0..65 {
            std::fs::File::create(orphaned_path.join(format!("{index:02}.md"))).unwrap();
        }
        let orphaned = Dir::open_ambient_dir(&orphaned_path, cap_std::ambient_authority()).unwrap();
        let mut budget = RepairReadBudget {
            remaining: 64 * ORPHAN_BASELINE_ENTRY_BUDGET_BYTES,
        };

        let error = orphaned_baseline_nofollow(&orphaned, None, &mut budget).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("repair_projection_rollback_too_large"),
            "{error}"
        );
    }

    #[test]
    fn first_page_write_creates_missing_projection_root_nofollow() {
        let parent = tempfile::TempDir::new().unwrap();
        let page_root = parent.path().join("new-page-root");
        let writer = KnowledgeWriter::new_for_test(page_root.clone());

        let path = writer.write_page_for_test(&test_concept()).unwrap();

        assert!(page_root.is_dir());
        assert!(page_root.join(".wenlan").is_dir());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            render_markdown(&test_concept())
        );
    }

    #[cfg(unix)]
    #[test]
    fn first_page_write_root_swap_uses_pinned_capability() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::TempDir::new().unwrap();
        let page_root = parent.path().join("page-root");
        let pinned_root = parent.path().join("pinned-root");
        let external = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(external.path().join(".wenlan")).unwrap();
        std::fs::write(external.path().join("canary"), b"external").unwrap();
        let writer = KnowledgeWriter::new_for_test(page_root.clone());

        writer
            .write_page_after_open_for_test(&test_concept(), || {
                std::fs::rename(&page_root, &pinned_root)?;
                symlink(external.path(), &page_root)?;
                Ok(())
            })
            .unwrap();

        assert_eq!(
            std::fs::read(external.path().join("canary")).unwrap(),
            b"external"
        );
        assert!(!external.path().join("rust-ownership.md").exists());
        assert!(!external.path().join("_sources").exists());
        assert_eq!(
            std::fs::read_to_string(pinned_root.join("rust-ownership.md")).unwrap(),
            render_markdown(&test_concept())
        );
        assert!(pinned_root.join(".wenlan/state.json").is_file());
        assert!(pinned_root.join("_sources/m1.md").is_file());
        assert!(pinned_root.join("_sources/.manifest.json").is_file());
    }

    #[test]
    fn test_write_page_creates_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let page = test_concept();

        let path = writer.write_page_for_test(&page).unwrap();
        assert!(path.ends_with("rust-ownership.md"));

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("---\n"));
        assert!(content.contains("title: \"Rust Ownership\""));
        assert!(content.contains("space: \"rust\""));
        assert!(content.contains("origin_id: concept_test123"));
        assert!(content.contains("origin_version: 2"));
        // Wikilinks converted
        assert!(content.contains("[[Borrowing]]"));
        assert!(!content.contains("(concept_borrow1)"));
    }

    #[test]
    fn test_write_updates_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());

        writer.write_page_for_test(&test_concept()).unwrap();

        let state = writer.load_state();
        assert_eq!(state.schema_version, KNOWLEDGE_STATE_SCHEMA_V2);
        assert!(state.pages.contains_key("concept_test123"));
        assert_eq!(state.pages["concept_test123"].file, "rust-ownership.md");
        assert_eq!(state.pages["concept_test123"].version, 2);
    }

    #[test]
    fn test_remove_page_deletes_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());

        let path = writer.write_page_for_test(&test_concept()).unwrap();
        assert!(std::path::Path::new(&path).exists());

        writer.remove_page_for_test("concept_test123").unwrap();
        assert!(!std::path::Path::new(&path).exists());

        let state = writer.load_state();
        assert!(!state.pages.contains_key("concept_test123"));
    }

    #[test]
    fn test_remove_nonexistent_page_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        writer.remove_page_for_test("nonexistent").unwrap();
    }

    #[test]
    fn test_write_multiple_pages() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());

        let c1 = Page {
            id: "concept_a".to_string(),
            title: "Alpha".to_string(),
            ..test_concept()
        };
        let c2 = Page {
            id: "concept_b".to_string(),
            title: "Beta".to_string(),
            ..test_concept()
        };

        writer.write_page_for_test(&c1).unwrap();
        writer.write_page_for_test(&c2).unwrap();

        assert!(dir.path().join("alpha.md").exists());
        assert!(dir.path().join("beta.md").exists());

        let state = writer.load_state();
        assert_eq!(state.pages.len(), 2);
    }

    #[test]
    fn test_knowledge_writer_overwrite_on_version_change() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());

        let mut page = test_concept();
        writer.write_page_for_test(&page).unwrap();

        let v1 = std::fs::read_to_string(dir.path().join("rust-ownership.md")).unwrap();
        assert!(v1.contains("origin_version: 2"));

        // Update version and content
        page.version = 3;
        page.content = "## Updated\nNew content.".to_string();
        writer.write_page_for_test(&page).unwrap();

        let v2 = std::fs::read_to_string(dir.path().join("rust-ownership.md")).unwrap();
        assert!(v2.contains("origin_version: 3"));
        assert!(v2.contains("## Updated"));
        assert!(!v2.contains("memory safety")); // old content replaced

        // State reflects new version
        let state = writer.load_state();
        assert_eq!(state.pages["concept_test123"].version, 3);
    }

    /// `write_page` must replace the target file via temp-file-then-rename,
    /// never via in-place truncate+write, so a concurrent reader can never
    /// observe a half-written page file. A plain `fs::write`
    /// truncates and rewrites the *same* inode, so its inode number is
    /// unchanged across writes; an atomic rename swaps in a new inode. That
    /// difference is deterministic (no thread timing needed) and is what
    /// actually distinguishes this from the old, non-atomic implementation —
    /// unlike the leftover-temp-file check below, which passes trivially on
    /// old code too (it never created temp files to begin with).
    #[cfg(unix)]
    #[test]
    fn write_page_replaces_file_via_atomic_rename_not_in_place_write() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());

        let mut page = test_concept();
        let path = writer.write_page_for_test(&page).unwrap();
        let ino_before = std::fs::metadata(&path).unwrap().ino();

        page.version = 3;
        page.content = "## Updated\nNew content for atomicity check.".to_string();
        writer.write_page_for_test(&page).unwrap();

        let ino_after = std::fs::metadata(&path).unwrap().ino();
        assert_ne!(
            ino_before, ino_after,
            "rewrite kept the same inode — target was truncated in place, not atomically renamed"
        );

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, render_markdown_for(&page));

        let leftover_temp_files: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftover_temp_files.is_empty(),
            "leftover temp file(s): {leftover_temp_files:?}"
        );
    }

    /// A projection file that vanished (rename never landed, user deleted it)
    /// is rewritten from DB truth.
    #[test]
    fn reconcile_rewrites_page_whose_projection_file_vanished() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let page = test_concept();
        let path = writer.write_page_for_test(&page).unwrap();
        std::fs::remove_file(&path).unwrap();

        let stats = writer
            .reconcile_for_test(std::slice::from_ref(&page))
            .unwrap();

        assert_eq!(stats.checked, 1);
        assert_eq!(stats.rewritten, 1);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            render_markdown_for(&page),
            "vanished projection was not restored from the DB"
        );
    }

    /// The fsync-free write buys atomicity, not durability: after a power loss
    /// the rename may be undone, leaving the *previous* version's bytes under
    /// the target name. The `origin_version` stamp is what proves it — the DB
    /// moved on, the file did not.
    #[test]
    fn reconcile_rewrites_projection_left_behind_by_a_torn_rename() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());

        let mut page = test_concept();
        let path = writer.write_page_for_test(&page).unwrap();

        // DB advanced to v3; the file still holds the v2 projection.
        page.version = 3;
        page.content = "## Updated\nv3 body that never reached disk.".to_string();

        let stats = writer
            .reconcile_for_test(std::slice::from_ref(&page))
            .unwrap();

        assert_eq!(stats.rewritten, 1);
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            on_disk,
            render_markdown_for(&page),
            "stale projection was not brought forward to the DB version"
        );
    }

    /// The other power-loss shape: the rename is durable but the bytes are
    /// not, so the target is zero-length. No frontmatter parses, so the
    /// version stamp reads 0 — behind any real page (pages start at 1).
    #[test]
    fn reconcile_rewrites_projection_truncated_to_zero_bytes() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let page = test_concept();
        let path = writer.write_page_for_test(&page).unwrap();
        std::fs::write(&path, "").unwrap();

        let stats = writer
            .reconcile_for_test(std::slice::from_ref(&page))
            .unwrap();

        assert_eq!(stats.rewritten, 1);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            render_markdown_for(&page)
        );
    }

    /// The safety boundary. md is canonical for prose (see `page_watcher`), so
    /// a body that differs from `render_markdown` while the version stamp is
    /// current is a USER EDIT made while the daemon was down — the watcher
    /// owns it, and reconcile must not clobber it on the way past. The temp
    /// sweep is likewise scoped to `write_page`'s own name shape, never to a
    /// file the user happens to have parked in the pages directory.
    #[test]
    fn reconcile_preserves_offline_user_edits_and_sweeps_only_our_temp_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let page = test_concept();
        let path = writer.write_page_for_test(&page).unwrap();

        let edited = std::fs::read_to_string(&path)
            .unwrap()
            .replace("Rust uses ownership", "Hand-written prose the user typed");
        std::fs::write(&path, &edited).unwrap();

        let ours = dir.path().join(format!(".{}.4242.7.tmp", page.id));
        let user_scratch = dir.path().join("draft-notes.tmp");
        let editor_swap = dir.path().join(".obsidian-swap.tmp");
        for f in [&ours, &user_scratch, &editor_swap] {
            std::fs::write(f, "scratch").unwrap();
        }

        let stats = writer
            .reconcile_for_test(std::slice::from_ref(&page))
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            edited,
            "reconcile clobbered an offline user edit"
        );
        assert_eq!(stats.rewritten, 0);
        assert_eq!(stats.temp_files_removed, 1);
        assert!(!ours.exists(), "our own leftover temp file survived");
        assert!(
            user_scratch.exists() && editor_swap.exists(),
            "reconcile deleted a temp file that isn't ours"
        );
    }

    /// Review finding 5: the 562 pages projected before the OKF frontmatter
    /// existed have no `type` key and must gain one on the first reconcile
    /// after upgrade, without a version bump -- as long as the body on disk
    /// still matches what we'd render today (no offline edit sitting on it).
    #[test]
    fn reconcile_upgrades_legacy_frontmatter_to_okf_when_body_is_unchanged() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let page = test_concept();
        let path = writer.write_page_for_test(&page).unwrap();

        // Same body a current write would produce, but pre-OKF frontmatter:
        // no `type` key, same `origin_version` the DB still has.
        let current = std::fs::read_to_string(&path).unwrap();
        let (_, body) = crate::sources::obsidian::extract_frontmatter(&current);
        let legacy = format!(
            "---\ntitle: \"{}\"\norigin_version: {}\n---\n\n{}",
            page.title, page.version, body
        );
        std::fs::write(&path, &legacy).unwrap();

        let stats = writer
            .reconcile_for_test(std::slice::from_ref(&page))
            .unwrap();

        assert_eq!(
            stats.rewritten, 1,
            "legacy frontmatter with an unchanged body must be upgraded to OKF"
        );
        let (fm, _) =
            crate::sources::obsidian::extract_frontmatter(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(fm.get_str("type"), Some("page"));
    }

    /// Review finding 5, the safety half: a legacy file with no `type` key
    /// but a body that no longer matches `render_markdown` is an offline edit
    /// sitting on top of pre-OKF frontmatter. Reconcile must leave it alone
    /// so the watcher gets first crack at it, exactly like the current-format
    /// offline-edit case above.
    #[test]
    fn reconcile_does_not_upgrade_legacy_frontmatter_over_an_offline_edit() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let page = test_concept();
        let path = writer.write_page_for_test(&page).unwrap();

        let current = std::fs::read_to_string(&path).unwrap();
        let (_, body) = crate::sources::obsidian::extract_frontmatter(&current);
        let edited_body = body.replace("Rust uses ownership", "Hand-written prose the user typed");
        let legacy = format!(
            "---\ntitle: \"{}\"\norigin_version: {}\n---\n\n{}",
            page.title, page.version, edited_body
        );
        std::fs::write(&path, &legacy).unwrap();

        let stats = writer
            .reconcile_for_test(std::slice::from_ref(&page))
            .unwrap();

        assert_eq!(
            stats.rewritten, 0,
            "reconcile must not clobber an offline edit made under legacy frontmatter"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), legacy);
    }

    /// Without a `state.json` entry we cannot tell which file on disk is a
    /// page's projection, and guessing forks a `<slug>-2.md` duplicate against
    /// the real one (the hazard `page_watcher` calls out for vaults synced
    /// without `.wenlan/`). Reconcile skips those rather than fork.
    #[test]
    fn reconcile_skips_pages_with_no_state_entry_rather_than_forking_a_duplicate() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let page = test_concept();
        // A projection exists on disk, but state.json does not know about it.
        std::fs::write(
            dir.path().join("rust-ownership.md"),
            render_markdown_for(&page),
        )
        .unwrap();

        let stats = writer
            .reconcile_for_test(std::slice::from_ref(&page))
            .unwrap();

        assert_eq!(stats.checked, 0);
        assert_eq!(stats.rewritten, 0);
        assert!(
            !dir.path().join("rust-ownership-2.md").exists(),
            "reconcile forked a duplicate projection"
        );
    }

    /// Q1 (stage c fix wave): the daemon startup backfill (`main.rs`, guarded
    /// by `startup_projection_writes_allowed`) fetches its page set via
    /// `db.list_pages("active", 10_000, 0)` and then unconditionally calls
    /// `KnowledgeProjectionWrite::write_page` for every page returned —
    /// `write_page` itself has no kind filter, so the guarantee that a
    /// `kind='entity'` dual-write shadow never lands on disk lives entirely
    /// in `list_pages` staying fenced. This mirrors that exact call sequence
    /// (fetch, then write-loop) rather than calling `write_page` directly, so
    /// a regression in the fence — not in this file — would fail it too.
    #[tokio::test]
    async fn knowledge_projection_backfill_excludes_entity_kind_shadow() {
        let (db, _dir) = crate::db::tests::test_db().await;
        db.insert_page_with_kind(
            "p_q1_backfill_concept",
            "Q1 Backfill Concept Page",
            None,
            "concept content",
            None,
            None,
            &[],
            "2000-01-01T00:00:00+00:00",
            "authored",
            "confirmed",
            None,
            None,
        )
        .await
        .unwrap();
        db.store_entity("Q1 Backfill Entity Marker", "person", None, None, None)
            .await
            .unwrap();

        let pages = db.list_pages("active", 10_000, 0).await.unwrap();
        let vault = tempfile::TempDir::new().unwrap();
        let projection = KnowledgeProjectionWrite::new(vault.path().to_path_buf(), &db);
        for page in &pages {
            projection.write_page(page).unwrap();
        }

        let written: Vec<_> = std::fs::read_dir(vault.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
            .filter(|e| {
                !e.file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(INDEX_FILE)
            })
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            written.len(),
            1,
            "startup backfill must write only the real page, not the entity shadow; got: {written:?}"
        );
        let content = std::fs::read_to_string(vault.path().join(&written[0])).unwrap();
        assert!(
            !content.contains("Q1 Backfill Entity Marker"),
            "the shadow page's title must never reach a projected markdown file"
        );
    }

    #[test]
    fn test_load_state_migrates_v1_concept_keys_to_page() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());

        // Write a legacy v1 state.json by hand.
        let v1_json = r#"{
            "concepts": {
                "concept_aaa": { "file": "a.md", "version": 1, "last_written": "2026-04-01T00:00:00+00:00" },
                "concept_bbb": { "file": "b.md", "version": 2, "last_written": "2026-04-02T00:00:00+00:00" }
            }
        }"#;
        std::fs::create_dir_all(dir.path().join(".wenlan")).unwrap();
        std::fs::write(dir.path().join(".wenlan/state.json"), v1_json).unwrap();

        let state = writer.load_state();
        // v1 keys are auto-rewritten to page_ prefix.
        assert!(state.pages.contains_key("page_aaa"));
        assert!(state.pages.contains_key("page_bbb"));
        assert!(!state.pages.contains_key("concept_aaa"));
        assert_eq!(state.pages["page_aaa"].file, "a.md");
        assert_eq!(state.pages["page_bbb"].version, 2);
        assert_eq!(state.schema_version, KNOWLEDGE_STATE_SCHEMA_V2);

        // After save_state, the file is rewritten in v2 form (no "concepts" key).
        writer.save_state(&state).unwrap();
        let written = std::fs::read_to_string(dir.path().join(".wenlan/state.json")).unwrap();
        assert!(written.contains("\"pages\""));
        assert!(!written.contains("\"concepts\""));
        assert!(written.contains("\"schema_version\""));
    }

    #[test]
    fn test_load_state_upgrades_writer_default_v0() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        std::fs::create_dir_all(dir.path().join(".wenlan")).unwrap();
        std::fs::write(
            dir.path().join(".wenlan/state.json"),
            r#"{"schema_version":0,"pages":{}}"#,
        )
        .unwrap();

        assert_eq!(
            writer.load_state().schema_version,
            KNOWLEDGE_STATE_SCHEMA_V2
        );
    }

    #[test]
    fn test_knowledge_writer_no_domain() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());

        let page = Page {
            space: None,
            ..test_concept()
        };
        let path = writer.write_page_for_test(&page).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("space:"));
    }

    #[test]
    fn render_markdown_emits_sources_frontmatter_and_delimiter_block() {
        let mut page = test_concept();
        page.source_memory_ids = vec!["mem_1".to_string(), "mem_2".to_string()];
        let md = render_markdown(&page);
        // Read-only frontmatter property, OKF object-list shape.
        assert!(md.contains("sources:\n  - {id: \"mem_1\", resource: \"wenlan://memory/mem_1\"}"));
        assert!(md.contains("  - {id: \"mem_2\", resource: \"wenlan://memory/mem_2\"}"));
        // Delimiter-wrapped Sources block in the body.
        assert!(md.contains(crate::export::provenance::SOURCES_BLOCK_START));
        assert!(md.contains(crate::export::provenance::SOURCES_BLOCK_END));
        assert!(md.contains("## Sources"));
        // The projected body, run back through the canonicalizer, drops the
        // generated block — the round-trip invariant the watcher relies on.
        let (_, body) = crate::sources::obsidian::extract_frontmatter(&md);
        let canon = crate::export::provenance::canonicalize_page_body(body);
        assert!(!canon.contains(crate::export::provenance::SOURCES_BLOCK_START));
        assert!(!canon.contains("## Sources"));
        // Substring of test_concept()'s actual body text.
        assert!(canon.contains("Rust uses ownership"));
    }

    #[test]
    fn stubs_resolve_then_gc_when_page_removed() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        page.source_memory_ids = vec!["mem_x".to_string()];
        writer.write_page_for_test(&page).unwrap();
        let stub = dir.path().join("_sources").join("mem_x.md");
        assert!(stub.exists(), "stub projected on write");

        writer.remove_page_for_test(&page.id).unwrap();
        assert!(!stub.exists(), "orphan stub GC'd on page removal");
    }

    #[test]
    fn render_markdown_source_less_page_has_no_sources_artifacts() {
        let mut page = test_concept();
        page.source_memory_ids = vec![];
        let md = render_markdown(&page);
        assert!(!md.contains("sources:"));
        assert!(!md.contains(crate::export::provenance::SOURCES_BLOCK_START));
    }

    #[test]
    fn render_markdown_escapes_title_so_frontmatter_survives_quotes() {
        let mut page = test_concept();
        page.title = "The \"Real\" Architecture".to_string();
        let md = render_markdown(&page);
        let (fm, _body) = crate::sources::obsidian::extract_frontmatter(&md);
        // Frontmatter must NOT have collapsed to empty: origin_id still parses.
        assert_eq!(
            fm.get_str("origin_id"),
            Some(page.id.as_str()),
            "quote-bearing title collapsed the frontmatter map"
        );
        // And the title round-trips intact.
        assert_eq!(fm.get_str("title"), Some("The \"Real\" Architecture"));
    }

    // ---- OKF v0.2 frontmatter (spec 2026-09-16-okf-projection.md) ----------

    #[test]
    fn render_markdown_frontmatter_gains_type_page() {
        let md = render_markdown(&test_concept());
        let (fm, _) = crate::sources::obsidian::extract_frontmatter(&md);
        assert_eq!(fm.get_str("type"), Some("page"));
    }

    #[test]
    fn render_markdown_description_reflects_summary_when_present() {
        let md = render_markdown(&test_concept());
        let (fm, _) = crate::sources::obsidian::extract_frontmatter(&md);
        assert_eq!(fm.get_str("description"), Some("Memory safety without GC"));
    }

    #[test]
    fn render_markdown_omits_description_when_no_summary() {
        let page = Page {
            summary: None,
            ..test_concept()
        };
        let md = render_markdown(&page);
        assert!(!md.contains("description:"));
    }

    #[test]
    fn render_markdown_tags_mirror_space_when_present() {
        let md = render_markdown(&test_concept());
        let (fm, _) = crate::sources::obsidian::extract_frontmatter(&md);
        assert_eq!(fm.tags(), vec!["rust".to_string()]);
        // The pre-existing `space:` line is kept alongside the new `tags:`.
        assert_eq!(fm.get_str("space"), Some("rust"));
    }

    #[test]
    fn render_markdown_omits_tags_when_no_space() {
        let page = Page {
            space: None,
            ..test_concept()
        };
        let md = render_markdown(&page);
        assert!(!md.contains("tags:"));
    }

    /// `generated.at` answers "how recently did this content change" (OKF 5.2),
    /// so it carries `last_modified`, never `created_at`. A page distilled once
    /// and rewritten many times must not read as fresh from the day it was born.
    #[test]
    fn render_markdown_generated_stamps_version_and_last_modified() {
        let mut page = test_concept();
        page.created_at = "2026-04-01T00:00:00+00:00".to_string();
        page.last_modified = "2026-09-17T12:00:00+00:00".to_string();
        let md = render_markdown(&page);
        assert!(
            md.contains(&format!(
                "generated: {{by: \"wenlan/{}\", at: \"{}\"}}",
                crate::version(),
                page.last_modified
            )),
            "{md}"
        );
        assert!(!md.contains("at: \"2026-04-01T00:00:00+00:00\""), "{md}");
    }

    #[test]
    fn render_markdown_status_stable_by_default() {
        let md = render_markdown(&test_concept());
        let (fm, _) = crate::sources::obsidian::extract_frontmatter(&md);
        assert_eq!(fm.get_str("status"), Some("stable"));
    }

    #[test]
    fn render_markdown_status_draft_when_stale_reason_set() {
        let page = Page {
            stale_reason: Some("source_updated".to_string()),
            ..test_concept()
        };
        let md = render_markdown(&page);
        let (fm, _) = crate::sources::obsidian::extract_frontmatter(&md);
        assert_eq!(fm.get_str("status"), Some("draft"));
    }

    #[test]
    fn render_markdown_status_deprecated_when_page_not_active() {
        let page = Page {
            status: "archived".to_string(),
            ..test_concept()
        };
        let md = render_markdown(&page);
        let (fm, _) = crate::sources::obsidian::extract_frontmatter(&md);
        assert_eq!(fm.get_str("status"), Some("deprecated"));
    }

    #[test]
    fn render_markdown_never_emits_verified_without_a_confirmation_timestamp_column() {
        // `review_status == "confirmed"` (test_concept()'s default) is the ONLY
        // signal available; the `pages` table has no `confirmed_at`/`reviewed_at`
        // column (verified by grepping migrations and `Page`), so `verified:`
        // must never appear rather than fabricate `at`.
        let page = test_concept();
        assert_eq!(page.review_status, "confirmed");
        let md = render_markdown(&page);
        assert!(!md.contains("verified:"));
    }

    #[test]
    fn render_markdown_body_is_unchanged_by_the_new_frontmatter_fields() {
        // Obsidian graph edges come from the body, not frontmatter: split the
        // projection at the closing `---` and check the body is exactly the
        // wikified content plus the delimiter-wrapped Sources block, same as
        // before this change.
        let page = test_concept();
        let md = render_markdown(&page);
        let (_, body) = crate::sources::obsidian::extract_frontmatter(&md);
        let expected = format!(
            "{}\n\n{}",
            convert_links_to_wikilinks(&page.content),
            crate::export::provenance::render_sources_block(&page.source_memory_ids)
        );
        assert_eq!(body, expected);
    }

    // ---- OKF v0.2 index.md ---------------------------------------------------

    #[test]
    fn write_page_regenerates_index_with_only_okf_version_frontmatter() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());

        let mut page_a = test_concept();
        page_a.id = "page_a".to_string();
        page_a.title = "Alpha".to_string();
        page_a.space = Some("rust".to_string());
        let path_a = writer.write_page_for_test(&page_a).unwrap();
        let filename_a = Path::new(&path_a)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();

        let mut page_b = test_concept();
        page_b.id = "page_b".to_string();
        page_b.title = "Beta".to_string();
        page_b.space = None;
        writer.write_page_for_test(&page_b).unwrap();

        let index_path = dir.path().join(INDEX_FILE);
        assert!(index_path.exists(), "index.md must be projected");
        let content = std::fs::read_to_string(&index_path).unwrap();
        let (fm, body) = crate::sources::obsidian::extract_frontmatter(&content);
        assert_eq!(
            fm.fields.len(),
            1,
            "index.md's only frontmatter key must be okf_version, got: {:?}",
            fm.fields
        );
        assert_eq!(fm.get_str("okf_version"), Some("0.2"));

        assert!(body.contains("## rust"));
        assert!(body.contains(&format!("[Alpha](/{filename_a})")));
        assert!(body.contains("## Unfiled"));
        assert!(body.contains("[Beta]("));
    }

    /// Review finding 8: a title is free text -- a newline would split it
    /// across index.md lines, and an unescaped `]` would close the markdown
    /// link early. Both must be neutralized in the generated bullet.
    #[test]
    fn regenerate_index_sanitizes_a_title_with_a_newline_and_a_bracket() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());

        let mut page = test_concept();
        page.title = "Weird]\nTitle".to_string();
        writer.write_page_for_test(&page).unwrap();

        let content = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();
        let (_, body) = crate::sources::obsidian::extract_frontmatter(&content);
        assert_eq!(
            body.lines().filter(|l| l.starts_with("* [")).count(),
            1,
            "the title's embedded newline must not add a bullet line; got: {body}"
        );
        assert!(
            body.contains("[Weird\\] Title]("),
            "the title's `]` must be escaped and its newline collapsed to a space; got: {body}"
        );
    }

    /// A page with no summary must not render `* [Title](/file.md) - `: the
    /// separator would point at nothing, in every projection index on disk.
    #[test]
    fn index_line_for_a_page_without_a_summary_has_no_trailing_separator() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());

        let mut described = test_concept();
        described.id = "page_described".to_string();
        described.title = "Described".to_string();
        described.summary = Some("A real summary".to_string());
        writer.write_page_for_test(&described).unwrap();

        let mut bare = test_concept();
        bare.id = "page_bare".to_string();
        bare.title = "Bare".to_string();
        bare.summary = None;
        writer.write_page_for_test(&bare).unwrap();

        // A summary of only whitespace is the same case: `sanitize_index_field`
        // collapses it to nothing before the line is built.
        let mut blank = test_concept();
        blank.id = "page_blank".to_string();
        blank.title = "Blank".to_string();
        blank.summary = Some("   \n  ".to_string());
        writer.write_page_for_test(&blank).unwrap();

        let content = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();
        let line = |title: &str| -> String {
            content
                .lines()
                .find(|l| l.starts_with(&format!("* [{title}](")))
                .unwrap_or_else(|| panic!("no index line for {title}; got: {content}"))
                .to_string()
        };
        assert!(line("Described").ends_with(" - A real summary"));
        assert!(
            line("Bare").ends_with(".md)"),
            "a page with no summary must end at the link; got: {}",
            line("Bare")
        );
        assert!(
            line("Blank").ends_with(".md)"),
            "a whitespace-only summary must end at the link; got: {}",
            line("Blank")
        );
        assert!(!content.contains(" - \n"), "{content}");
    }

    #[test]
    fn remove_page_regenerates_index_dropping_the_removed_page() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let page = test_concept();
        writer.write_page_for_test(&page).unwrap();
        assert!(std::fs::read_to_string(dir.path().join(INDEX_FILE))
            .unwrap()
            .contains(&page.title));

        writer.remove_page_for_test(&page.id).unwrap();
        let content = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();
        assert!(!content.contains(&page.title));
    }

    #[test]
    fn reconcile_regenerates_index_even_when_no_page_is_rewritten() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let page = test_concept();
        writer.write_page_for_test(&page).unwrap();
        // Simulate a projection directory reconciled for the first time after
        // this change ships: every page file is current, but index.md predates
        // it and is missing.
        std::fs::remove_file(dir.path().join(INDEX_FILE)).unwrap();
        assert!(!dir.path().join(INDEX_FILE).exists());

        let stats = writer.reconcile_for_test(&[page]).unwrap();
        assert_eq!(
            stats.rewritten, 0,
            "the page file is current; nothing should be rewritten"
        );
        assert!(
            dir.path().join(INDEX_FILE).exists(),
            "reconcile must regenerate index.md unconditionally, not only on rewrite"
        );
    }

    /// Integrated review F1: a pages folder opened in Obsidian often has a
    /// home note named `index.md`. Page writes, reconcile and removal must
    /// leave it byte for byte, including one with its own frontmatter.
    #[test]
    fn a_user_index_note_survives_write_reconcile_and_remove() {
        for note in [
            "# Home\n\nMy own start page.\n",
            "---\ntitle: Home\nokf_version: \"0.2\"\n---\n\nMy own start page.\n",
            "",
        ] {
            let dir = tempfile::TempDir::new().unwrap();
            let index_path = dir.path().join(INDEX_FILE);
            std::fs::write(&index_path, note).unwrap();
            let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
            let page = test_concept();

            let page_path = writer.write_page_for_test(&page).unwrap();
            assert!(
                Path::new(&page_path).exists(),
                "the page is still projected"
            );
            assert_eq!(std::fs::read_to_string(&index_path).unwrap(), note);

            writer
                .reconcile_for_test(std::slice::from_ref(&page))
                .unwrap();
            assert_eq!(std::fs::read_to_string(&index_path).unwrap(), note);

            writer.remove_page_for_test(&page.id).unwrap();
            assert_eq!(std::fs::read_to_string(&index_path).unwrap(), note);
        }
    }

    /// Prose typed into Wenlan's own generated index survives the next page
    /// write. This test used to assert the opposite, which is the defect: the
    /// frontmatter check said "only `okf_version`, so it is ours", and the
    /// sentence a person typed underneath was overwritten. The index going
    /// stale is the intended trade -- it is recoverable (move the note aside),
    /// and the warning says how.
    #[test]
    fn a_hand_edit_to_wenlans_own_index_is_not_overwritten() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page_a = test_concept();
        page_a.id = "page_a".to_string();
        page_a.title = "Alpha".to_string();
        writer.write_page_for_test(&page_a).unwrap();

        let index_path = dir.path().join(INDEX_FILE);
        let mut edited = std::fs::read_to_string(&index_path).unwrap();
        edited.push_str("\nWhere I keep my reading queue.\n");
        std::fs::write(&index_path, &edited).unwrap();

        let mut page_b = test_concept();
        page_b.id = "page_b".to_string();
        page_b.title = "Beta".to_string();
        writer.write_page_for_test(&page_b).unwrap();

        assert_eq!(
            std::fs::read_to_string(&index_path).unwrap(),
            edited,
            "the index is left exactly as the user left it"
        );
    }

    /// An OKF root index somebody hand-wrote is not Wenlan's. `okf_version`
    /// alone is the STANDARD shape, not a Wenlan signature, so the frontmatter
    /// check claimed and overwrote an index from another producer.
    ///
    /// The body here is prose. An index whose body is genuinely
    /// indistinguishable from the renderer's output IS still replaced; that is
    /// the residual recorded at `index_file_is_replaceable`.
    #[test]
    fn a_hand_written_okf_index_with_prose_is_left_alone() {
        let dir = tempfile::TempDir::new().unwrap();
        let index_path = dir.path().join(INDEX_FILE);
        let theirs = "---\nokf_version: \"0.2\"\n---\n\nMy own root index, written by hand.\n";
        std::fs::write(&index_path, theirs).unwrap();

        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        writer.write_page_for_test(&test_concept()).unwrap();

        assert_eq!(std::fs::read_to_string(&index_path).unwrap(), theirs);
    }

    /// Muse: `* [` is also how a Markdown checklist starts, so a to-do list
    /// under `okf_version`-only frontmatter passed the first version of the
    /// body-shape test and was replaced.
    #[test]
    fn a_checklist_is_not_mistaken_for_index_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        let index_path = dir.path().join(INDEX_FILE);
        let theirs = "---\nokf_version: \"0.2\"\n---\n\n* [ ] buy milk\n* [x] file taxes\n";
        std::fs::write(&index_path, theirs).unwrap();

        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        writer.write_page_for_test(&test_concept()).unwrap();

        assert_eq!(std::fs::read_to_string(&index_path).unwrap(), theirs);
    }

    /// Every generated index that has a heading has an entry under it, so a
    /// file of bare headings is somebody's outline.
    #[test]
    fn a_headings_only_outline_is_not_mistaken_for_an_index() {
        let dir = tempfile::TempDir::new().unwrap();
        let index_path = dir.path().join(INDEX_FILE);
        let theirs = "---\nokf_version: \"0.2\"\n---\n\n## Projects\n\n## Someday\n";
        std::fs::write(&index_path, theirs).unwrap();

        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        writer.write_page_for_test(&test_concept()).unwrap();

        assert_eq!(std::fs::read_to_string(&index_path).unwrap(), theirs);
    }

    /// The flip side: an index this projection actually generated is still
    /// replaceable, so the index keeps updating in the ordinary case.
    #[test]
    fn an_untouched_generated_index_is_still_regenerated() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page_a = test_concept();
        page_a.id = "page_a".to_string();
        page_a.title = "Alpha".to_string();
        writer.write_page_for_test(&page_a).unwrap();

        let mut page_b = test_concept();
        page_b.id = "page_b".to_string();
        page_b.title = "Beta".to_string();
        writer.write_page_for_test(&page_b).unwrap();

        let content = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();
        assert!(content.contains("[Alpha]("), "{content}");
        assert!(content.contains("[Beta]("), "{content}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_index_is_neither_replaced_nor_followed() {
        let dir = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let target = outside.path().join("home.md");
        std::fs::write(&target, "# Elsewhere\n").unwrap();
        let index_path = dir.path().join(INDEX_FILE);
        std::os::unix::fs::symlink(&target, &index_path).unwrap();

        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        writer.write_page_for_test(&test_concept()).unwrap();

        assert!(std::fs::symlink_metadata(&index_path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "# Elsewhere\n");
    }

    fn index_frontmatter_keys(dir: &Path) -> Vec<String> {
        let content = std::fs::read_to_string(dir.join(INDEX_FILE)).unwrap();
        let (fm, _) = crate::sources::obsidian::extract_frontmatter(&content);
        let mut keys: Vec<String> = fm.fields.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// PR #763 review: `slugify` lowercases, so a page titled "Index" in an
    /// empty vault used to be projected at `index.md`, and the OKF index could
    /// then never be written there.
    #[test]
    fn a_page_titled_index_never_takes_the_index_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        page.title = "Index".to_string();

        let page_path = writer.write_page_for_test(&page).unwrap();
        assert!(page_path.ends_with("index-page.md"), "{page_path}");
        assert_eq!(index_frontmatter_keys(dir.path()), vec!["okf_version"]);
        let index = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();
        assert!(index.contains("[Index](/index-page.md)"), "{index}");
    }

    /// OKF reserves `log.md` at every level of the hierarchy for a directory's
    /// update history (spec 3.1), so a page titled "Log" may not take it, for
    /// the same reason a page titled "Index" may not take `index.md`.
    #[test]
    fn a_page_titled_log_never_takes_the_log_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        page.title = "Log".to_string();

        let page_path = writer.write_page_for_test(&page).unwrap();
        assert!(page_path.ends_with("log-page.md"), "{page_path}");
        assert!(
            !dir.path().join("log.md").exists(),
            "log.md is reserved and must stay free"
        );
        let index = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();
        assert!(index.contains("[Log](/log-page.md)"), "{index}");
    }

    /// A vault projected before `log.md` was reserved holds a page titled
    /// "Log" there. Its next write moves it to a free name and removes the
    /// copy left behind, the same migration a page at `index.md` gets.
    #[test]
    fn a_page_already_at_log_md_moves_on_its_next_write() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        page.title = "Log".to_string();
        let moved_path = writer.write_page_for_test(&page).unwrap();

        // Put the projection back where an older build would have left it.
        std::fs::rename(&moved_path, dir.path().join("log.md")).unwrap();
        let mut state = writer.load_state();
        state.pages.get_mut(&page.id).unwrap().file = "log.md".to_string();
        writer.save_state(&state).unwrap();

        page.version += 1;
        let rewritten = writer.write_page_for_test(&page).unwrap();
        assert!(rewritten.ends_with("log-page.md"), "{rewritten}");
        assert!(std::fs::read_to_string(&rewritten)
            .unwrap()
            .contains(&format!("origin_id: {}", page.id)));
        assert_eq!(
            writer.page_filename(&page.id).as_deref(),
            Some("log-page.md")
        );
        assert!(
            !dir.path().join("log.md").exists(),
            "the copy left at the reserved name must be cleared"
        );
        let index = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();
        assert!(index.contains("[Log](/log-page.md)"), "{index}");
    }

    /// The copy left at a reserved name is archived, not unlinked. Editing a
    /// page in the vault leaves its frontmatter alone, so `origin_id` cannot
    /// tell Wenlan's own bytes from bytes the person typed over them, and the
    /// database can rebuild the page while it cannot rebuild the typing.
    #[test]
    fn an_edit_left_at_a_reserved_name_is_archived_not_destroyed() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        page.title = "Log".to_string();
        let moved_path = writer.write_page_for_test(&page).unwrap();

        // An older build left the page at the reserved name, and the person
        // then added a line to it in place, keeping the frontmatter.
        let edited = format!(
            "{}\n\nA line I typed myself.\n",
            std::fs::read_to_string(&moved_path).unwrap()
        );
        std::fs::remove_file(&moved_path).unwrap();
        std::fs::write(dir.path().join("log.md"), &edited).unwrap();
        let mut state = writer.load_state();
        state.pages.get_mut(&page.id).unwrap().file = "log.md".to_string();
        writer.save_state(&state).unwrap();

        page.version += 1;
        let rewritten = writer.write_page_for_test(&page).unwrap();
        assert!(rewritten.ends_with("log-page.md"), "{rewritten}");
        assert!(
            !dir.path().join("log.md").exists(),
            "the reserved name must be free again"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("archive").join("log-page.md")).unwrap(),
            edited,
            "the typing must survive in archive/"
        );
        assert!(
            !dir.path().join("archive").join("log.md").exists(),
            "and must not take the reserved name into archive/, which OKF \
             reserves at every level too (spec 3.1)"
        );
    }

    /// Astra finding 5. The relocation used to be gated on the writer that
    /// also regenerates the index, so a repair rewrote a legacy page straight
    /// back onto `log.md` and the conformance defect never cleared. The
    /// relocation now fires for every writer, and forces the index with it so
    /// no writer leaves a link pointing at the name it just vacated.
    #[test]
    fn the_repair_writer_also_moves_a_page_off_a_reserved_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let seed = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        page.title = "Log".to_string();
        let path = seed.write_page_for_test(&page).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(
            dir.path().join("log.md"),
            format!("---\ntype: page\norigin_id: {}\n---\n\nlegacy\n", page.id),
        )
        .unwrap();
        let mut state = seed.load_state();
        state.pages.get_mut(&page.id).unwrap().file = "log.md".to_string();
        seed.save_state(&state).unwrap();

        let repair = KnowledgeWriter::new_for_test_repair(dir.path().to_path_buf());
        page.version += 1;
        let rewritten = repair.write_page_for_test(&page).unwrap();

        assert!(rewritten.ends_with("log-page.md"), "{rewritten}");
        assert!(
            !dir.path().join("log.md").exists(),
            "the reserved name must be free again after a repair too"
        );
        let index = std::fs::read_to_string(dir.path().join("index.md")).unwrap();
        assert!(
            index.contains("/log-page.md") && !index.contains("(/log.md)"),
            "the relocation forces the index even for a writer that skips it: {index}"
        );
    }

    /// A page already titled "Index Page" keeps `index-page.md`; the page
    /// titled "Index" takes the next free name, never `index.md`.
    #[test]
    fn a_page_titled_index_gets_the_next_name_when_index_page_is_taken() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut named = test_concept();
        named.id = "page_index_page".to_string();
        named.title = "Index Page".to_string();
        let named_path = writer.write_page_for_test(&named).unwrap();
        assert!(named_path.ends_with("index-page.md"), "{named_path}");

        let mut page = test_concept();
        page.title = "Index".to_string();
        let page_path = writer.write_page_for_test(&page).unwrap();
        assert!(page_path.ends_with("index-page-2.md"), "{page_path}");
        assert_eq!(index_frontmatter_keys(dir.path()), vec!["okf_version"]);
    }

    /// A vault projected before the name was reserved holds a page titled
    /// "Index" at `index.md`. Its next write moves it to a free name, removes
    /// the copy left behind, and the OKF index takes the name.
    #[test]
    fn a_page_already_at_index_md_moves_on_its_next_write() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        page.title = "Index".to_string();
        let moved_path = writer.write_page_for_test(&page).unwrap();

        std::fs::rename(&moved_path, dir.path().join(INDEX_FILE)).unwrap();
        let mut state = writer.load_state();
        state.pages.get_mut(&page.id).unwrap().file = INDEX_FILE.to_string();
        writer.save_state(&state).unwrap();
        let projected = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();

        // Until it is written again, the page's only file stays put: reconcile
        // finds it current and must not replace it with the index.
        let stats = writer
            .reconcile_for_test(std::slice::from_ref(&page))
            .unwrap();
        assert_eq!(stats.rewritten, 0);
        assert_eq!(
            std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap(),
            projected
        );

        let page_path = writer.write_page_for_test(&page).unwrap();
        assert!(page_path.ends_with("index-page.md"), "{page_path}");
        assert!(std::fs::read_to_string(&page_path)
            .unwrap()
            .contains(&format!("origin_id: {}", page.id)));
        assert_eq!(
            writer.page_filename(&page.id).as_deref(),
            Some("index-page.md")
        );
        assert_eq!(index_frontmatter_keys(dir.path()), vec!["okf_version"]);
        let index = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();
        assert!(index.contains("[Index](/index-page.md)"), "{index}");
    }

    /// PR #763 closure review: a home note made by copying a page file keeps
    /// that page's `origin_id`. It is still the user's note, not a copy Wenlan
    /// left behind, and survives the next page write.
    #[test]
    fn a_home_note_copied_from_a_page_file_is_kept() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let page = test_concept();
        let page_path = writer.write_page_for_test(&page).unwrap();
        std::fs::remove_file(dir.path().join(INDEX_FILE)).unwrap();
        let note = format!(
            "{}\n\nMy home note.\n",
            std::fs::read_to_string(&page_path).unwrap()
        );
        std::fs::write(dir.path().join(INDEX_FILE), &note).unwrap();

        let mut other = test_concept();
        other.id = "page_other".to_string();
        other.title = "Other".to_string();
        writer.write_page_for_test(&other).unwrap();
        writer.write_page_for_test(&page).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap(),
            note
        );
    }

    /// A page recorded at `index.md` moves on its next write, but a note the
    /// user saved over that file in the meantime is not its copy and stays.
    #[test]
    fn a_note_saved_over_a_moving_pages_file_is_kept() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        page.title = "Index".to_string();
        let moved_path = writer.write_page_for_test(&page).unwrap();
        std::fs::remove_file(&moved_path).unwrap();
        let mut state = writer.load_state();
        state.pages.get_mut(&page.id).unwrap().file = INDEX_FILE.to_string();
        writer.save_state(&state).unwrap();
        std::fs::write(dir.path().join(INDEX_FILE), "# Home\n").unwrap();

        let page_path = writer.write_page_for_test(&page).unwrap();
        assert!(page_path.ends_with("index-page.md"), "{page_path}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap(),
            "# Home\n"
        );
    }

    /// Review finding 4b: each per-page rewrite inside `reconcile` must skip
    /// its own `index.md` regeneration (`regenerate_index: false`) so only the
    /// single regeneration at the end of `reconcile` runs -- but the end
    /// result still has to be correct for every page reconcile rewrote.
    #[test]
    fn reconcile_over_three_behind_pages_leaves_a_correct_index() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());

        let mut page_a = test_concept();
        page_a.id = "page_a".to_string();
        page_a.title = "Alpha".to_string();
        page_a.space = Some("rust".to_string());
        writer.write_page_for_test(&page_a).unwrap();

        let mut page_b = test_concept();
        page_b.id = "page_b".to_string();
        page_b.title = "Beta".to_string();
        page_b.space = Some("rust".to_string());
        writer.write_page_for_test(&page_b).unwrap();

        let mut page_c = test_concept();
        page_c.id = "page_c".to_string();
        page_c.title = "Gamma".to_string();
        page_c.space = None;
        writer.write_page_for_test(&page_c).unwrap();

        // All three DB rows advance past what is on disk, so every one of
        // them is behind and reconcile rewrites all three.
        for page in [&mut page_a, &mut page_b, &mut page_c] {
            page.version += 1;
        }

        let stats = writer
            .reconcile_for_test(&[page_a.clone(), page_b.clone(), page_c.clone()])
            .unwrap();
        assert_eq!(stats.rewritten, 3);

        let content = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();
        let (fm, body) = crate::sources::obsidian::extract_frontmatter(&content);
        assert_eq!(fm.get_str("okf_version"), Some("0.2"));
        assert!(body.contains("## rust"));
        assert!(body.contains("[Alpha]("));
        assert!(body.contains("[Beta]("));
        assert!(body.contains("## Unfiled"));
        assert!(body.contains("[Gamma]("));
    }

    /// Acceptance 5 fallback (spec 2026-09-16-okf-projection.md): when a
    /// third-party OKF linter cannot be installed, an in-repo test must assert
    /// the MUST rules directly -- every non-index `.md` has parseable
    /// frontmatter with a non-empty `type`, and `index.md` has no other
    /// frontmatter key. The second half is covered by
    /// `write_page_regenerates_index_with_only_okf_version_frontmatter`; this
    /// test covers the first half across every kind of projected file: the
    /// page itself and its `_sources/` stubs.
    #[test]
    fn every_non_index_md_has_parseable_frontmatter_with_a_non_empty_type() {
        fn collect_md_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect_md_files(&path, out);
                } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                    out.push(path);
                }
            }
        }

        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        page.source_memory_ids = vec!["mem_1".to_string(), "mem_2".to_string()];
        writer.write_page_for_test(&page).unwrap();

        let mut md_files = Vec::new();
        collect_md_files(dir.path(), &mut md_files);
        let non_index: Vec<_> = md_files
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| !n.eq_ignore_ascii_case(INDEX_FILE))
                    .unwrap_or(true)
            })
            .collect();
        assert!(
            non_index.len() >= 3,
            "expected the page plus two source stubs, got {non_index:?}"
        );
        for path in non_index {
            let content = std::fs::read_to_string(&path).unwrap();
            let (fm, _body) = crate::sources::obsidian::extract_frontmatter(&content);
            let ty = fm.get_str("type");
            assert!(
                ty.is_some_and(|t| !t.is_empty()),
                "{path:?} must have a non-empty `type` frontmatter key, got {ty:?}"
            );
        }
    }
    #[test]
    fn projection_recovery_preserves_an_editor_open_inode_after_installation() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        let file = writer.write_page_for_test(&page).unwrap();
        let mut editor = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap();
        page.version += 1;
        page.content = "The desktop saved its new draft.".into();
        writer.write_page_for_test(&page).unwrap();
        // An in-place editor may finish saving through its old open handle.
        editor
            .write_all(b"\nlate external draft must survive\n")
            .unwrap();
        editor.sync_all().unwrap();
        assert!(std::fs::read_to_string(&file)
            .unwrap()
            .contains(&page.content));
        let recovered: Vec<_> = std::fs::read_dir(dir.path().join(".wenlan/projection-recovery"))
            .unwrap()
            .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
            .collect();
        assert!(recovered
            .iter()
            .any(|s| s.contains("late external draft must survive")));
        let state = writer.load_state();
        assert_eq!(
            state.pages[&page.id].content_sha256.as_deref(),
            Some(projection_digest(&std::fs::read(&file).unwrap()).as_str())
        );
    }

    #[test]
    fn projection_recovery_does_not_replace_an_editor_file_created_after_move() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        let file = writer.write_page_for_test(&page).unwrap();
        let old = std::fs::read(&file).unwrap();
        let state = writer.load_state();
        let filename = state.pages[&page.id].file.clone();
        page.version += 1;
        page.content = "new desktop prose".into();
        let result = KnowledgeProjectionWrite::with_projection_capabilities(dir.path(), |caps| {
            write_page_preserving_external_edits_with_hook(
                caps,
                &filename,
                ".race.tmp",
                &page,
                render_markdown(&page).as_bytes(),
                state.pages.get(&page.id),
                || {
                    std::fs::write(&file, b"external replacement at the old pathname")?;
                    Ok(())
                },
            )
        });
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&file).unwrap(),
            b"external replacement at the old pathname"
        );
        assert!(
            std::fs::read_dir(dir.path().join(".wenlan/projection-recovery"))
                .unwrap()
                .any(|e| std::fs::read(e.unwrap().path()).unwrap() == old)
        );
    }

    #[cfg(unix)]
    #[test]
    fn interrupted_projection_state_temp_keeps_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        writer.write_page_for_test(&test_concept()).unwrap();
        let state_path = dir.path().join(".wenlan/state.json");
        std::fs::set_permissions(&state_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let before = std::fs::read(&state_path).unwrap();
        let temporary = dir.path().join(".wenlan/.private-state.tmp");
        let result = KnowledgeProjectionWrite::with_projection_capabilities(dir.path(), |caps| {
            write_page_atomically_nofollow_with_hook(
                &caps.wenlan,
                "state.json",
                ".private-state.tmp",
                b"private replacement metadata",
                || {
                    assert_eq!(
                        std::fs::read(&temporary).unwrap(),
                        b"private replacement metadata"
                    );
                    assert_eq!(
                        std::fs::metadata(&temporary).unwrap().permissions().mode() & 0o777,
                        0o600
                    );
                    Err(std::io::Error::other("injected after private temp write"))
                },
            )
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&state_path).unwrap(), before);
        assert!(!temporary.exists());
    }

    #[cfg(unix)]
    #[test]
    fn projection_state_replacement_preserves_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        writer.write_page_for_test(&page).unwrap();
        let state_path = dir.path().join(".wenlan/state.json");
        std::fs::set_permissions(&state_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        page.version += 1;
        page.content = "updated canonical prose".into();
        writer.write_page_for_test(&page).unwrap();
        assert_eq!(
            std::fs::metadata(state_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn projection_recovery_repairs_file_installed_before_first_state_save() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let page = test_concept();
        let guard = writer.begin_test_write();
        create_projection_root_nofollow(dir.path()).unwrap();
        let interrupted =
            KnowledgeProjectionWrite::with_projection_capabilities(dir.path(), |caps| {
                writer.write_page_with_lock_held_and_hook(
                    caps,
                    &guard,
                    &page,
                    || Err(WenlanError::Conflict("injected before state save".into())),
                    true,
                )
            });
        assert!(interrupted.is_err());
        assert!(writer.page_filename(&page.id).is_none());
        let file = writer.write_page_for_test(&page).unwrap();
        assert_eq!(
            std::path::Path::new(&file).file_name().unwrap(),
            "rust-ownership.md"
        );
        assert!(!dir.path().join("rust-ownership-2.md").exists());
        assert_eq!(
            writer.load_state().pages[&page.id]
                .content_sha256
                .as_deref(),
            Some(projection_digest(&std::fs::read(&file).unwrap()).as_str())
        );
    }

    #[test]
    fn projection_recovery_preserves_legacy_differing_prose_and_adopts_matching_prose() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        let file = writer.write_page_for_test(&page).unwrap();
        let mut state = writer.load_state();
        state.pages.get_mut(&page.id).unwrap().content_sha256 = None;
        writer.save_state(&state).unwrap();
        page.version += 1;
        page.content = "DB moved past the legacy file".into();
        let old = std::fs::read(&file).unwrap();
        assert!(writer.write_page_for_test(&page).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), old);
        let stats = writer
            .reconcile_for_test(std::slice::from_ref(&page))
            .unwrap();
        assert_eq!(stats.errors, 1);
        assert_eq!(std::fs::read(&file).unwrap(), old);
        // Human has adopted the external prose: establishing the byte baseline
        // does not delete unknown text and can now complete the projection.
        page.content = test_concept().content;
        writer.write_page_for_test(&page).unwrap();
        assert!(writer.load_state().pages[&page.id].content_sha256.is_some());
    }
    #[test]
    fn projection_recovery_preserves_edits_to_the_actual_moved_inode() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = test_concept();
        let file = writer.write_page_for_test(&page).unwrap();
        let mut editor = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap();
        let state = writer.load_state();
        let filename = state.pages[&page.id].file.clone();
        page.version += 1;
        page.content = "desktop candidate".into();
        let result = KnowledgeProjectionWrite::with_projection_capabilities(dir.path(), |caps| {
            write_page_preserving_external_edits_with_hook(
                caps,
                &filename,
                ".moved-race.tmp",
                &page,
                render_markdown(&page).as_bytes(),
                state.pages.get(&page.id),
                || {
                    editor.write_all(b"\nexternal prose arrived during replacement\n")?;
                    editor.sync_all()?;
                    Ok(())
                },
            )
        });
        assert!(matches!(result, Err(WenlanError::Conflict(_))));
        assert!(std::fs::read_to_string(&file)
            .unwrap()
            .contains("external prose arrived during replacement"));
        assert!(!std::fs::read_to_string(&file)
            .unwrap()
            .contains("desktop candidate"));
    }

    #[test]
    fn projection_recovery_refuses_a_writer_holding_an_older_page_snapshot() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let older = test_concept();
        let file = writer.write_page_for_test(&older).unwrap();
        let mut newer = older.clone();
        newer.version += 1;
        newer.content = "a second writer already projected this version".into();
        writer.write_page_for_test(&newer).unwrap();
        let saved = std::fs::read(&file).unwrap();
        assert!(writer.write_page_for_test(&older).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), saved);
        assert_eq!(writer.load_state().pages[&older.id].version, newer.version);
    }
}
