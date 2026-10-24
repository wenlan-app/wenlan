// SPDX-License-Identifier: Apache-2.0
//! Document source types -- MemoryType enum, RawDocument, SourceType, SyncStatus.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Closed taxonomy of memory facets -- validated at API boundary.
/// Stored as lowercase TEXT in SQLite.
///
/// Six user-facing types after taxonomy refactor:
/// - Identity, Preference -- about the user (Protected tier)
/// - Decision -- choices made (Standard tier)
/// - Lesson -- positive learnings (Standard tier)
/// - Gotcha -- traps/warnings/negative learnings (Standard tier)
/// - Fact -- durable knowledge (Standard tier)
///
/// Goal is deprecated: incoming "goal" parses as Identity (aspirations
/// are part of who the user is). Existing rows migrate via DB migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    Identity,
    Preference,
    Decision,
    Lesson,
    Gotcha,
    Fact,
}

impl MemoryType {
    /// All valid lowercase string values (6 canonical types).
    pub fn all_values() -> &'static [&'static str] {
        &[
            "identity",
            "preference",
            "decision",
            "lesson",
            "gotcha",
            "fact",
        ]
    }

    /// Check if input is the "profile" high-level alias (case-insensitive).
    /// Used by the store flow to detect when async LLM sub-classification is needed.
    pub fn is_profile_alias(s: &str) -> bool {
        s.eq_ignore_ascii_case("profile")
    }

    /// Check if input is the "knowledge" high-level alias (case-insensitive).
    /// Knowledge expands to fact | lesson | gotcha and needs sub-classification.
    pub fn is_knowledge_alias(s: &str) -> bool {
        s.eq_ignore_ascii_case("knowledge")
    }
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Identity => "identity",
            Self::Preference => "preference",
            Self::Decision => "decision",
            Self::Lesson => "lesson",
            Self::Gotcha => "gotcha",
            Self::Fact => "fact",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for MemoryType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "identity" => Ok(Self::Identity),
            "preference" => Ok(Self::Preference),
            "decision" => Ok(Self::Decision),
            "lesson" => Ok(Self::Lesson),
            "gotcha" => Ok(Self::Gotcha),
            "fact" => Ok(Self::Fact),
            // Deprecated: "goal" folds into Identity (aspirations = identity).
            "goal" => Ok(Self::Identity),
            // High-level alias: "profile" needs async LLM sub-classification
            "profile" => Err(
                "profile requires sub-classification into identity or preference -- use classify_memory_type".to_string()
            ),
            // High-level alias: "knowledge" needs sub-classification (fact | lesson | gotcha)
            "knowledge" => Err(
                "knowledge requires sub-classification into fact, lesson, or gotcha -- use classify_memory_type".to_string()
            ),
            // Backward compat: removed types map to Fact
            "correction" | "custom" | "recap" => Ok(Self::Fact),
            _ => Err(format!(
                "invalid memory_type '{}', valid values: {}",
                s,
                Self::all_values().join(", ")
            )),
        }
    }
}

/// Stability tiers determine supersede behavior, confidence defaults, and retrieval decay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StabilityTier {
    /// identity, preference -- supersede requires human confirmation
    Protected,
    /// fact, decision, lesson, gotcha -- supersede auto-applies unconfirmed
    Standard,
    /// catch-all for unknown / removed types -- supersede auto-applies silently
    Ephemeral,
}

/// Map a memory type string to its stability tier. NULL -> Ephemeral.
pub fn stability_tier(memory_type: Option<&str>) -> StabilityTier {
    match memory_type {
        Some("identity") | Some("preference") => StabilityTier::Protected,
        Some("fact") | Some("decision") | Some("lesson") | Some("gotcha") => {
            StabilityTier::Standard
        }
        // Deprecated: "goal" still in DB rows pre-migration -> treat as Identity (Protected).
        Some("goal") => StabilityTier::Protected,
        _ => StabilityTier::Ephemeral,
    }
}

/// A raw document fetched from any source, ready for chunking and embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawDocument {
    /// Source identifier ("gmail", "notion", "local_files", etc.)
    pub source: String,
    /// Unique ID within the source (message ID, page ID, file path)
    pub source_id: String,
    /// Document title (filename, subject line, page title)
    pub title: String,
    /// LLM-generated summary (stored separately from chunk content)
    pub summary: Option<String>,
    /// Plain text content
    pub content: String,
    /// Deep link back to the source (URL, file path)
    pub url: Option<String>,
    /// Unix timestamp of last modification
    pub last_modified: i64,
    /// Additional metadata
    pub metadata: HashMap<String, String>,

    // --- Memory layer fields (all optional for backward compat) ---
    /// Memory category: "preference", "decision", "fact", "goal", "relationship"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_type: Option<String>,
    /// Space context: "work", "personal", "health", or "project:<name>"
    #[serde(default, alias = "domain", skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// Which AI agent stored this memory (e.g. "claude-code", "chatgpt")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_agent: Option<String>,
    /// Confidence score (0.0-1.0) assigned by the storing agent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    /// Whether a human has confirmed this memory
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed: Option<bool>,
    /// Stability tier: "new", "learned", or "confirmed"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stability: Option<String>,
    /// source_id of the memory this entry supersedes (version chain)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    /// Whether this is a pending revision awaiting human approval (Protected tier supersede)
    #[serde(default)]
    pub pending_revision: bool,
    /// Link to a knowledge graph entity (nullable, cascade handled manually)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    /// Quality assessment: "low", "medium", "high" (NULL = unassessed)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// T8 salience prior: importance rating 1-10 (LLM-assigned), NULL = unrated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance: Option<u8>,
    /// Whether this memory is a recap/summary of other memories
    #[serde(default)]
    pub is_recap: bool,
    /// Deprecated: enrichment status is now derived from the `enrichment_steps` table.
    /// This field is ignored on INSERT. Kept for API compatibility with downstream consumers.
    #[serde(default = "default_enrichment_status")]
    pub enrichment_status: String,
    /// How superseded content is handled: "hide" (default) or "archive" (visible but muted)
    #[serde(default = "default_supersede_mode")]
    pub supersede_mode: String,
    /// JSON object with type-specific structured fields (e.g. {"claim": "...", "context": "..."})
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_fields: Option<String>,
    /// LLM-generated question this memory answers -- embedded for vector search
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval_cue: Option<String>,
    /// Original prose content, preserved when structured_fields are promoted to primary content
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_text: Option<String>,
    /// Provenance: content hash of the source file this document came from.
    /// All chunks of one file share this hash (folder / multi-format ingest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

fn default_enrichment_status() -> String {
    "raw".to_string()
}

fn default_supersede_mode() -> String {
    "hide".to_string()
}

impl Default for RawDocument {
    fn default() -> Self {
        Self {
            source: String::new(),
            source_id: String::new(),
            title: String::new(),
            summary: None,
            content: String::new(),
            url: None,
            last_modified: 0,
            metadata: HashMap::new(),
            memory_type: None,
            space: None,
            source_agent: None,
            confidence: None,
            confirmed: None,
            stability: None,
            supersedes: None,
            pending_revision: false,
            entity_id: None,
            quality: None,
            importance: None,
            is_recap: false,
            enrichment_status: "raw".to_string(),
            supersede_mode: "hide".to_string(),
            structured_fields: None,
            retrieval_cue: None,
            source_text: None,
            content_hash: None,
        }
    }
}

/// Persisted source type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    Obsidian,
    Directory,
    /// A file-based Logseq graph: the Markdown pages under `pages/` and
    /// `journals/`, followed like an Obsidian vault.
    Logseq,
    /// An Open Knowledge Format (OKF) bundle, such as an OpenWiki wiki: one
    /// source page per concept, provenance kept for display, one Space.
    Okf,
}

impl SourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Obsidian => "obsidian",
            Self::Directory => "directory",
            Self::Logseq => "logseq",
            Self::Okf => "okf",
        }
    }
}

/// Sync status for a connected source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncStatus {
    Active,
    Paused,
    Error(String),
    /// The source root (directory or single file) is missing or unreadable,
    /// or an `okf` source cannot import (its folder is a Wenlan export, or its
    /// Space was deleted). Distinct from `Error` (a sync that ran but hit
    /// per-file failures) and `Paused` (user-initiated): "root-gone !=
    /// file-gone", so while a source is `Unavailable` the sync deletes
    /// nothing. Auto-recovers -- the next sync that finds the root live and
    /// importable flips it back to `Active`.
    Unavailable(String),
}

/// Status of a connected source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceStatus {
    pub name: String,
    pub connected: bool,
    pub requires_auth: bool,
    pub last_sync: Option<i64>,
    pub document_count: u64,
    pub error: Option<String>,
}

/// Persisted source configuration -- stored in config.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub source_type: SourceType,
    pub path: std::path::PathBuf,
    #[serde(default = "default_sync_status")]
    pub status: SyncStatus,
    pub last_sync: Option<i64>,
    #[serde(default)]
    pub file_count: u64,
    #[serde(default)]
    pub memory_count: u64,
    /// Number of files that failed to read / ingest in the last sync.
    #[serde(default)]
    pub last_sync_errors: u64,
    /// Categorized detail of last sync errors for UI display.
    /// Known values: "google_drive_offline", "file_read_errors",
    /// "document_enrichment_failed" (read fine, the worker gave up).
    #[serde(default)]
    pub last_sync_error_detail: Option<String>,
    /// Space every new document from this source lands in, fixed at
    /// registration. `None` keeps today's behavior (unfiled, or the
    /// Obsidian first-tag rule). Only `okf` sources set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// `okf` sources: files in the document queue that were not yet prepared
    /// (parsed and embedded) when the last sync finished.
    #[serde(default)]
    pub queued_files: u64,
    /// `okf` sources: files not yet handed to the queue after the last sync,
    /// because a batch was still being prepared or that sync's batch was full.
    #[serde(default)]
    pub waiting_files: u64,
}

fn default_sync_status() -> SyncStatus {
    SyncStatus::Active
}

/// Whether AI may change one of the person's own notes in a vault.
///
/// The note's file is the person's; this setting lives only on Wenlan's side.
/// There is deliberately no "allowed" value yet: an AI change to a person's
/// note is always a proposal they accept first, or nothing at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiChanges {
    /// AI may propose a change; the file is written only when the person
    /// accepts it.
    AskFirst,
    /// AI only cites the note. It proposes nothing and changes nothing.
    Never,
}

/// Where a note's effective [`AiChanges`] came from, most specific first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiChangesFrom {
    /// Set on this file.
    File,
    /// Set on a folder that holds it (the nearest one wins).
    Folder,
    /// Set for the whole vault.
    Vault,
    /// Nothing set: a person's notes start at [`AiChanges::Never`].
    Default,
    /// PDFs are always [`AiChanges::Never`].
    Pdf,
}

/// The effective setting for one path in a vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteAiChanges {
    /// Vault-relative path with `/` separators: `""` is the vault, a folder
    /// ends with `/`.
    pub path: String,
    pub value: AiChanges,
    pub from: AiChangesFrom,
}

/// A change an AI proposed to one of the person's notes. Nothing is written
/// to the note until the person accepts it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteProposal {
    pub id: String,
    pub source_id: String,
    /// Vault-relative path with `/` separators.
    pub path: String,
    /// The whole proposed file.
    pub content: String,
    /// Why the AI wants the change, in its words.
    #[serde(default)]
    pub reason: String,
    /// The agent that proposed it.
    pub agent: String,
    /// Unix seconds.
    pub created_at: i64,
    /// sha256 of the file the proposal was made against. Accepting refuses
    /// when the file no longer matches, so an edit made since is never lost.
    pub base_hash: String,
}
