// SPDX-License-Identifier: Apache-2.0
//! The memory palace layout (UX-GRAPH-1..3): where each point sits, the
//! person's own lines, circles and notes, and which AI-added items the person
//! accepted. Stored by the daemon as `<data root>/layout/palace.json`, the
//! file the storage migration names (`layout/palace.json`), so the same shape
//! is the future file.
//!
//! A point keeps its place until the person moves it or asks for a relayout;
//! a relayout is one more versioned write, so it can be reverted.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `format` of `layout/palace.json`.
pub const PALACE_FORMAT: u32 = 1;

/// Who added an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PalaceAuthor {
    Person,
    Ai,
}

/// One point's place. `node_id` (the map key) is the graph node's id: a page,
/// entity or memory id, or the id of a point the person added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PalaceNode {
    pub x: f64,
    pub y: f64,
    /// Text of a point the person (or an AI) added that is not in the graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Absent for graph points placed by layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<PalaceAuthor>,
    /// For an AI-added point: `false` while it is a suggestion (drawn dashed),
    /// `true` once the person accepted it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<bool>,
}

/// A line between two points.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PalaceLine {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<PalaceAuthor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<bool>,
}

/// A circle (group) around some points.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PalaceCircle {
    pub id: String,
    pub members: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<PalaceAuthor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<bool>,
}

/// A note placed on the palace, optionally pinned to a point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PalaceNote {
    pub id: String,
    pub text: String,
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<PalaceAuthor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<bool>,
}

/// Everything the person arranged.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PalaceLayout {
    #[serde(default)]
    pub nodes: BTreeMap<String, PalaceNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<PalaceLine>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub circles: Vec<PalaceCircle>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<PalaceNote>,
    /// Graph points the person deleted from the palace, so they do not come
    /// back when the graph is drawn again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}

/// Why a version was written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PalaceWriteReason {
    /// Drag, add, delete, connect, circle, accept: the person's edit.
    #[default]
    Edit,
    /// A relayout the person asked for.
    Relayout,
    /// Back to an earlier version.
    Revert,
}

/// `layout/palace.json` as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PalaceLayoutFile {
    pub format: u32,
    /// Starts at 1 with the first write and goes up by one per write.
    pub version: u64,
    /// RFC 3339, UTC.
    pub updated_at: String,
    pub updated_by: PalaceAuthor,
    /// The agent behind an AI write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub reason: PalaceWriteReason,
    #[serde(flatten)]
    pub layout: PalaceLayout,
}

/// `GET /api/palace/layout` and the answer to every write.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PalaceLayoutResponse {
    /// 0 while nothing was ever stored (and `layout` is empty).
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<PalaceWriteReason>,
    pub layout: PalaceLayout,
    /// Versions a revert can go back to, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub earlier_versions: Vec<u64>,
}

/// `PUT /api/palace/layout`: replace the layout. Refused with 409 unless
/// `expected_version` is the stored version (0 for the first write).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PutPalaceLayoutRequest {
    pub expected_version: u64,
    pub layout: PalaceLayout,
    /// `relayout` only when the person asked for one.
    #[serde(default)]
    pub reason: PalaceWriteReason,
}

/// `POST /api/palace/layout/revert`: write an earlier version again as the
/// newest one. `to_version` defaults to the version before the current one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevertPalaceLayoutRequest {
    pub expected_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_version: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_is_flat_with_nodes_keyed_by_id() {
        let mut layout = PalaceLayout::default();
        layout.nodes.insert(
            "page_a".into(),
            PalaceNode {
                x: 1.5,
                y: -2.0,
                label: None,
                added_by: None,
                accepted: None,
            },
        );
        let file = PalaceLayoutFile {
            format: PALACE_FORMAT,
            version: 3,
            updated_at: "2026-10-31T00:00:00Z".into(),
            updated_by: PalaceAuthor::Person,
            agent: None,
            reason: PalaceWriteReason::Edit,
            layout,
        };
        assert_eq!(
            serde_json::to_value(&file).unwrap(),
            serde_json::json!({
                "format": 1,
                "version": 3,
                "updated_at": "2026-10-31T00:00:00Z",
                "updated_by": "person",
                "reason": "edit",
                "nodes": {"page_a": {"x": 1.5, "y": -2.0}}
            })
        );
        let back: PalaceLayoutFile =
            serde_json::from_value(serde_json::to_value(&file).unwrap()).unwrap();
        assert_eq!(back, file);
    }
}
