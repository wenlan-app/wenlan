// SPDX-License-Identifier: Apache-2.0
//! JSON Schema description strings for the `memory_type` parameter.
//!
//! The canonical type set is owned by [`crate::sources::MemoryType`] and
//! returned by [`MemoryType::all_values`]. This module exposes the prose
//! description strings that downstream tool schemas (MCP `capture`,
//! MCP `recall`, future MCP/Tauri schemas) advertise to clients.
//!
//! The drift tests below iterate over the canonical enum and assert each
//! description string lists every variant. Adding a variant to
//! [`crate::sources::MemoryType`] but forgetting to extend the description
//! prose fails CI here — not silently in production.

/// JSON Schema `description` for the `memory_type` parameter on memory-write
/// tools (e.g. MCP `capture`). Lists only the precise types: the caller sets
/// one on every capture, because nothing may classify the memory later when
/// Steep runs on demand, and the broad "profile" / "knowledge" shorthands
/// need that later classification.
pub const MEMORY_TYPE_CAPTURE_DESCRIPTION: &str =
    "One of \"identity\", \"preference\", \"decision\", \"lesson\", \"gotcha\", \"fact\". \
     Set it on every capture; it is kept as given.";

/// JSON Schema `description` for the `memory_type` parameter on memory-read
/// filter tools (e.g. MCP `recall`, `list_pending`). Same vocabulary as
/// capture, framed as a filter.
pub const MEMORY_TYPE_FILTER_DESCRIPTION: &str =
    "Filter by type. Two-level filter: \"profile\" (user-facing) or \
     \"knowledge\" (world-facing), or precise: \"identity\", \"preference\", \
     \"decision\", \"lesson\", \"gotcha\", \"fact\".";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::MemoryType;

    #[test]
    fn capture_description_lists_every_canonical_type() {
        for ty in MemoryType::all_values() {
            let needle = format!("\"{ty}\"");
            assert!(
                MEMORY_TYPE_CAPTURE_DESCRIPTION.contains(&needle),
                "MEMORY_TYPE_CAPTURE_DESCRIPTION missing \"{ty}\""
            );
        }
    }

    #[test]
    fn filter_description_lists_every_canonical_type() {
        for ty in MemoryType::all_values() {
            let needle = format!("\"{ty}\"");
            assert!(
                MEMORY_TYPE_FILTER_DESCRIPTION.contains(&needle),
                "MEMORY_TYPE_FILTER_DESCRIPTION missing \"{ty}\""
            );
        }
    }

    /// "goal" is a legacy alias folded to Identity by `MemoryType::FromStr`.
    /// It must NOT be advertised in any description surface, or clients will
    /// believe they can store with `memory_type = "goal"` and get an
    /// unexpected fold.
    #[test]
    fn descriptions_omit_legacy_goal() {
        for desc in [
            MEMORY_TYPE_CAPTURE_DESCRIPTION,
            MEMORY_TYPE_FILTER_DESCRIPTION,
        ] {
            assert!(
                !contains_word(desc, "goal"),
                "description must not advertise legacy \"goal\": {desc}"
            );
        }
    }

    /// Word-boundary contains: returns true iff `needle` appears as a
    /// standalone alphanumeric token (not a substring of a longer word).
    /// Used by drift tests to ensure "goal" rejection doesn't false-match
    /// on "goals" (plural English noun) elsewhere in prose.
    pub(crate) fn contains_word(haystack: &str, needle: &str) -> bool {
        haystack
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .any(|tok| tok == needle)
    }

    #[test]
    fn contains_word_rejects_partial_matches() {
        assert!(contains_word("goal", "goal"));
        assert!(contains_word("/ goal /", "goal"));
        assert!(contains_word("goal,", "goal"));
        assert!(contains_word("(goal)", "goal"));
        assert!(!contains_word("goals", "goal"));
        assert!(!contains_word("their goals here", "goal"));
        assert!(!contains_word("subgoal", "goal"));
    }
}
