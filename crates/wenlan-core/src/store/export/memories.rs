// SPDX-License-Identifier: Apache-2.0
//! Memories as files: `backstage/memories/YYYY-MM/<id>.md` (spec section 3).
//!
//! - One file per memory (`source_id`), not per chunk. The body is the text
//!   the app shows: a quick capture's `authored_content`, otherwise the
//!   chunks joined with line ends, as `get_memory_detail` joins them.
//! - `kind` and `fields` are written only when someone stated them
//!   (`enrichment_origin`); a classified type or extracted fields are
//!   recomputed by the rebuild.
//! - `origin` keeps whether the text counts as the person's writing.
//! - `about` links each filed entity by its entity file's stem.
//! - Not files: document chunks (their connected folder is their truth) and
//!   rows derived from pages, web pages and summaries, which the rebuild
//!   recomputes. Pending revisions become proposals in a later part. Each is
//!   counted in the manifest.

use std::collections::{BTreeMap, HashMap};

use super::pages::Paths;
use super::{ExportDirs, LeftOut, Manifest, NotFiles};
use crate::db::export_reads::ExportMemoryRow;
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::store::format::memory::{self, MemoryFile};
use crate::store::format::names::file_stem;

/// The memory sources that are memories: agent and person writes, quick
/// captures from before migration 144, and episodes.
const MEMORY_SOURCES: &[&str] = &["memory", "manual", "episode"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoriesExported {
    pub memories: usize,
}

pub async fn export_memories(
    db: &MemoryDB,
    entity_stems: &HashMap<String, String>,
    dirs: &ExportDirs,
    manifest: &mut Manifest,
) -> Result<MemoriesExported, WenlanError> {
    let rows = db.export_memory_rows().await?;
    let mut about: HashMap<String, Vec<String>> = HashMap::new();
    for (memory_id, entity_id) in db.export_memory_entities().await? {
        about.entry(memory_id).or_default().push(entity_id);
    }

    let mut groups: BTreeMap<&str, Vec<&ExportMemoryRow>> = BTreeMap::new();
    for row in &rows {
        groups.entry(row.source_id.as_str()).or_default().push(row);
    }

    let mut done = MemoriesExported::default();
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    let mut paths = Paths::default();
    for (id, chunks) in groups {
        let head = chunks[0];
        if let Some(reason) = not_a_file(head) {
            *skipped.entry(reason).or_default() += 1;
            continue;
        }
        if head.pending_revision {
            manifest.left_out.push(LeftOut {
                id: id.to_string(),
                field: "memory".to_string(),
                why: "a pending revision; it becomes a proposal file in a later part".to_string(),
            });
            continue;
        }
        let mut file = MemoryFile::new(id, text_of(&chunks));
        file.kind = head
            .memory_type
            .clone()
            .filter(|_| head.memory_type_explicit);
        file.space = head.space.clone();
        file.agent = head.source_agent.clone();
        file.created = head.created_at.map(utc);
        file.pinned = head.pinned;
        file.confirmed = head.confirmed;
        file.supersedes = head.supersedes.clone();
        file.supersede_mode = head.supersede_mode.clone();
        file.episode_of = head.episode_of.clone();
        file.event_date = head.event_date.map(utc);
        file.event_end = head.event_end.map(utc);
        file.importance = head.importance;
        file.origin = head.origin_class.clone();
        file.title = head.title.clone().filter(|title| !title.is_empty());
        file.url = head.url.clone();
        file.session = head.session_id.clone();
        file.entity = head.entity_id.clone();
        file.recap = head.is_recap;
        if head.structured_fields_explicit {
            file.fields = head.structured_fields.as_deref().map(fields_value);
        }
        file.source_text = head
            .source_text
            .clone()
            .filter(|source| *source != file.text);
        for entity_id in about.remove(id).unwrap_or_default() {
            match entity_stems.get(&entity_id) {
                Some(stem) => file.about.push(stem.clone()),
                None => manifest.left_out.push(LeftOut {
                    id: id.to_string(),
                    field: format!("about[{entity_id}]"),
                    why: "the memory is filed under an entity that has no entity file".to_string(),
                }),
            }
        }
        let folder = format!("memories/{}", month_of(head.created_at));
        let path = paths.claim(&folder, &file_stem(id), id);
        manifest.files.push(dirs.write(
            &format!("backstage/{path}"),
            id,
            "memory",
            &memory::render(&file),
        )?);
        done.memories += 1;
    }
    for (what, count) in skipped {
        manifest.not_files.push(NotFiles {
            what,
            count,
            why: "derived or owned by a connected folder; the rebuild recomputes it".to_string(),
        });
    }
    Ok(done)
}

/// Why a memory is not written as a file, if it is not.
fn not_a_file(head: &ExportMemoryRow) -> Option<String> {
    if !MEMORY_SOURCES.contains(&head.source.as_str()) {
        return Some(format!("memories with source {}", head.source));
    }
    let ingest = head.origin_class.as_deref() == Some("document_ingest")
        || head
            .source_agent
            .as_deref()
            .is_some_and(crate::origin::is_document_ingest_source_agent);
    ingest.then(|| "document chunks".to_string())
}

/// The text the app shows for a memory.
fn text_of(chunks: &[&ExportMemoryRow]) -> String {
    if let Some(authored) = chunks
        .iter()
        .find_map(|chunk| chunk.authored_content.clone())
    {
        return authored;
    }
    chunks
        .iter()
        .map(|chunk| chunk.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Stated fields as YAML; text that is not JSON is kept as text.
fn fields_value(raw: &str) -> serde_yaml::Value {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|json| serde_yaml::to_value(json).ok())
        .unwrap_or_else(|| serde_yaml::Value::String(raw.to_string()))
}

/// `2026-11-01T10:00:00Z`, or the number as written when it is not a time.
pub(crate) fn utc(seconds: i64) -> String {
    chrono::DateTime::from_timestamp(seconds, 0)
        .map(|at| at.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| seconds.to_string())
}

fn month_of(created_at: Option<i64>) -> String {
    created_at
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map(|at| at.format("%Y-%m").to_string())
        .unwrap_or_else(|| "undated".to_string())
}
