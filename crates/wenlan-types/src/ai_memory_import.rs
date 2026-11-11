// SPDX-License-Identifier: Apache-2.0
//! Wire shapes of following what Claude Code and Codex remember on this
//! computer (`/api/import/ai-memory`).

use serde::{Deserialize, Serialize};

/// What one pass over the tools' memory did: where each memory it read went.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportCounts {
    /// Memories read.
    pub read: usize,
    /// Left out by the secret and personal-data patterns.
    pub sensitive: usize,
    /// Left out by the four questions (tool habits, expiring, private, unsure).
    pub not_knowledge: usize,
    /// Wenlan already holds it: nothing added.
    pub already: usize,
    /// Added as new points.
    pub added: usize,
    /// Disagreed with what Wenlan holds, or could not be told: left out.
    pub conflicts: usize,
    /// A model call failed, so these were not decided; tried again later.
    pub undecided: usize,
    /// Edited in the tool, so their point was brought up to date.
    #[serde(default)]
    pub updated: usize,
    /// Gone from the tool, so their point was removed.
    #[serde(default)]
    pub removed: usize,
    /// New or edited, waiting for a model to read them.
    #[serde(default)]
    pub waiting: usize,
}

/// Memories one tool keeps on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiMemoryToolCount {
    /// `claude-code` or `codex`.
    pub tool: String,
    pub memories: usize,
}

/// Following, while it is on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiMemoryImportSummary {
    /// When the person turned it on.
    pub at: i64,
    /// When the tools' memory was last read.
    #[serde(default)]
    pub last_at: i64,
    /// What that last pass did.
    pub counts: ImportCounts,
    pub by_tool: Vec<AiMemoryToolCount>,
}

/// `GET /api/import/ai-memory`: what is on this computer to follow, and
/// following, if it is on. Counts only; no memory text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiMemoryImportStatus {
    pub found: Vec<AiMemoryToolCount>,
    /// A model to read the memories with is set up.
    pub ready: bool,
    pub import: Option<AiMemoryImportSummary>,
}

/// `POST /api/import/ai-memory/undo`: following turned off and what it
/// brought in taken back, if it was on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiMemoryImportUndo {
    pub undone: Option<AiMemoryImportSummary>,
}
