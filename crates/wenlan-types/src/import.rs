// SPDX-License-Identifier: Apache-2.0
//! Types for the chat-export import endpoint.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportChatExportRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportChatExportResponse {
    pub import_id: String,
    pub vendor: String,
    pub conversations_total: usize,
    pub conversations_new: usize,
    pub conversations_skipped_existing: usize,
    pub memories_stored: usize,
    /// Artifacts and canvases kept as originals, each a file under
    /// `originals/` with its own page.
    #[serde(default)]
    pub originals_kept: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingImport {
    pub id: String,
    pub vendor: String,
    pub stage: String,
    pub source_path: String,
    pub processed_conversations: i64,
    pub total_conversations: Option<i64>,
}

// ===== Import phases and batch status =====

/// A user-facing phase of one import. `Ingest` and `Store` finish inside the
/// import request; the rest are background work the ambient scheduler owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportPhase {
    Ingest,
    Store,
    Detect,
    Enrich,
    Link,
    Distill,
}

impl ImportPhase {
    /// Every phase, in the order a user sees them.
    pub const ALL: [ImportPhase; 6] = [
        ImportPhase::Ingest,
        ImportPhase::Store,
        ImportPhase::Detect,
        ImportPhase::Enrich,
        ImportPhase::Link,
        ImportPhase::Distill,
    ];

    /// The `enrichment_steps.step_name` rows that make up this phase.
    ///
    /// Empty for `Ingest` and `Store`, which finish inside the import request,
    /// and for `Distill`, which writes no step row — its progress is the number
    /// of distilled pages citing the batch, with no knowable total.
    pub fn step_names(self) -> &'static [&'static str] {
        match self {
            ImportPhase::Ingest | ImportPhase::Store | ImportPhase::Distill => &[],
            ImportPhase::Detect => &["entity_extract", "entity_link"],
            ImportPhase::Enrich => &["title_enrich"],
            ImportPhase::Link => &["page_growth"],
        }
    }

    /// True when this phase reports a `total` a bar can be drawn against.
    /// `Distill` never does: it reports a live count only.
    pub fn has_known_total(self) -> bool {
        !matches!(self, ImportPhase::Distill)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportPhaseState {
    Pending,
    Running,
    Complete,
    Failed,
}

/// Real counts for one phase. Never a timer: `done` and `total` are rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPhaseStatus {
    pub phase: ImportPhase,
    pub state: ImportPhaseState,
    /// Units finished — memories for every phase except `Distill`, which counts pages.
    pub done: u64,
    /// Units expected. 0 means not yet known.
    pub total: u64,
    /// Units that failed and will not be retried.
    pub failed: u64,
}

/// Aggregate progress for one import batch, derived from the batch's memories
/// and their `enrichment_steps` rows. Every field is a live count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportBatchStatus {
    pub batch_id: String,
    /// The `import_source` the memories were written with (chatgpt, claude, other).
    pub source: String,
    pub started_at: i64,
    pub updated_at: i64,
    /// Import requests received so far for this batch.
    pub chunks_received: u32,
    pub memories_imported: u64,
    pub memories_skipped: u64,
    /// Entities this batch's memories link to, by lifecycle state.
    pub entities_detected: u64,
    pub entities_established: u64,
    pub pages_distilled: u64,
    pub phases: Vec<ImportPhaseStatus>,
    /// Every background phase has settled (complete or failed).
    pub complete: bool,
    #[serde(default)]
    pub space: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveImportBatchesResponse {
    pub batches: Vec<ImportBatchStatus>,
}
