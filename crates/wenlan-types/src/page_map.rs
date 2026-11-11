// SPDX-License-Identifier: Apache-2.0
//! Wire types for the Page Map API (stage 2). See
//! docs/superpowers/plans/2026-07-18-page-map-api-spec.md for the full
//! contract. Mirrors the row shapes in
//! `crates/wenlan-core/src/db/page_map.rs`, adding the read-time computed
//! `ref_state` field the daemon derives at GET time (never stored).

use serde::{Deserialize, Serialize};

/// Whether a node's backing object (memory/entity/page/section) still
/// resolves. Computed at read time by the server; never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefState {
    Live,
    Dangling,
}

/// `page_maps.viewport`, wire shape (stored server-side as a JSON string).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageMapViewport {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

/// A node in the map, as returned to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageMapNode {
    pub id: String,
    /// `None` only for the root.
    pub parent_id: Option<String>,
    pub rank: f64,
    pub ref_kind: String,
    pub ref_id: String,
    /// Map-local display override; `None` = render from the backing object.
    pub label: Option<String>,
    pub status: String,
    pub pinned: bool,
    pub placed: bool,
    pub collapsed: bool,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub ref_state: RefState,
}

/// An edge in the map, as returned to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageMapEdge {
    pub id: String,
    pub from_node: String,
    pub to_node: String,
    pub kind: String,
    pub label: Option<String>,
    pub status: String,
}

/// What an outline node is: one of the page's headings, or an item written
/// under one (a list item, a bold lead, a table's first column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutlineKind {
    Heading,
    Item,
}

/// One node of a page's own outline, read live from the page body on every
/// map read (never stored, never a suggestion). `parent_id: None` hangs it
/// directly under the page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageOutlineNode {
    /// Stable while the page keeps the same shape: the heading path plus the
    /// item's position under it.
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: OutlineKind,
    /// Short text for the box.
    pub label: String,
    /// The whole line, for hover.
    pub text: String,
    /// Heading slug, so a click can jump to that section of the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// Zero-based line in the page body this node was read from, so an edit
    /// made on the map can land on that line. Absent where no single line
    /// stands behind the node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

/// Full map read: `GET /api/pages/{id}/map` and `PUT .../map/layout`, which
/// also returns the whole (now-updated) map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageMapResponse {
    pub page_id: String,
    pub revision: i64,
    pub map_schema: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<PageMapViewport>,
    pub nodes: Vec<PageMapNode>,
    pub edges: Vec<PageMapEdge>,
    /// Server capability: clients may create map-owned ideas independently
    /// of backing notes. Defaults off for older serialized responses.
    #[serde(default)]
    pub independent_ideas: bool,
    /// The page's own headings and items, read live (GET only). Older
    /// daemons omit it; clients then show only stored nodes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<PageOutlineNode>,
    /// The page version `outline` was read from (GET only). An edit made on
    /// the map is applied to that version and refused if the page moved on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_version: Option<i64>,
}

/// `POST /api/pages/{id}/map/nodes`. `ref_kind`/`ref_id` are `Option` on the
/// wire (rather than required strings) so a missing ref can be reported as a
/// 400 by the handler rather than whatever status axum's own JSON-rejection
/// happens to pick for a missing field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMapNodeRequest {
    pub base_revision: i64,
    /// `None` = attach under the map's root (resolved server-side; lets the
    /// very first node on a brand-new map be created without the client
    /// already knowing the root's freshly-minted id).
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub ref_kind: Option<String>,
    #[serde(default)]
    pub ref_id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub rank: f64,
}

/// `PATCH /api/pages/{id}/map/nodes/{node_id}`. `label` is a nested
/// `Option` so a patch can distinguish "don't touch the label" (key absent)
/// from "clear the override back to NULL" (key present, value `null`) —
/// see `deserialize_double_option` below.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchMapNodeRequest {
    pub base_revision: i64,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub label: Option<Option<String>>,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub rank: Option<f64>,
    #[serde(default)]
    pub parent_id: Option<String>,
}

/// `DELETE /api/pages/{id}/map/nodes/{node_id}` (and the edge equivalent
/// below) — a conditional write like every other map mutation, so it still
/// carries `base_revision` even though it has no other fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteMapNodeRequest {
    pub base_revision: i64,
}

/// The shared mutation envelope for node writes: the new revision plus the
/// touched row, so the client can chain edits without a round-trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMutationResponse {
    pub revision: i64,
    pub node: PageMapNode,
}

fn default_edge_kind() -> String {
    "link".to_string()
}

/// `POST /api/pages/{id}/map/edges`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMapEdgeRequest {
    pub base_revision: i64,
    pub from_node: String,
    pub to_node: String,
    #[serde(default = "default_edge_kind")]
    pub kind: String,
    #[serde(default)]
    pub label: Option<String>,
}

/// `PATCH /api/pages/{id}/map/edges/{edge_id}` — accept/dismiss/relabel only
/// (no from/to/kind changes, per the route table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchMapEdgeRequest {
    pub base_revision: i64,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub label: Option<Option<String>>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteMapEdgeRequest {
    pub base_revision: i64,
}

/// The shared mutation envelope for edge writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeMutationResponse {
    pub revision: i64,
    pub edge: PageMapEdge,
}

/// One placed node in a `PUT .../map/layout` write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageMapNodeLayout {
    pub node_id: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub collapsed: bool,
}

/// `PUT /api/pages/{id}/map/layout`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PutPageMapLayoutRequest {
    pub base_revision: i64,
    #[serde(default)]
    pub viewport: Option<PageMapViewport>,
    #[serde(default)]
    pub positions: Vec<PageMapNodeLayout>,
}

/// Distinguishes "field omitted" (`None`, leave unchanged) from "field
/// present with value `null`" (`Some(None)`, clear the override) for a
/// `label` PATCH — serde's derived `Option<Option<T>>` collapses both to
/// `None` by default (a JSON `null` short-circuits at the outer `Option`),
/// so the nested option is deserialized by hand here. This is the same ~3
/// line recipe the `serde_with::rust::double_option` crate ships; hand-
/// rolled so wenlan-types stays serde + serde_json + anyhow only.
fn deserialize_double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Ok(Some(Option::deserialize(deserializer)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_state_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&RefState::Live).unwrap(), "\"live\"");
        assert_eq!(
            serde_json::to_string(&RefState::Dangling).unwrap(),
            "\"dangling\""
        );
    }

    #[test]
    fn double_option_label_distinguishes_absent_null_and_value() {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default, deserialize_with = "deserialize_double_option")]
            label: Option<Option<String>>,
        }

        let absent: Wrapper = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.label, None);

        let cleared: Wrapper = serde_json::from_str(r#"{"label": null}"#).unwrap();
        assert_eq!(cleared.label, Some(None));

        let set: Wrapper = serde_json::from_str(r#"{"label": "custom"}"#).unwrap();
        assert_eq!(set.label, Some(Some("custom".to_string())));
    }

    #[test]
    fn patch_map_node_request_label_field_uses_double_option() {
        let req: PatchMapNodeRequest =
            serde_json::from_str(r#"{"base_revision": 1, "label": null}"#).unwrap();
        assert_eq!(req.label, Some(None));

        let req: PatchMapNodeRequest = serde_json::from_str(r#"{"base_revision": 1}"#).unwrap();
        assert_eq!(req.label, None);
    }

    #[test]
    fn create_map_edge_request_defaults_kind_to_link() {
        let req: CreateMapEdgeRequest =
            serde_json::from_str(r#"{"base_revision": 1, "from_node": "a", "to_node": "b"}"#)
                .unwrap();
        assert_eq!(req.kind, "link");
    }

    #[test]
    fn page_map_response_omits_absent_viewport() {
        let response = PageMapResponse {
            page_id: "p1".to_string(),
            revision: 0,
            map_schema: 1,
            viewport: None,
            nodes: vec![],
            edges: vec![],
            independent_ideas: false,
            outline: vec![],
            page_version: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("viewport"));
    }

    #[test]
    fn page_map_response_defaults_independent_ideas_off_when_reading_older_payloads() {
        let response: PageMapResponse = serde_json::from_str(
            r#"{"page_id":"p1","revision":0,"map_schema":1,"nodes":[],"edges":[]}"#,
        )
        .unwrap();
        assert!(!response.independent_ideas);
    }
}
