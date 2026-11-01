// SPDX-License-Identifier: Apache-2.0
//! The canvas file, `<page-stem>.canvas` beside the pages (spec section 3):
//! JSON Canvas 1.0, readable by Obsidian, with a `wenlan` key on the nodes
//! Wenlan placed.
//!
//! ```json
//! {"nodes":[{"id":"n1","type":"text","x":0,"y":0,"width":250,"height":60,
//!            "text":"Lisbon","wenlan":{"source":"page_abc@12","ref":"page:page_def","accepted":true}}],
//!  "edges":[]}
//! ```
//!
//! Keys this module does not know (a newer JSON Canvas, an Obsidian plugin)
//! are kept on read and written back. If Obsidian turns out to drop the
//! `wenlan` key when it saves, the same data lives in a sidecar,
//! `layout/canvas/<id>.json`, keyed by node id: [`split_sidecar`] takes it
//! out of the canvas and [`merge_sidecar`] puts it back, reporting nodes
//! whose sidecar entry is missing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::WenlanError;

/// What Wenlan knows about a node it placed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WenlanNode {
    /// The page version the node came from, `page_abc@12`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// What the node stands for: `memory:…`, `entity:…`, `page:…`,
    /// `section:…`.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Whether the person accepted a node an AI tool added.
    #[serde(default)]
    pub accepted: bool,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// Numbers as written: Obsidian writes integers, other tools may not.
    pub x: serde_json::Number,
    pub y: serde_json::Number,
    pub width: serde_json::Number,
    pub height: serde_json::Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wenlan: Option<WenlanNode>,
    /// `text`, `file`, `url`, `label`, `color` and anything else, as written.
    #[serde(flatten)]
    pub rest: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    #[serde(rename = "fromNode")]
    pub from_node: String,
    #[serde(rename = "toNode")]
    pub to_node: String,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanvasFile {
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, serde_json::Value>,
}

/// The sidecar: each node's `wenlan` data by node id.
pub type Sidecar = BTreeMap<String, WenlanNode>;

pub fn parse(text: &str) -> Result<CanvasFile, WenlanError> {
    serde_json::from_str(text)
        .map_err(|error| WenlanError::Validation(format!("not a JSON Canvas file: {error}")))
}

/// Tab-indented, the way Obsidian writes canvases, so a save in Obsidian
/// changes only what changed.
pub fn render(canvas: &CanvasFile) -> String {
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    canvas
        .serialize(&mut serializer)
        .expect("a canvas always serializes");
    String::from_utf8(out).expect("JSON is UTF-8")
}

/// Takes every node's `wenlan` data out of the canvas into a sidecar.
pub fn split_sidecar(canvas: &mut CanvasFile) -> Sidecar {
    canvas
        .nodes
        .iter_mut()
        .filter_map(|node| node.wenlan.take().map(|data| (node.id.clone(), data)))
        .collect()
}

/// Puts sidecar data back on the nodes that lack a `wenlan` key. Returns
/// the sidecar entries whose node is gone (the person deleted it in
/// Obsidian), for `doctor` to report. A `wenlan` key already on a node wins
/// over the sidecar.
pub fn merge_sidecar(canvas: &mut CanvasFile, mut sidecar: Sidecar) -> Vec<String> {
    for node in &mut canvas.nodes {
        if let Some(data) = sidecar.remove(&node.id) {
            if node.wenlan.is_none() {
                node.wenlan = Some(data);
            }
        }
    }
    sidecar.into_keys().collect()
}
