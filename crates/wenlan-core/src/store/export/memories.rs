// SPDX-License-Identifier: Apache-2.0
//! Memories as files: `backstage/memories/YYYY-MM/<id>.md` (spec section 3).
//!
//! - One file per memory (`source`, `source_id`), not per chunk; the app
//!   reads a memory by both, and an episode row shares its parent's
//!   `source_id`. The body is the text
//!   the app shows: a quick capture's `authored_content`, otherwise the
//!   chunks joined with line ends, as `get_memory_detail` joins them.
//! - `kind` and `fields` are written only when someone stated them
//!   (`enrichment_origin`); a classified type or extracted fields are
//!   recomputed by the rebuild.
//! - `origin` keeps whether the text counts as the person's writing.
//! - `about` links each filed entity by its entity file's stem.
//! - An imported chat (`source_id` `import_<vendor>_<conversation id>`)
//!   becomes `backstage/originals/<vendor>/<conversation id>.md` instead,
//!   one message per chunk in order. Import never stored who said each
//!   message, so every heading reads `message`.
//! - A saved web page (`source` `webpage`, id = its URL) becomes
//!   `backstage/originals/web/<url stem>.md`, one `page` message per chunk:
//!   the rebuild cannot fetch it again.
//! - Wenlan's keys the memory file has no field for ride in its other keys:
//!   `source` (when not `memory`), `modified`, `cue` (retrieval cue) and
//!   `space_rejected`. An original carries the same person decisions (pin,
//!   confirm, entity, supersedes, stated kind and fields, `about`, …) there.
//! - Not files: document chunks (their connected folder is their truth) and
//!   rows derived from pages, summaries and episodes, which the rebuild
//!   recomputes. Pending revisions become proposals in a later part. Each is
//!   counted in the manifest. A source this export does not know is listed
//!   as left out, never counted as derived.

use std::collections::{BTreeMap, HashMap};

use super::pages::Paths;
use super::{ExportDirs, LeftOut, Manifest, NotFiles};
use crate::chat_import::types::Vendor;
use crate::db::export_reads::ExportMemoryRow;
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::store::format::memory::{self, MemoryFile};
use crate::store::format::names::file_stem;
use crate::store::format::original::{self, Message, OriginalFile};

/// The memory sources that are memories: agent and person writes, quick
/// captures from before migration 144, and kept clipboard captures.
const MEMORY_SOURCES: &[&str] = &["memory", "manual", "clipboard"];

/// Rows the rebuild recomputes: episodes (a verbatim slice of their parent),
/// page and summary rows, and connected-folder documents.
const DERIVED_SOURCES: &[&str] = &["episode", "page", "summary", "local_files"];

/// A person's saved web page.
const WEB_SOURCE: &str = "webpage";

/// A document from the file watcher of older versions: an original, kept
/// as written, since the folder it came from may be gone.
const FILE_SOURCE: &str = "file";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoriesExported {
    pub memories: usize,
    pub originals: usize,
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

    let mut groups: BTreeMap<(&str, &str), Vec<&ExportMemoryRow>> = BTreeMap::new();
    for row in &rows {
        groups
            .entry((row.source_id.as_str(), row.source.as_str()))
            .or_default()
            .push(row);
    }

    let mut done = MemoriesExported::default();
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    let mut paths = Paths::default();
    for ((id, source), chunks) in groups {
        let head = chunks[0];
        match class_of(head) {
            Class::Memory | Class::Original => {}
            Class::Derived(reason) => {
                *skipped.entry(reason).or_default() += 1;
                continue;
            }
            Class::Unknown => {
                manifest.left_out.push(LeftOut {
                    id: id.to_string(),
                    field: format!("memory with source {source}"),
                    why: "a source the export does not know; compare by hand".to_string(),
                });
                continue;
            }
        }
        if head.pending_revision {
            manifest.left_out.push(LeftOut {
                id: id.to_string(),
                field: "memory".to_string(),
                why: "a pending revision; it becomes a proposal file in a later part".to_string(),
            });
            continue;
        }
        let original = if source == WEB_SOURCE {
            Some(("web", id, "page"))
        } else if source == FILE_SOURCE {
            Some(("file", id, "document"))
        } else {
            imported_chat(id).map(|(vendor, external_id)| (vendor, external_id, "message"))
        };
        if let Some((vendor, external_id, role)) = original {
            let path = paths.claim(&format!("originals/{vendor}"), &file_stem(external_id), id);
            let mut file = original_of(id, vendor, external_id, role, &chunks);
            let decisions = memory_file(id, head, &chunks, &mut about, entity_stems, manifest);
            file.other.extend(decisions_of(&decisions));
            let text = original::render(&file);
            check(manifest, id, original::parse(&text).ok() == Some(file));
            manifest
                .files
                .push(dirs.write(&format!("backstage/{path}"), id, "original", &text)?);
            done.originals += 1;
            continue;
        }
        let file = memory_file(id, head, &chunks, &mut about, entity_stems, manifest);
        let folder = format!("memories/{}", month_of(head.created_at));
        let path = paths.claim(&folder, &file_stem(id), id);
        let text = memory::render(&file);
        check(manifest, id, memory::parse(&text).ok() == Some(file));
        manifest
            .files
            .push(dirs.write(&format!("backstage/{path}"), id, "memory", &text)?);
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

/// The memory file for one memory's rows, with what someone stated.
fn memory_file(
    id: &str,
    head: &ExportMemoryRow,
    chunks: &[&ExportMemoryRow],
    about: &mut HashMap<String, Vec<String>>,
    entity_stems: &HashMap<String, String>,
    manifest: &mut Manifest,
) -> MemoryFile {
    let mut file = MemoryFile::new(id, text_of(chunks));
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
    if head.source != "memory" {
        file.other
            .insert("source".into(), head.source.as_str().into());
    }
    if let Some(modified) = head.last_modified {
        file.other.insert("modified".into(), utc(modified).into());
    }
    if let Some(cue) = head.retrieval_cue.as_deref().filter(|cue| !cue.is_empty()) {
        file.other.insert("cue".into(), cue.into());
    }
    if head.space_rejected {
        file.other.insert("space_rejected".into(), true.into());
    }
    file
}

/// The person's decisions on a memory that became an original, as keys.
fn decisions_of(file: &MemoryFile) -> serde_yaml::Mapping {
    let mut keys = serde_yaml::Mapping::new();
    let mut put = |key: &str, value: serde_yaml::Value| {
        keys.insert(key.into(), value);
    };
    let text = |value: &Option<String>| value.as_deref().map(serde_yaml::Value::from);
    for (key, value) in [
        ("kind", text(&file.kind)),
        ("agent", text(&file.agent)),
        ("created", text(&file.created)),
        ("supersedes", text(&file.supersedes)),
        ("supersede_mode", text(&file.supersede_mode)),
        ("episode_of", text(&file.episode_of)),
        ("event_date", text(&file.event_date)),
        ("event_end", text(&file.event_end)),
        ("origin", text(&file.origin)),
        ("entity", text(&file.entity)),
        ("source_text", text(&file.source_text)),
    ] {
        if let Some(value) = value {
            put(key, value);
        }
    }
    if file.pinned {
        put("pinned", true.into());
    }
    if file.confirmed {
        put("confirmed", true.into());
    }
    if file.recap {
        put("recap", true.into());
    }
    if let Some(importance) = file.importance {
        put("importance", importance.into());
    }
    if let Some(fields) = &file.fields {
        put("fields", fields.clone());
    }
    if !file.about.is_empty() {
        put(
            "about",
            serde_yaml::Value::Sequence(
                file.about.iter().map(|stem| stem.as_str().into()).collect(),
            ),
        );
    }
    for (key, value) in &file.other {
        keys.insert(key.clone(), value.clone());
    }
    keys
}

/// `(vendor, conversation id)` of an imported chat's memory id.
fn imported_chat(id: &str) -> Option<(&'static str, &str)> {
    [Vendor::ChatGpt, Vendor::Claude]
        .into_iter()
        .find_map(|vendor| {
            id.strip_prefix(vendor.source_id_prefix())
                .filter(|rest| !rest.is_empty())
                .map(|rest| (vendor.as_str(), rest))
        })
}

fn original_of(
    id: &str,
    vendor: &str,
    external_id: &str,
    role: &str,
    chunks: &[&ExportMemoryRow],
) -> OriginalFile {
    let head = chunks[0];
    let mut other = serde_yaml::Mapping::new();
    if let Some(session) = &head.session_id {
        other.insert("session".into(), session.as_str().into());
    }
    if let Some(space) = &head.space {
        other.insert("space".into(), space.as_str().into());
    }
    OriginalFile {
        id: id.to_string(),
        vendor: vendor.to_string(),
        external_id: external_id.to_string(),
        title: head.title.clone().filter(|title| !title.is_empty()),
        imported: None,
        url: head.url.clone(),
        other,
        messages: chunks
            .iter()
            .map(|chunk| Message {
                role: role.to_string(),
                at: chunk.created_at.map(utc),
                text: chunk.content.clone(),
            })
            .collect(),
    }
}

/// Lists a file whose text does not read back as what was written.
pub(super) fn check(manifest: &mut Manifest, id: &str, reads_back: bool) {
    if !reads_back {
        manifest.left_out.push(LeftOut {
            id: id.to_string(),
            field: "file".to_string(),
            why: "the file does not read back exactly as written; compare by hand".to_string(),
        });
    }
}

enum Class {
    Memory,
    /// A saved web page or watched file, written as an original.
    Original,
    /// Recomputed by the rebuild, counted under this name.
    Derived(String),
    Unknown,
}

fn class_of(head: &ExportMemoryRow) -> Class {
    let source = head.source.as_str();
    let ingest = head.origin_class.as_deref() == Some("document_ingest")
        || head
            .source_agent
            .as_deref()
            .is_some_and(crate::origin::is_document_ingest_source_agent);
    if DERIVED_SOURCES.contains(&source) {
        Class::Derived(format!("memories with source {source}"))
    } else if ingest {
        Class::Derived("document chunks".to_string())
    } else if MEMORY_SOURCES.contains(&source) {
        Class::Memory
    } else if source == WEB_SOURCE || source == FILE_SOURCE {
        Class::Original
    } else {
        Class::Unknown
    }
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
