// SPDX-License-Identifier: Apache-2.0
//! Canvases (`page_maps`) as JSON Canvas files (spec section 3).
//!
//! - A page's canvas goes beside its page file, `<stem>.canvas`, with the
//!   nodes and lines the person sees (status `active`). A canvas whose page
//!   has no file goes to `backstage/layout/canvas/<page id>.canvas`.
//! - Suggested and dismissed nodes and lines go to
//!   `backstage/proposals/canvas/<page id>.canvas`, so Obsidian never shows
//!   them and nothing is lost.
//! - Each node keeps `"wenlan": {"ref": "<kind>:<id>", "accepted", …}` with
//!   its parent, rank, pin, placed, collapsed, fingerprint, provenance and
//!   times. A position or size the database leaves empty is drawn at a
//!   default and named in `wenlan.unset`, so the rebuild restores it empty;
//!   an infinite one is named as `x=inf`.
//! - The canvas itself keeps its revision, schema, viewport and times under
//!   a top-level `wenlan` key.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Number, Value};

use super::memories::check;
use super::pages::Paths;
use super::{ExportDirs, Manifest};
use crate::db::export_reads::{ExportCanvasEdgeRow, ExportCanvasNodeRow, ExportCanvasRow};
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::store::format::canvas::{self, CanvasFile, Edge, Node, WenlanNode};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CanvasesExported {
    pub canvases: usize,
    pub hidden: usize,
}

/// `page_paths` maps a page id to its exported file, relative to the root.
pub async fn export_canvases(
    db: &MemoryDB,
    page_paths: &HashMap<String, String>,
    dirs: &ExportDirs,
    manifest: &mut Manifest,
) -> Result<CanvasesExported, WenlanError> {
    let (maps, nodes, edges) = db.export_canvases().await?;
    let mut nodes_by_page: HashMap<&str, Vec<&ExportCanvasNodeRow>> = HashMap::new();
    for node in &nodes {
        nodes_by_page.entry(&node.page_id).or_default().push(node);
    }
    let mut edges_by_page: HashMap<&str, Vec<&ExportCanvasEdgeRow>> = HashMap::new();
    for edge in &edges {
        edges_by_page.entry(&edge.page_id).or_default().push(edge);
    }
    let mut done = CanvasesExported::default();
    // Page ids differ by case on a case-sensitive database but not on the
    // file system, so backstage canvas names are claimed like page names.
    let mut names = Paths::default();
    for map in &maps {
        let page_nodes = nodes_by_page
            .remove(map.page_id.as_str())
            .unwrap_or_default();
        let page_edges = edges_by_page
            .remove(map.page_id.as_str())
            .unwrap_or_default();
        let shown: HashSet<&str> = page_nodes
            .iter()
            .filter(|node| node.status == "active")
            .map(|node| node.id.as_str())
            .collect();
        let is_shown = |edge: &ExportCanvasEdgeRow| {
            edge.status == "active"
                && shown.contains(edge.from_node.as_str())
                && shown.contains(edge.to_node.as_str())
        };

        let mut visible = CanvasFile {
            nodes: Vec::new(),
            edges: Vec::new(),
            rest: Map::new(),
        };
        visible.rest.insert("wenlan".to_string(), canvas_data(map));
        let mut hidden = CanvasFile {
            nodes: Vec::new(),
            edges: Vec::new(),
            rest: Map::new(),
        };
        hidden
            .rest
            .insert("wenlan".to_string(), json!({ "page": map.page_id }));
        for node in page_nodes {
            let target = if shown.contains(node.id.as_str()) {
                &mut visible
            } else {
                &mut hidden
            };
            target.nodes.push(node_of(node));
        }
        for edge in page_edges {
            let target = if is_shown(edge) {
                &mut visible
            } else {
                &mut hidden
            };
            target.edges.push(edge_of(edge));
        }

        let path = match page_paths.get(&map.page_id) {
            Some(page) => format!("{}.canvas", page.strip_suffix(".md").unwrap_or(page)),
            None => format!(
                "backstage/{}",
                names.claim_as(
                    "layout/canvas",
                    &file_safe(&map.page_id),
                    &map.page_id,
                    "canvas"
                )
            ),
        };
        write_canvas(dirs, manifest, &path, &map.page_id, "canvas", &visible)?;
        done.canvases += 1;
        if !hidden.nodes.is_empty() || !hidden.edges.is_empty() {
            let path = format!(
                "backstage/{}",
                names.claim_as(
                    "proposals/canvas",
                    &file_safe(&map.page_id),
                    &map.page_id,
                    "canvas"
                )
            );
            write_canvas(
                dirs,
                manifest,
                &path,
                &map.page_id,
                "canvas_hidden",
                &hidden,
            )?;
            done.hidden += 1;
        }
    }
    for (page, rows) in nodes_by_page {
        manifest.left_out.push(super::LeftOut {
            id: page.to_string(),
            field: format!("canvas nodes ({})", rows.len()),
            why: "nodes of a page that has no canvas row".to_string(),
        });
    }
    for (page, rows) in edges_by_page {
        manifest.left_out.push(super::LeftOut {
            id: page.to_string(),
            field: format!("canvas lines ({})", rows.len()),
            why: "lines of a page that has no canvas row".to_string(),
        });
    }
    Ok(done)
}

fn write_canvas(
    dirs: &ExportDirs,
    manifest: &mut Manifest,
    path: &str,
    id: &str,
    file_type: &str,
    file: &CanvasFile,
) -> Result<(), WenlanError> {
    let text = canvas::render(file);
    check(
        manifest,
        id,
        canvas::parse(&text).ok().as_ref() == Some(file),
    );
    manifest.files.push(dirs.write(path, id, file_type, &text)?);
    Ok(())
}

/// A page id as a file name; ids are plain, but a path must never leave its
/// folder.
fn file_safe(id: &str) -> String {
    crate::store::format::names::file_stem(id)
}

fn canvas_data(map: &ExportCanvasRow) -> Value {
    json!({
        "page": map.page_id,
        "revision": map.revision,
        "map_schema": map.map_schema,
        "viewport": map.viewport,
        "generated_at": map.generated_at,
        "created": map.created_at,
        "updated": map.updated_at,
    })
}

/// An integral coordinate as an integer, the way Obsidian writes them.
fn number(value: f64) -> Number {
    if value.fract() == 0.0 && value.abs() < 9.0e15 {
        Number::from(value as i64)
    } else {
        Number::from_f64(value).unwrap_or_else(|| Number::from(0))
    }
}

fn node_of(node: &ExportCanvasNodeRow) -> Node {
    let mut unset = Vec::new();
    // An empty or non-finite value is drawn at the default and named.
    let mut coordinate = |name: &str, value: Option<f64>, default: f64| match value {
        Some(value) if value.is_finite() => number(value),
        Some(value) => {
            unset.push(Value::String(format!("{name}={value}")));
            number(default)
        }
        None => {
            unset.push(Value::String(name.to_string()));
            number(default)
        }
    };
    let x = coordinate("x", node.x, 0.0);
    let y = coordinate("y", node.y, 0.0);
    let width = coordinate("width", node.width, 250.0);
    let height = coordinate("height", node.height, 60.0);

    let mut data = Map::new();
    let mut put = |key: &str, value: Value| {
        if !value.is_null() {
            data.insert(key.to_string(), value);
        }
    };
    put("status", json!(node.status));
    put("parent", json!(node.parent_id));
    put("rank", Value::Number(number(node.rank)));
    put("pinned", json!(node.pinned));
    put("placed", json!(node.placed));
    put("collapsed", json!(node.collapsed));
    put("fingerprint", json!(node.fingerprint));
    put("provenance", json!(node.provenance));
    put("created", json!(node.created_at));
    put("updated", json!(node.updated_at));
    if !unset.is_empty() {
        data.insert("unset".to_string(), Value::Array(unset));
    }

    let mut rest = Map::new();
    if let Some(label) = &node.label {
        rest.insert("text".to_string(), json!(label));
    }
    Node {
        id: node.id.clone(),
        kind: "text".to_string(),
        x,
        y,
        width,
        height,
        wenlan: Some(WenlanNode {
            source: None,
            reference: Some(format!("{}:{}", node.ref_kind, node.ref_id)),
            accepted: node.status == "active",
            rest: data,
        }),
        rest,
    }
}

fn edge_of(edge: &ExportCanvasEdgeRow) -> Edge {
    let mut rest = Map::new();
    if let Some(label) = &edge.label {
        rest.insert("label".to_string(), json!(label));
    }
    let mut data = Map::new();
    data.insert("kind".to_string(), json!(edge.kind));
    data.insert("status".to_string(), json!(edge.status));
    if let Some(provenance) = &edge.provenance {
        data.insert("provenance".to_string(), json!(provenance));
    }
    if let Some(created) = &edge.created_at {
        data.insert("created".to_string(), json!(created));
    }
    rest.insert("wenlan".to_string(), Value::Object(data));
    Edge {
        id: edge.id.clone(),
        from_node: edge.from_node.clone(),
        to_node: edge.to_node.clone(),
        rest,
    }
}
