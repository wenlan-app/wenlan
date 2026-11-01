// SPDX-License-Identifier: Apache-2.0
//! History as the append-only log (spec section 4): every row of every
//! history-class table becomes one committed entry in
//! `backstage/history/`, in time order, written by [`HistoryLog`]. Reads
//! (`session_reads`, `access_log`, `agent_activity`), whose volume would
//! swamp the main log, go to `backstage/history/reads/`.
//!
//! Each entry carries the spec's fields where the row has them (`page.write`
//! with the page, version and text; `brief.update` with its receipt; the
//! sentence and thing logs with their own op, actor and target), and
//! `data: {"table": <table>, "row": {<column>: <value>, …}}` with every
//! other column as stored, so replaying the log gives back each row
//! exactly. The column moved into `text` is not repeated in `row`.
//!
//! Who made a page version is what the app itself says
//! ([`authorship::version_author`]), with a plain `create` decided by the
//! page's creation kind the way the sentence view decides it. A writer the
//! app does not know (`migration_87` among them) is unknown, which is
//! protected like the person's. A `page_truth_state` row is `page.approve`
//! when the person reviewed it and `page.review_state` otherwise.
//! `data.text_column` names the column that moved into `text`, so the
//! rebuild needs no table list of its own.
//!
//! The log is synced once at the end rather than per entry: the export
//! writes a new folder and can start over, and a real database has up to a
//! million read rows. Seq order is export order (time, then table, then
//! the table's key order); the rebuild orders rows by their own keys.
//!
//! `thing_sessions` is current state and is already in
//! `settings/things.json`.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Map, Value};

use super::settings::cell;
use super::{ExportDirs, Manifest};
use crate::authorship::{self, VersionAuthor};
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::store::history::{
    Actor, ActorKind, HistoryConfig, HistoryLog, NewEntry, Receipt, Target,
};

/// Which log a table's rows go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Log {
    Main,
    Reads,
}

/// One history-class table: its log, the op its entries get (`None` = the
/// row's own `op` column), and the column that holds its time.
pub struct HistoryTable {
    pub table: &'static str,
    pub log: Log,
    pub op: Option<&'static str>,
    pub time: Option<&'static str>,
}

const fn table(
    table: &'static str,
    log: Log,
    op: Option<&'static str>,
    time: Option<&'static str>,
) -> HistoryTable {
    HistoryTable {
        table,
        log,
        op,
        time,
    }
}

pub const HISTORY_TABLES: &[HistoryTable] = &[
    table(
        "page_history",
        Log::Main,
        Some("page.write"),
        Some("created_at"),
    ),
    table("sentence_history", Log::Main, None, Some("ts")),
    table(
        "brief_history",
        Log::Main,
        Some("brief.update"),
        Some("created_at"),
    ),
    table(
        "operation_receipts",
        Log::Main,
        Some("receipt"),
        Some("created_at"),
    ),
    table(
        "presence_receipts",
        Log::Main,
        Some("receipt"),
        Some("created_at"),
    ),
    table(
        "presence_nonces",
        Log::Main,
        Some("receipt.nonce"),
        Some("consumed_at"),
    ),
    table(
        "rejected_memories",
        Log::Main,
        Some("memory.reject"),
        Some("created_at"),
    ),
    table(
        "page_truth_state",
        Log::Main,
        Some("page.approve"),
        Some("updated_at"),
    ),
    table(
        "page_draft_create_requests",
        Log::Main,
        Some("draft.discard"),
        None,
    ),
    table(
        "agent_sessions",
        Log::Main,
        Some("session.record"),
        Some("created_at"),
    ),
    table("enrichment_origin", Log::Main, Some("memory.origin"), None),
    table(
        "chat_reminders",
        Log::Main,
        Some("chat.reminder"),
        Some("ts"),
    ),
    table("thing_history", Log::Main, None, Some("ts")),
    table(
        "session_reads",
        Log::Reads,
        Some("session.read"),
        Some("created_at"),
    ),
    table(
        "agent_activity",
        Log::Reads,
        Some("agent.activity"),
        Some("timestamp"),
    ),
    table(
        "access_log",
        Log::Reads,
        Some("memory.access"),
        Some("accessed_at"),
    ),
];

/// History-class tables written elsewhere, and where.
pub const HISTORY_ELSEWHERE: &[(&str, &str)] = &[("thing_sessions", "settings/things.json")];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryExported {
    pub entries: usize,
    pub reads: usize,
}

pub async fn export_history(
    db: &MemoryDB,
    dirs: &ExportDirs,
    manifest: &mut Manifest,
) -> Result<HistoryExported, WenlanError> {
    let births = db.export_page_births().await?;
    let page_rows = db.export_table("page_history").await?;
    let first_kinds = birth_kinds(&births, page_rows.as_ref());
    let mut main = Vec::new();
    let mut reads = Vec::new();
    for spec in HISTORY_TABLES {
        let Some(copied) = db.export_table(spec.table).await? else {
            continue;
        };
        for (order, values) in copied.rows.iter().enumerate() {
            let row: Map<String, Value> = copied
                .columns
                .iter()
                .cloned()
                .zip(values.iter().map(cell))
                .collect();
            let entry = entry_of(spec, row, &first_kinds);
            let key = (entry.ts.unwrap_or(0), spec.table, order);
            match spec.log {
                Log::Main => main.push((key, entry)),
                Log::Reads => reads.push((key, entry)),
            }
        }
    }
    let done = HistoryExported {
        entries: main.len(),
        reads: reads.len(),
    };
    let root = dirs.backstage().join("history");
    write_log(&root, main)?;
    write_log(&root.join("reads"), reads)?;
    list_files(&dirs.root, &root, manifest)?;
    Ok(done)
}

type Keyed = ((i64, &'static str, usize), NewEntry);

fn write_log(root: &Path, mut entries: Vec<Keyed>) -> Result<(), WenlanError> {
    if entries.is_empty() {
        return Ok(());
    }
    // Time order, so each month's file holds that month; rows with the same
    // time keep their table and row order.
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let config = HistoryConfig {
        sync_each_entry: false,
        ..HistoryConfig::default()
    };
    let (mut log, _) = HistoryLog::open_with(root, config)?;
    for (_, entry) in entries {
        log.write(entry)?;
    }
    log.sync()?;
    drop(log);
    // The lock file is the daemon's, not part of the export.
    let _ = std::fs::remove_file(root.join(".lock"));
    Ok(())
}

/// Every log and blob file, for the manifest.
fn list_files(export_root: &Path, dir: &Path, manifest: &mut Manifest) -> Result<(), WenlanError> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut entries: Vec<_> = read.collect::<Result<_, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            list_files(export_root, &path, manifest)?;
            continue;
        }
        let relative = path
            .strip_prefix(export_root)
            .map_err(|_| WenlanError::Validation("history file outside the export".to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(&path)?;
        manifest.files.push(super::ExportedFile {
            id: relative.clone(),
            file_type: "history".to_string(),
            path: relative,
            sha256: super::sha256(&bytes),
        });
    }
    Ok(())
}

/// Each page's creation kind as the sentence view reads it for a first
/// version recorded as plain `create` (`sentences::birth_kind`).
fn birth_kinds(
    births: &HashMap<String, (String, bool)>,
    page_history: Option<&crate::db::export_reads::ExportTable>,
) -> HashMap<String, String> {
    let mut person_later = std::collections::HashSet::new();
    if let Some(table) = page_history {
        let column = |name: &str| table.columns.iter().position(|c| c == name);
        if let (Some(page), Some(version), Some(by)) =
            (column("page_id"), column("version"), column("edited_by"))
        {
            for row in &table.rows {
                let (
                    libsql::Value::Text(page),
                    libsql::Value::Integer(version),
                    libsql::Value::Text(by),
                ) = (&row[page], &row[version], &row[by])
                else {
                    continue;
                };
                if *version > 1 && authorship::version_author(by, "") == VersionAuthor::Person {
                    person_later.insert(page.clone());
                }
            }
        }
    }
    births
        .iter()
        .map(|(id, (creation_kind, user_edited))| {
            let birth = if creation_kind != "authored" {
                creation_kind.clone()
            } else if !user_edited {
                "agent_authored".to_string()
            } else if person_later.contains(id) {
                "legacy_authored".to_string()
            } else {
                "authored".to_string()
            };
            (id.clone(), birth)
        })
        .collect()
}

fn entry_of(
    spec: &HistoryTable,
    mut row: Map<String, Value>,
    first_kinds: &HashMap<String, String>,
) -> NewEntry {
    let text_of = |row: &Map<String, Value>, column: &str| {
        row.get(column).and_then(Value::as_str).map(str::to_string)
    };
    let ts = spec
        .time
        .and_then(|column| row.get(column))
        .and_then(seconds);
    let reviewed = row.get("human_reviewed").and_then(Value::as_i64) == Some(1);
    let op = match spec.table {
        "page_truth_state" if !reviewed => Some("page.review_state".to_string()),
        _ => spec.op.map(str::to_string),
    }
    .or_else(|| text_of(&row, "op"))
    .unwrap_or_else(|| format!("{}.row", spec.table));
    let mut entry = NewEntry::new(actor_of(spec.table, &row, first_kinds), op);
    entry.ts = Some(ts.unwrap_or(0));
    let text_column = match spec.table {
        "page_history" => {
            entry.target = text_of(&row, "page_id").map(|id| Target::new("page", id));
            entry.version = row.get("version").and_then(Value::as_i64);
            Some("content")
        }
        "sentence_history" => {
            if let (Some(kind), Some(id)) =
                (text_of(&row, "target_type"), text_of(&row, "target_id"))
            {
                entry.target = Some(Target::new(kind, id));
            }
            entry.version = row.get("version").and_then(Value::as_i64);
            entry.why = text_of(&row, "why");
            entry.digest_before = text_of(&row, "digest_before");
            entry.digest_after = text_of(&row, "digest_after");
            Some("text")
        }
        "brief_history" => {
            entry.target = text_of(&row, "space_id").map(|id| Target::new("brief", id));
            entry.version = row.get("brief_version").and_then(Value::as_i64);
            entry.receipt = receipt_of(&row, None);
            None
        }
        "operation_receipts" | "presence_receipts" | "presence_nonces" => {
            entry.receipt = receipt_of(&row, text_of(&row, "request_digest"));
            None
        }
        "rejected_memories" => {
            entry.target = text_of(&row, "id").map(|id| Target::new("memory", id));
            None
        }
        "page_truth_state" | "page_draft_create_requests" => {
            entry.target = text_of(&row, "page_id").map(|id| Target::new("page", id));
            None
        }
        "enrichment_origin" | "access_log" => {
            entry.target = text_of(&row, "source_id").map(|id| Target::new("memory", id));
            None
        }
        "thing_history" => {
            entry.target = text_of(&row, "thing_id").map(|id| Target::new("thing", id));
            None
        }
        "agent_sessions" => {
            entry.target = text_of(&row, "id").map(|id| Target::new("session", id));
            None
        }
        "session_reads" => {
            entry.target = text_of(&row, "session_id").map(|id| Target::new("session", id));
            None
        }
        "chat_reminders" => {
            if let (Some(kind), Some(id)) =
                (text_of(&row, "target_type"), text_of(&row, "target_id"))
            {
                entry.target = Some(Target::new(kind, id));
            }
            None
        }
        _ => None,
    };
    if let Some(column) = text_column {
        match row.remove(column) {
            Some(Value::String(text)) => entry.text = Some(text),
            // Not text (NULL or another type): it stays in the row.
            Some(value) => {
                row.insert(column.to_string(), value);
            }
            None => {}
        }
    }
    let mut data = json!({ "table": spec.table, "row": Value::Object(row) });
    if let (Some(column), Some(_)) = (text_column, &entry.text) {
        data["text_column"] = json!(column);
    }
    entry.data = Some(data);
    entry
}

fn receipt_of(row: &Map<String, Value>, request_digest: Option<String>) -> Option<Receipt> {
    let caller = row.get("caller_id")?.as_str()?;
    let operation = row.get("operation_id")?.as_str()?;
    Some(Receipt {
        caller_id: caller.to_string(),
        operation_id: operation.to_string(),
        request_digest,
        response_digest: None,
    })
}

fn actor_of(table: &str, row: &Map<String, Value>, first_kinds: &HashMap<String, String>) -> Actor {
    let text = |column: &str| row.get(column).and_then(Value::as_str);
    let named = |kind: ActorKind, agent: Option<&str>| Actor {
        agent: agent.map(str::to_string),
        ..Actor::new(kind)
    };
    match table {
        "page_history" => {
            let edited_by = text("edited_by");
            let birth = text("page_id")
                .and_then(|page| first_kinds.get(page))
                .map(String::as_str)
                .unwrap_or("");
            let kind = match authorship::version_author(edited_by.unwrap_or(""), birth) {
                VersionAuthor::Person => ActorKind::Person,
                VersionAuthor::Ai => ActorKind::Ai,
                VersionAuthor::Original => ActorKind::Original,
                VersionAuthor::System => ActorKind::System,
                VersionAuthor::Unknown => ActorKind::Unknown,
            };
            Actor {
                name: edited_by.map(str::to_string),
                session: text("session_id").map(str::to_string),
                ..Actor::new(kind)
            }
        }
        "sentence_history" => text("actor")
            .and_then(|actor| serde_json::from_str::<Actor>(actor).ok())
            .unwrap_or_else(|| Actor::new(ActorKind::Unknown)),
        "thing_history" => match text("actor") {
            Some("person") => Actor::new(ActorKind::Person),
            Some("ai") => Actor::new(ActorKind::Ai),
            _ => Actor::new(ActorKind::Unknown),
        },
        "rejected_memories" => named(ActorKind::Ai, text("source_agent")),
        "agent_activity" => named(ActorKind::Ai, text("agent_name")),
        "page_truth_state" if row.get("human_reviewed").and_then(Value::as_i64) == Some(1) => {
            Actor::new(ActorKind::Person)
        }
        _ => Actor::system(),
    }
}

/// A stored time as Unix seconds: seconds, milliseconds, RFC 3339 or
/// SQLite's `YYYY-MM-DD HH:MM:SS`. `None` when it is none of those; the
/// stored value is in the row either way.
fn seconds(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => {
            let n = number
                .as_i64()
                .or_else(|| number.as_f64().map(|f| f as i64))?;
            Some(if n.abs() > 100_000_000_000 {
                n / 1000
            } else {
                n
            })
        }
        Value::String(text) => chrono::DateTime::parse_from_rfc3339(text)
            .map(|at| at.timestamp())
            .ok()
            .or_else(|| {
                chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
                    .ok()
                    .map(|at| at.and_utc().timestamp())
            }),
        _ => None,
    }
}
