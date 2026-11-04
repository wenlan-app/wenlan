// SPDX-License-Identifier: Apache-2.0
//! The lossless export (storage migration step 2, spec section 10): the
//! database written out as the files it will become, into a new folder, so a
//! rebuild from that folder can be compared field by field with the database
//! it came from. The database is only read; deleting the folder undoes it.
//!
//! ```text
//! <out>/visible/…            pages, in the folders they are in today
//! <out>/backstage/entities/  entity files
//! <out>/backstage/archive/   archived pages
//! <out>/backstage/memories/  memory files, by month
//! <out>/backstage/originals/ imported chats, by vendor
//! <out>/backstage/briefs/    one Brief per space
//! <out>/backstage/proposals/canvas/ suggested and dismissed canvas parts
//! <out>/backstage/layout/canvas/    canvases of pages with no page file
//! <out>/backstage/settings/  spaces, things, AI tools, profile, onboarding
//! <out>/backstage/records/   records with no file shape of their own yet
//! <out>/backstage/history/   the append-only log; reads under reads/
//! <out>/backstage/layout/    the palace layout, copied as it is
//! <out>/export-manifest.json every file written, and every value left out
//! ```
//!
//! So far: pages, entities and archived pages ([`pages::export_pages`])
//! memories and imported chats ([`memories::export_memories`]), Briefs
//! ([`briefs::export_briefs`]), canvases beside their pages
//! ([`canvases::export_canvases`]) and artifact originals
//! ([`artifacts::export_artifacts`]), settings
//! ([`settings::export_settings`]) and the remaining records
//! ([`records::export_records`]). [`FILE_TABLE_HOMES`] names where every
//! file-class table goes, and a test keeps it equal to the table classes.
//! History ([`history::export_history`]) is the append-only log.
//! [`run::export_lossless`] runs every part in order and writes the
//! manifest. Proposals follow in their own part. Every file is
//! read back after it is rendered; one that does not read back as written
//! is listed.

pub mod artifacts;
pub mod briefs;
pub mod canvases;
pub mod history;
pub mod memories;
pub mod pages;
pub mod records;
pub mod run;
pub mod settings;

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::WenlanError;

/// Bumped when a file shape or the manifest changes, so a rebuild never
/// reads an export it does not understand.
pub const EXPORT_FORMAT: u32 = 1;
pub const MANIFEST_FILE: &str = "export-manifest.json";

/// The export part that writes each table classed
/// [`File`](crate::db::table_classes::TableClass::File).
pub const FILE_TABLE_HOMES: &[(&str, &str)] = &[
    ("pages", "pages"),
    ("page_maps", "canvases"),
    ("page_map_nodes", "canvases"),
    ("page_map_edges", "canvases"),
    ("memories", "memories"),
    ("briefs", "briefs"),
    ("brief_items", "briefs"),
    ("originals", "artifacts"),
    ("spaces", "settings"),
    ("things", "settings"),
    ("thing_maps", "settings"),
    ("thing_map_nodes", "settings"),
    ("thing_map_edges", "settings"),
    ("thing_map_proposals", "settings"),
    ("thing_merge_proposals", "settings"),
    ("profiles", "settings"),
    ("agent_connections", "settings"),
    ("onboarding_milestones", "settings"),
    ("app_metadata", "settings"),
    ("source_annotations", "records"),
    ("okf_concepts", "records"),
    ("okf_concept_links", "records"),
    ("page_basis", "records"),
    ("page_code_basis", "records"),
    ("entity_page_map", "records"),
    ("page_evidence", "records"),
    ("page_sources", "records"),
    ("edges", "records"),
    ("provenance_roots", "records"),
    ("provenance_root_minhash_bands", "records"),
    ("observations", "records"),
    ("relations", "records"),
    ("entailment_cache", "records"),
    ("eval_signals", "records"),
    ("enrichment_steps", "records"),
];

/// Columns of the tables written field by field (pages, entities, memories,
/// Briefs) that their files hold. Every other column of those tables is in
/// [`DEFERRED_COLUMNS`]; a test keeps the two equal to the live schema.
pub const FILE_COLUMNS: &[(&str, &[&str])] = &[
    (
        "pages",
        &[
            "id",
            "title",
            "content",
            "status",
            "space",
            "created_at",
            "last_modified",
            "citations",
            "entity_id",
            "entity_type",
            "entity_confirmed",
            "aliases",
            "established_by",
            "summary",
            "entity_created_at",
            "entity_updated_at",
        ],
    ),
    (
        "memories",
        &[
            "id",
            "source",
            "source_id",
            "chunk_index",
            "content",
            "title",
            "url",
            "memory_type",
            "space",
            "source_agent",
            "confirmed",
            "pinned",
            "supersedes",
            "supersede_mode",
            "pending_revision",
            "structured_fields",
            "source_text",
            "created_at",
            "event_date",
            "event_end",
            "importance",
            "episode_of",
            "origin_class",
            "session_id",
            "authored_content",
            "entity_id",
            "is_recap",
            "last_modified",
            "retrieval_cue",
            // What enrichment wrote; a model would have to write it again.
            "summary",
        ],
    ),
    (
        "briefs",
        &[
            "space_id",
            "last_session_summary",
            "last_handoff_at",
            "version",
            "updated_at",
        ],
    ),
    (
        "brief_items",
        &[
            "id", "space_id", "text", "state", "added_at", "gate", "version",
        ],
    ),
];

/// Where the columns no file holds go: `history` (the history export
/// restores them) or `cache` (the rebuild recomputes them). The comparison
/// step checks each one against its source.
pub const DEFERRED_COLUMNS: &[(&str, &str, &[&str])] = &[
    (
        "pages",
        "history",
        &[
            "version",
            "review_status",
            "review_required",
            "user_edited",
            "kind",
            "creation_kind",
            "workspace",
            "changelog",
            "source_memory_ids",
            "source_revision",
            "incarnation",
            "basis_revision",
            "source_agent",
            // What the person is shown about a page: "its sources were
            // updated", "refresh is blocked". Nothing works these out again
            // after a restore (spec section 3).
            "sources_updated_count",
            "stale_reason",
            "refresh_blocked_reason",
            // Worked out by jobs that do not run again for an unchanged
            // page, so a restore keeps them as they were.
            "community_id",
            "confidence",
        ],
    ),
    (
        "pages",
        "cache",
        &["embedding", "embedding_updated_at", "last_compiled"],
    ),
    (
        "memories",
        "history",
        &[
            "version",
            "changelog",
            "access_count",
            "last_accessed",
            // What enrichment and the jobs after it worked out, and whether
            // a memory still waits for enrichment: nothing works these out
            // again for a memory the steps record says is done.
            "confidence",
            "quality",
            "enrichment_status",
            "last_distilled_at",
            // A folder document's file hash: what the folder sync compares,
            // and what keeps a document from seeding a page.
            "content_hash",
            // New, learned or confirmed: the nightly promotion reads it.
            "stability",
        ],
    ),
    // A quick capture's `content` (the tidied text) is also cache: its
    // file holds what the person typed. Chunk boundaries are cache too: the
    // comparison checks the joined text, not the rows.
    (
        "memories",
        "cache",
        &[
            "embedding",
            "needs_reembed",
            "refinement_status",
            "effective_confidence",
            "word_count",
            "language",
            "chunk_type",
            "byte_start",
            "byte_end",
            "semantic_unit",
        ],
    ),
    ("brief_items", "history", &["updated_at"]),
];

/// [`DEFERRED_COLUMNS`] as the manifest lists them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deferred {
    pub table: String,
    pub to: String,
    pub columns: Vec<String>,
}

/// Where an export goes.
#[derive(Debug, Clone)]
pub struct ExportDirs {
    pub root: PathBuf,
    /// Set when the export refreshes an earlier one where it is
    /// ([`Self::in_place`]) instead of writing a new folder.
    in_place: Option<std::sync::Arc<InPlace>>,
}

/// What an in-place refresh tracks.
#[derive(Debug, Default)]
struct InPlace {
    /// Files written because they were new or their bytes changed.
    written: std::sync::atomic::AtomicUsize,
}

/// Suffix of the file a refresh writes before it renames it over the old
/// one. A leftover (the refresh stopped) is removed by the next refresh.
pub const TEMP_SUFFIX: &str = ".wenlan-tmp";

impl ExportDirs {
    /// `root` must not exist yet, or be an empty folder: an export never
    /// writes over anything.
    pub fn create(root: impl Into<PathBuf>) -> Result<Self, WenlanError> {
        let root = root.into();
        match std::fs::read_dir(&root) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return Err(WenlanError::Validation(format!(
                        "export folder {} is not empty",
                        root.display()
                    )));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(&root)?;
            }
            Err(error) => return Err(error.into()),
        }
        Ok(Self {
            root,
            in_place: None,
        })
    }

    /// Refresh the export already in `root` where it is: a file whose bytes
    /// did not change is not written again, a changed one is written beside
    /// it and renamed over it (so a reader never sees half a file), and
    /// [`Self::finish_in_place`] removes the files the new manifest no
    /// longer lists. `root` is created when missing.
    pub fn in_place(root: impl Into<PathBuf>) -> Result<Self, WenlanError> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self {
            root,
            in_place: Some(std::sync::Arc::new(InPlace::default())),
        })
    }

    /// How many files an in-place refresh wrote so far.
    pub fn written(&self) -> usize {
        self.in_place.as_ref().map_or(0, |in_place| {
            in_place.written.load(std::sync::atomic::Ordering::Relaxed)
        })
    }

    /// Counts files an in-place refresh wrote other than through
    /// [`Self::write_bytes`] (the history log).
    pub(crate) fn count_written(&self, files: usize) {
        if let Some(in_place) = &self.in_place {
            in_place
                .written
                .fetch_add(files, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub fn is_in_place(&self) -> bool {
        self.in_place.is_some()
    }

    pub fn visible(&self) -> PathBuf {
        self.root.join("visible")
    }

    pub fn backstage(&self) -> PathBuf {
        self.root.join("backstage")
    }

    /// Writes `text` at `relative` (forward slashes) under the root and
    /// returns what the manifest records for it. Refuses a path that would
    /// leave the root or a file that is already there.
    pub(crate) fn write(
        &self,
        relative: &str,
        id: &str,
        file_type: &str,
        text: &str,
    ) -> Result<ExportedFile, WenlanError> {
        self.write_bytes(relative, id, file_type, text.as_bytes())
    }

    /// [`Self::write`] for bytes that need not be text.
    pub(crate) fn write_bytes(
        &self,
        relative: &str,
        id: &str,
        file_type: &str,
        bytes: &[u8],
    ) -> Result<ExportedFile, WenlanError> {
        let path = self.root.join(safe_relative(relative)?);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let exported = ExportedFile {
            id: id.to_string(),
            file_type: file_type.to_string(),
            path: relative.to_string(),
            sha256: sha256(bytes),
        };
        if let Some(in_place) = &self.in_place {
            if std::fs::read(&path).is_ok_and(|old| old == bytes) {
                return Ok(exported);
            }
            replace_file(&path, bytes)?;
            in_place
                .written
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(exported);
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                WenlanError::Validation(format!("cannot write export file {relative}: {error}"))
            })?;
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
        Ok(exported)
    }

    /// Writes the manifest last, so a folder without one is an export that
    /// did not finish.
    pub fn write_manifest(&self, manifest: &Manifest) -> Result<(), WenlanError> {
        let text = serde_json::to_string_pretty(manifest)
            .map_err(|error| WenlanError::Validation(format!("manifest: {error}")))?;
        let path = self.root.join(MANIFEST_FILE);
        if self.in_place.is_some() {
            return replace_file(&path, format!("{text}\n").as_bytes());
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        std::io::Write::write_all(&mut file, text.as_bytes())?;
        std::io::Write::write_all(&mut file, b"\n")?;
        file.sync_all()?;
        Ok(())
    }
}

impl ExportDirs {
    /// Ends an in-place refresh that produced `manifest` (call it before
    /// [`Self::write_manifest`]): removes every file under `root` that
    /// `manifest` does not list (records the database no longer has, and
    /// leftovers of a refresh that stopped), so the folder holds exactly
    /// the export. `root` is the export's own folder; nothing else lives
    /// there. Returns how many files were removed.
    pub fn finish_in_place(&self, manifest: &Manifest) -> Result<usize, WenlanError> {
        let listed: std::collections::HashSet<PathBuf> = manifest
            .files
            .iter()
            .filter_map(|file| safe_relative(&file.path).ok())
            .collect();
        remove_unlisted(&self.root, Path::new(""), &listed)
    }
}

fn remove_unlisted(
    root: &Path,
    relative: &Path,
    listed: &std::collections::HashSet<PathBuf>,
) -> Result<usize, WenlanError> {
    let dir = root.join(relative);
    let mut removed = 0;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let inner = relative.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            removed += remove_unlisted(root, &inner, listed)?;
            // Empty now: the records that were in it are gone.
            let _ = std::fs::remove_dir(entry.path());
        } else if inner != Path::new(MANIFEST_FILE)
            && inner != Path::new(crate::store::write_log::POSITION_FILE)
            && !listed.contains(&inner)
        {
            std::fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// The manifest an export at `root` wrote, if it has one that reads.
pub fn read_manifest(root: &Path) -> Option<Manifest> {
    let text = std::fs::read_to_string(root.join(MANIFEST_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes `bytes` beside `path` and renames it over `path`, so a reader
/// sees the old file or the new one, never half of one.
fn replace_file(path: &Path, bytes: &[u8]) -> Result<(), WenlanError> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(TEMP_SUFFIX);
    let temp = path.with_file_name(name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp)?;
    std::io::Write::write_all(&mut file, bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temp, path)?;
    Ok(())
}

/// `relative` as a path under the root: forward slashes, no `..`, nothing
/// absolute.
fn safe_relative(relative: &str) -> Result<PathBuf, WenlanError> {
    let path = Path::new(relative);
    let fine = !relative.is_empty()
        && !relative.contains('\\')
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if fine {
        Ok(path.to_path_buf())
    } else {
        Err(WenlanError::Validation(format!(
            "export path {relative:?} is not inside the export folder"
        )))
    }
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportedFile {
    pub id: String,
    #[serde(rename = "type")]
    pub file_type: String,
    pub path: String,
    pub sha256: String,
}

/// A value the files do not hold, so the comparison can tell an expected
/// difference from a lost one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftOut {
    pub id: String,
    pub field: String,
    pub why: String,
}

/// Rows that are deliberately not files, counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotFiles {
    pub what: String,
    pub count: usize,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub files: Vec<ExportedFile>,
    pub left_out: Vec<LeftOut>,
    pub not_files: Vec<NotFiles>,
    /// Columns no file holds, and where each goes.
    pub deferred: Vec<Deferred>,
}

impl Manifest {
    pub fn new() -> Self {
        Self {
            format: EXPORT_FORMAT,
            deferred: DEFERRED_COLUMNS
                .iter()
                .filter(|(_, _, columns)| !columns.is_empty())
                .map(|(table, to, columns)| Deferred {
                    table: table.to_string(),
                    to: to.to_string(),
                    columns: columns.iter().map(|c| c.to_string()).collect(),
                })
                .collect(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
#[path = "export_test.rs"]
pub(crate) mod tests;
