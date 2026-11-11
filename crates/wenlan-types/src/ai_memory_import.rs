// SPDX-License-Identifier: Apache-2.0
//! Wire shapes of the one-time import of what Claude Code and Codex remember
//! on this computer (`/api/import/ai-memory`).

use serde::{Deserialize, Serialize};

/// How an import ended: where each memory it read went.
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
    /// A model call failed, so these were not decided.
    pub undecided: usize,
}

/// Memories one tool keeps on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiMemoryToolCount {
    /// `claude-code` or `codex`.
    pub tool: String,
    pub memories: usize,
}

/// An import that stands, until it is taken back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiMemoryImportSummary {
    pub at: i64,
    pub counts: ImportCounts,
    pub by_tool: Vec<AiMemoryToolCount>,
}

/// `GET /api/import/ai-memory`: what is on this computer to import, and the
/// import that stands, if any. Counts only; no memory text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiMemoryImportStatus {
    pub found: Vec<AiMemoryToolCount>,
    /// A model to read the memories with is set up.
    pub ready: bool,
    pub import: Option<AiMemoryImportSummary>,
}

/// `POST /api/import/ai-memory/undo`: the import taken back, if one stood.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiMemoryImportUndo {
    pub undone: Option<AiMemoryImportSummary>,
}
