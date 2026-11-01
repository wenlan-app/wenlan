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
//! Proposals and history follow in their own parts and add to the same
//! manifest. Every file is
//! read back after it is rendered; one that does not read back as written
//! is listed.

pub mod artifacts;
pub mod briefs;
pub mod canvases;
pub mod memories;
pub mod pages;
pub mod records;
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
        ],
    ),
    (
        "pages",
        "cache",
        &[
            "embedding",
            "embedding_updated_at",
            "community_id",
            "confidence",
            "last_compiled",
            "sources_updated_count",
            "stale_reason",
            "refresh_blocked_reason",
        ],
    ),
    (
        "memories",
        "history",
        &["version", "changelog", "access_count", "last_accessed"],
    ),
    // A quick capture's `content` (the tidied text) is also cache: its
    // file holds what the person typed. Chunk boundaries are cache too: the
    // comparison checks the joined text, not the rows.
    (
        "memories",
        "cache",
        &[
            "embedding",
            "summary",
            "confidence",
            "quality",
            "stability",
            "enrichment_status",
            "needs_reembed",
            "content_hash",
            "refinement_status",
            "effective_confidence",
            "word_count",
            "language",
            "chunk_type",
            "byte_start",
            "byte_end",
            "semantic_unit",
            "last_distilled_at",
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
}

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
        Ok(Self { root })
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
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                WenlanError::Validation(format!("cannot write export file {relative}: {error}"))
            })?;
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
        Ok(ExportedFile {
            id: id.to_string(),
            file_type: file_type.to_string(),
            path: relative.to_string(),
            sha256: sha256(bytes),
        })
    }

    /// Writes the manifest last, so a folder without one is an export that
    /// did not finish.
    pub fn write_manifest(&self, manifest: &Manifest) -> Result<(), WenlanError> {
        let text = serde_json::to_string_pretty(manifest)
            .map_err(|error| WenlanError::Validation(format!("manifest: {error}")))?;
        let path = self.root.join(MANIFEST_FILE);
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
mod tests;
