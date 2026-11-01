// SPDX-License-Identifier: Apache-2.0
//! Canvases read back into `page_maps`, `page_map_nodes` and
//! `page_map_edges` rows: the inverse of `store::export::canvases`. The
//! visible canvas holds the canvas row and its accepted parts; the hidden
//! one in `proposals/canvas/` holds the rest.

use std::path::Path;

use serde_json::{Map, Value};

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::store::export::Manifest;
use crate::store::format::canvas::{self, Edge, Node};

/// Writes every canvas file the manifest lists; returns how many canvases.
pub async fn rebuild_canvases(
    from: &Path,
    manifest: &Manifest,
    into: &MemoryDB,
    problems: &mut Vec<String>,
) -> Result<usize, WenlanError> {
    let mut maps = Vec::new();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for file in &manifest.files {
        let visible = match file.file_type.as_str() {
            "canvas" => true,
            "canvas_hidden" => false,
            _ => continue,
        };
        let text = std::fs::read_to_string(from.join(&file.path))?;
        let parsed = match canvas::parse(&text) {
            Ok(parsed) => parsed,
            Err(error) => {
                problems.push(format!("{}: {error}", file.path));
                continue;
            }
        };
        let data = parsed.rest.get("wenlan").and_then(Value::as_object);
        let Some(page) = data.and_then(|d| d.get("page")).and_then(Value::as_str) else {
            problems.push(format!("{}: no wenlan.page", file.path));
            continue;
        };
        if visible {
            let data = data.cloned().unwrap_or_default();
            maps.push(vec![
                text_value(Some(page)),
                int_value(data.get("revision")),
                int_value(data.get("map_schema")),
                text_value(data.get("viewport").and_then(Value::as_str)),
                text_value(data.get("generated_at").and_then(Value::as_str)),
                text_value(data.get("created").and_then(Value::as_str)),
                text_value(data.get("updated").and_then(Value::as_str)),
            ]);
        }
        for node in &parsed.nodes {
            nodes.push(node_row(page, node));
        }
        for edge in &parsed.edges {
            edges.push(edge_row(page, edge));
        }
    }
    let names = |columns: &[&str]| columns.iter().map(|c| c.to_string()).collect::<Vec<_>>();
    let count = maps.len();
    into.rebuild_replace_table(
        "page_maps",
        &names(&[
            "page_id",
            "revision",
            "map_schema",
            "viewport",
            "generated_at",
            "created_at",
            "updated_at",
        ]),
        &maps,
    )
    .await?;
    into.rebuild_replace_table(
        "page_map_nodes",
        &names(&[
            "id",
            "page_id",
            "parent_id",
            "rank",
            "ref_kind",
            "ref_id",
            "label",
            "status",
            "pinned",
            "placed",
            "collapsed",
            "x",
            "y",
            "width",
            "height",
            "fingerprint",
            "provenance",
            "created_at",
            "updated_at",
        ]),
        &nodes,
    )
    .await?;
    into.rebuild_replace_table(
        "page_map_edges",
        &names(&[
            "id",
            "page_id",
            "from_node",
            "to_node",
            "kind",
            "label",
            "status",
            "provenance",
            "created_at",
        ]),
        &edges,
    )
    .await?;
    Ok(count)
}

fn node_row(page: &str, node: &Node) -> Vec<libsql::Value> {
    let empty = Map::new();
    let data = node.wenlan.as_ref().map(|w| &w.rest).unwrap_or(&empty);
    let reference = node
        .wenlan
        .as_ref()
        .and_then(|w| w.reference.clone())
        .unwrap_or_default();
    let (ref_kind, ref_id) = reference
        .split_once(':')
        .map(|(kind, id)| (kind.to_string(), id.to_string()))
        .unwrap_or((reference.clone(), String::new()));
    let unset: Vec<&str> = data
        .get("unset")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let coordinate = |name: &str, drawn: &serde_json::Number| {
        for entry in &unset {
            if *entry == name {
                return libsql::Value::Null;
            }
            if let Some(value) = entry
                .strip_prefix(name)
                .and_then(|rest| rest.strip_prefix('='))
            {
                return value
                    .parse::<f64>()
                    .map_or(libsql::Value::Null, libsql::Value::Real);
            }
        }
        libsql::Value::Real(drawn.as_f64().unwrap_or(0.0))
    };
    let flag = |key: &str| {
        libsql::Value::Integer(i64::from(
            data.get(key).and_then(Value::as_bool).unwrap_or(false),
        ))
    };
    let text = |key: &str| text_value(data.get(key).and_then(Value::as_str));
    vec![
        libsql::Value::Text(node.id.clone()),
        libsql::Value::Text(page.to_string()),
        text("parent"),
        libsql::Value::Real(data.get("rank").and_then(Value::as_f64).unwrap_or(0.0)),
        libsql::Value::Text(ref_kind),
        libsql::Value::Text(ref_id),
        text_value(node.rest.get("text").and_then(Value::as_str)),
        text("status"),
        flag("pinned"),
        flag("placed"),
        flag("collapsed"),
        coordinate("x", &node.x),
        coordinate("y", &node.y),
        coordinate("width", &node.width),
        coordinate("height", &node.height),
        libsql::Value::Text(
            data.get("fingerprint")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        text("provenance"),
        text("created"),
        text("updated"),
    ]
}

fn edge_row(page: &str, edge: &Edge) -> Vec<libsql::Value> {
    let empty = Map::new();
    let data = edge
        .rest
        .get("wenlan")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let text = |key: &str| text_value(data.get(key).and_then(Value::as_str));
    vec![
        libsql::Value::Text(edge.id.clone()),
        libsql::Value::Text(page.to_string()),
        libsql::Value::Text(edge.from_node.clone()),
        libsql::Value::Text(edge.to_node.clone()),
        text("kind"),
        text_value(edge.rest.get("label").and_then(Value::as_str)),
        text("status"),
        text("provenance"),
        text("created"),
    ]
}

fn text_value(value: Option<&str>) -> libsql::Value {
    value.map_or(libsql::Value::Null, |text| {
        libsql::Value::Text(text.to_string())
    })
}

fn int_value(value: Option<&Value>) -> libsql::Value {
    value
        .and_then(Value::as_i64)
        .map_or(libsql::Value::Null, libsql::Value::Integer)
}
