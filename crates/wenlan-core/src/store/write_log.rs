// SPDX-License-Identifier: Apache-2.0
//! The write log: what makes the files the truth and the database only an
//! index (migration spec, sections 4 and 6).
//!
//! Every change to a table the files hold ([`TableClass::File`] and
//! [`TableClass::History`]) is appended to `<data root>/files-mirror/log/`
//! and flushed to disk before the database commits it: SQLite's commit hook
//! writes the transaction's rows, then a `commit` line, then syncs the file,
//! and only then lets the database commit. When the log cannot be written,
//! the database write fails. The file copy ([`super::mirror`]) records the
//! last log line it holds ([`POSITION_FILE`]); the database can therefore
//! be deleted at any moment and built again from the copy plus the lines
//! after that position ([`super::restore`]), losing nothing.
//!
//! Each line is one JSON object: `{seq, tx, op, table, key, row}` for a row
//! (`op` is `insert`, `update` or `delete`; `key` is the row's primary key
//! before the change, `row` the whole row after it), then `{seq, tx, op:
//! "commit", schema, ts}`. A transaction without its `commit` line, or with
//! an `abort` line after it, is ignored. Vectors are left out: they are
//! worked out again from the text. Lines are kept until a file copy holds
//! them ([`WriteLog::prune_through`]).
//!
//! How the rows are seen: SQLite calls a pre-update hook for every row a
//! statement changes, triggers and foreign key actions included, so the log
//! is a physical record and a replay ([`crate::db::write_log_replay`]) runs
//! with triggers set aside. The hooks are installed on every connection to
//! a registered database file through an auto extension ([`register`]),
//! since libsql does not hand out its connection handle.
//!
//! [`TableClass::File`]: crate::db::table_classes::TableClass::File
//! [`TableClass::History`]: crate::db::table_classes::TableClass::History

use std::collections::{BTreeSet, HashMap};
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, RwLock};

use libsql::ffi;
use libsql::Value;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value as Json};

use crate::error::WenlanError;

/// The folder under the mirror folder.
pub const LOG_DIR: &str = "log";
/// Written into a file copy: the last log line it holds.
pub const POSITION_FILE: &str = "log-position.json";
const EPOCH_FILE: &str = "epoch";
/// The last log line the database is known to hold: written once the
/// connection runs its next statement after a commit (a commit that fails
/// rolls back inside its own statement), best effort and not synced, read by [`crate::db::write_log_replay::settle`].
const APPLIED_FILE: &str = "applied";
const SEGMENT_SUFFIX: &str = ".jsonl";
/// A new segment starts once the current one is this large.
const SEGMENT_BYTES: u64 = 64 * 1024 * 1024;

/// Where a file copy stands in the log. `epoch` names one log: a log folder
/// that was deleted and started again has a new one, and a position from
/// the old one then means nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogPosition {
    pub epoch: String,
    pub seq: u64,
}

/// The position recorded in the file copy `files`, if any.
pub fn read_position(files: &Path) -> Option<LogPosition> {
    let text = std::fs::read_to_string(files.join(POSITION_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Records `position` in the file copy `files` (written beside, then
/// renamed over).
pub fn write_position(files: &Path, position: &LogPosition) -> Result<(), WenlanError> {
    write_json(&files.join(POSITION_FILE), position)
}

fn write_json(path: &Path, position: &LogPosition) -> Result<(), WenlanError> {
    let json = serde_json::to_string(position)
        .map_err(|error| WenlanError::Validation(format!("log position: {error}")))?;
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    std::fs::write(&temp, json + "\n")?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

/// The last line the database at the log in `dir` is known to hold.
pub fn read_applied(dir: &Path) -> Option<LogPosition> {
    let text = std::fs::read_to_string(dir.join(APPLIED_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Records that the database holds every line up to `position`.
pub fn write_applied(dir: &Path, position: &LogPosition) -> Result<(), WenlanError> {
    write_json(&dir.join(APPLIED_FILE), position)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowOp {
    Insert,
    Update,
    Delete,
}

impl RowOp {
    fn name(self) -> &'static str {
        match self {
            RowOp::Insert => "insert",
            RowOp::Update => "update",
            RowOp::Delete => "delete",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "insert" => Some(RowOp::Insert),
            "update" => Some(RowOp::Update),
            "delete" => Some(RowOp::Delete),
            _ => None,
        }
    }
}

/// One row change.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub table: String,
    pub op: RowOp,
    /// The key columns and their values before the change (update and
    /// delete): the primary key, or every logged column when the table has
    /// none.
    pub key: Vec<(String, Value)>,
    /// The row after the change (insert and update), vectors left out.
    pub row: Vec<(String, Value)>,
}

/// One committed transaction read back from the log.
#[derive(Debug, Clone, PartialEq)]
pub struct Tx {
    pub tx: u64,
    /// The seq of its `commit` line.
    pub seq: u64,
    /// The schema version of the build that wrote it.
    pub schema: i64,
    pub changes: Vec<Change>,
}

/// What the hooks need to know about a logged table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    /// Every column as SQLite numbers them in a row.
    pub columns: Vec<String>,
    /// Columns left out of the log (vectors and generated columns).
    pub skip: Vec<bool>,
    /// Positions of the primary key columns; empty when there is none.
    pub key: Vec<usize>,
}

impl Shape {
    fn key_positions(&self) -> Vec<usize> {
        if self.key.is_empty() {
            (0..self.columns.len()).filter(|i| !self.skip[*i]).collect()
        } else {
            self.key.clone()
        }
    }
}

struct Appender {
    file: Option<File>,
    segment_bytes: u64,
    next_seq: u64,
    /// Seq of the last `commit` line written.
    last_commit: u64,
}

/// The log of one database. Open it with [`WriteLog::open`] before the
/// database opens, [`register`] it for the database file, and arm it with
/// the table shapes once migrations are done ([`WriteLog::arm`]).
pub struct WriteLog {
    dir: PathBuf,
    epoch: String,
    armed: AtomicBool,
    schema: AtomicI64,
    shapes: RwLock<HashMap<String, Shape>>,
    appender: Mutex<Appender>,
    /// Seq of the last `commit` line whose database commit finished.
    applied: AtomicU64,
}

fn io(context: &str, error: std::io::Error) -> WenlanError {
    WenlanError::Validation(format!("write log {context}: {error}"))
}

/// Flushes `file` to disk the way SQLite flushes the database at commit.
/// On macOS that is `fsync`, not the much slower `F_FULLFSYNC` the standard
/// library uses (SQLite uses it only with `PRAGMA fullfsync`).
fn sync(file: &File) -> std::io::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        rustix::fs::fsync(file).map_err(std::io::Error::from)
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        file.sync_data()
    }
}

fn segment_name(start: u64) -> String {
    format!("{start:020}{SEGMENT_SUFFIX}")
}

/// The segments of `dir`, by the seq they start at.
fn segments(dir: &Path) -> Result<Vec<(u64, PathBuf)>, WenlanError> {
    let mut found = Vec::new();
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(error) => return Err(io("read folder", error)),
    };
    for entry in read {
        let entry = entry.map_err(|error| io("read folder", error))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(start) = name
            .strip_suffix(SEGMENT_SUFFIX)
            .and_then(|stem| stem.parse::<u64>().ok())
        {
            found.push((start, entry.path()));
        }
    }
    found.sort();
    Ok(found)
}

/// The epoch of the log in `dir`, if it has one.
pub fn read_epoch(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(EPOCH_FILE)).ok()?;
    let epoch = text.trim().to_string();
    (!epoch.is_empty()).then_some(epoch)
}

impl WriteLog {
    /// Opens the log in `dir`, creating it when missing. A last line that
    /// was cut off (the process stopped mid-write) is removed, with any
    /// rows after the last `commit` line: their transaction never
    /// committed.
    pub fn open(dir: &Path) -> Result<Arc<WriteLog>, WenlanError> {
        std::fs::create_dir_all(dir).map_err(|error| io("create folder", error))?;
        let epoch = match read_epoch(dir) {
            Some(epoch) => epoch,
            None => {
                let epoch = uuid::Uuid::new_v4().simple().to_string();
                let temp = dir.join(format!("{EPOCH_FILE}.tmp"));
                std::fs::write(&temp, format!("{epoch}\n"))
                    .map_err(|error| io("write epoch", error))?;
                std::fs::rename(&temp, dir.join(EPOCH_FILE))
                    .map_err(|error| io("write epoch", error))?;
                epoch
            }
        };
        let mut appender = Appender {
            file: None,
            segment_bytes: 0,
            next_seq: 1,
            last_commit: 0,
        };
        if let Some((start, path)) = segments(dir)?.pop() {
            let bytes = std::fs::read(&path).map_err(|error| io("read segment", error))?;
            let mut keep = 0usize;
            let mut offset = 0usize;
            let mut last_seq = start.saturating_sub(1);
            for line in bytes.split_inclusive(|byte| *byte == b'\n') {
                offset += line.len();
                if !line.ends_with(b"\n") {
                    break;
                }
                let Ok(value) = serde_json::from_slice::<Json>(line) else {
                    continue;
                };
                let seq = value["seq"].as_u64().unwrap_or(0);
                last_seq = last_seq.max(seq);
                if matches!(value["op"].as_str(), Some("commit") | Some("abort")) {
                    keep = offset;
                    if value["op"] == "commit" {
                        appender.last_commit = seq;
                    }
                }
            }
            if keep < bytes.len() {
                let file = OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .map_err(|error| io("open segment", error))?;
                file.set_len(keep as u64)
                    .map_err(|error| io("cut segment", error))?;
                file.sync_all().map_err(|error| io("sync segment", error))?;
            }
            appender.next_seq = last_seq + 1;
            appender.segment_bytes = keep as u64;
            appender.file = Some(
                OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .map_err(|error| io("open segment", error))?,
            );
        }
        Ok(Arc::new(WriteLog {
            dir: dir.to_path_buf(),
            epoch,
            armed: AtomicBool::new(false),
            schema: AtomicI64::new(0),
            shapes: RwLock::new(HashMap::new()),
            appender: Mutex::new(appender),
            applied: AtomicU64::new(0),
        }))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    /// Starts logging: from now on every change to a table in `shapes` is
    /// written here before it commits. `schema` is the database's schema
    /// version, written on each `commit` line.
    /// Call it once the database holds every line ([`crate::db::write_log_replay::settle`]
    /// ran).
    pub fn arm(&self, shapes: HashMap<String, Shape>, schema: i64) {
        *self.shapes.write().unwrap_or_else(|e| e.into_inner()) = shapes;
        self.schema.store(schema, Ordering::SeqCst);
        let last = self
            .appender
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last_commit;
        self.mark_applied(last);
        self.armed.store(true, Ordering::SeqCst);
    }

    /// The database finished committing the transaction whose `commit`
    /// line is `commit`.
    fn mark_applied(&self, commit: u64) {
        let seq = self.applied.fetch_max(commit, Ordering::SeqCst).max(commit);
        let position = LogPosition {
            epoch: self.epoch.clone(),
            seq,
        };
        if let Err(error) = write_applied(&self.dir, &position) {
            log::warn!("[write_log] applied position not recorded: {error}");
        }
    }

    /// The database refused the transaction `tx` after its lines were
    /// written: an `abort` line takes it back.
    fn abort(&self, tx: u64) {
        let mut appender = self.appender.lock().unwrap_or_else(|e| e.into_inner());
        let seq = appender.next_seq;
        appender.next_seq += 1;
        let line = json!({"seq": seq, "tx": tx, "op": "abort"}).to_string() + "\n";
        let written = match appender.file.as_mut() {
            Some(file) => file.write_all(line.as_bytes()).and_then(|()| sync(file)),
            None => Err(std::io::Error::other("no segment open")),
        };
        match written {
            Ok(()) => appender.segment_bytes += line.len() as u64,
            Err(error) => log::error!("[write_log] could not take back transaction {tx}: {error}"),
        }
    }

    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::SeqCst)
    }

    /// The last line whose database commit finished: every transaction up
    /// to it is in the database. A snapshot read after this holds them.
    pub fn position(&self) -> LogPosition {
        LogPosition {
            epoch: self.epoch.clone(),
            seq: self.applied.load(Ordering::SeqCst),
        }
    }

    /// Appends `changes` as one transaction and syncs the file. Returns the
    /// transaction (the seq of its first line) and the seq of its `commit`
    /// line.
    pub fn append(&self, changes: &[Change]) -> Result<(u64, u64), WenlanError> {
        let mut appender = self.appender.lock().unwrap_or_else(|e| e.into_inner());
        if appender.file.is_none() || appender.segment_bytes >= SEGMENT_BYTES {
            let path = self.dir.join(segment_name(appender.next_seq));
            std::fs::create_dir_all(&self.dir).map_err(|error| io("create folder", error))?;
            appender.file = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .map_err(|error| io("open segment", error))?,
            );
            appender.segment_bytes = 0;
        }
        // The seq of its first line: never used twice, whatever a segment
        // that was cut or removed held.
        let tx = appender.next_seq;
        let mut text = String::new();
        let mut seq = appender.next_seq;
        for change in changes {
            let mut line = Map::new();
            line.insert("seq".into(), json!(seq));
            line.insert("tx".into(), json!(tx));
            line.insert("op".into(), json!(change.op.name()));
            line.insert("table".into(), json!(change.table));
            if !change.key.is_empty() {
                line.insert("key".into(), encode_pairs(&change.key));
            }
            if !change.row.is_empty() {
                line.insert("row".into(), encode_pairs(&change.row));
            }
            text.push_str(&Json::Object(line).to_string());
            text.push('\n');
            seq += 1;
        }
        let commit = seq;
        text.push_str(
            &json!({
                "seq": commit,
                "tx": tx,
                "op": "commit",
                "schema": self.schema.load(Ordering::SeqCst),
                "ts": chrono::Utc::now().timestamp_millis(),
            })
            .to_string(),
        );
        text.push('\n');
        appender.next_seq = commit + 1;
        let file = appender.file.as_mut().expect("segment opened above");
        let written = file.write_all(text.as_bytes()).and_then(|()| sync(file));
        if let Err(error) = written {
            // The lines may be on disk in part or whole: mark the
            // transaction as not committed, and start a new segment.
            let abort = json!({"seq": commit + 1, "tx": tx, "op": "abort"}).to_string() + "\n";
            let _ = file.write_all(abort.as_bytes()).and_then(|()| sync(file));
            appender.next_seq = commit + 2;
            appender.file = None;
            return Err(io("append", error));
        }
        appender.segment_bytes += text.len() as u64;
        appender.last_commit = commit;
        Ok((tx, commit))
    }

    /// Removes the segments whose every line is at or before `position`
    /// (a file copy holds them now). The current segment is kept.
    pub fn prune_through(&self, position: &LogPosition) -> Result<usize, WenlanError> {
        if position.epoch != self.epoch {
            return Ok(0);
        }
        let _appender = self.appender.lock().unwrap_or_else(|e| e.into_inner());
        let all = segments(&self.dir)?;
        let mut removed = 0;
        for pair in all.windows(2) {
            let (_, path) = &pair[0];
            let (next_start, _) = pair[1];
            if next_start <= position.seq + 1 {
                std::fs::remove_file(path).map_err(|error| io("remove segment", error))?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// The committed transactions in the log at `dir` after `after` (all of
/// them when `after` is `None` or from another log). Lines that cannot be
/// read are reported in the second value.
pub fn read_after(
    dir: &Path,
    after: Option<&LogPosition>,
) -> Result<(Vec<Tx>, Vec<String>), WenlanError> {
    let mut problems = Vec::new();
    let Some(epoch) = read_epoch(dir) else {
        return Ok((Vec::new(), problems));
    };
    let from = match after {
        Some(position) if position.epoch == epoch => position.seq,
        Some(_) => {
            problems.push(
                "the file copy's log position is from another log: the whole log is replayed"
                    .to_string(),
            );
            0
        }
        None => 0,
    };
    let all = segments(dir)?;
    let mut texts = Vec::new();
    for (i, (_, path)) in all.iter().enumerate() {
        // A segment whose next one starts at or before the position holds
        // nothing after it.
        if all.get(i + 1).is_some_and(|(next, _)| *next <= from + 1) {
            continue;
        }
        texts.push(std::fs::read_to_string(path).map_err(|error| io("read segment", error))?);
    }
    let mut aborted = BTreeSet::new();
    let mut lines = Vec::new();
    for text in &texts {
        for line in text.lines() {
            match serde_json::from_str::<Json>(line) {
                Ok(value) => {
                    if value["op"] == "abort" {
                        if let Some(tx) = value["tx"].as_u64() {
                            aborted.insert(tx);
                        }
                    }
                    lines.push(value);
                }
                Err(_) if line.trim().is_empty() => {}
                Err(error) => problems.push(format!("write log line not read: {error}")),
            }
        }
    }
    let mut open: HashMap<u64, Vec<Change>> = HashMap::new();
    let mut txs = Vec::new();
    for value in lines {
        let Some(tx) = value["tx"].as_u64() else {
            continue;
        };
        match value["op"].as_str() {
            Some("commit") => {
                let changes = open.remove(&tx).unwrap_or_default();
                let seq = value["seq"].as_u64().unwrap_or(0);
                if seq > from && !aborted.contains(&tx) {
                    txs.push(Tx {
                        tx,
                        seq,
                        schema: value["schema"].as_i64().unwrap_or(0),
                        changes,
                    });
                }
            }
            Some("abort") => {}
            Some(op) => match decode_change(op, &value) {
                Some(change) => open.entry(tx).or_default().push(change),
                None => problems.push(format!("write log line {} not read", value["seq"])),
            },
            None => {}
        }
    }
    Ok((txs, problems))
}

/// The last committed transaction in the log at `dir`.
pub fn last_committed(dir: &Path) -> Result<Option<Tx>, WenlanError> {
    let Some((_, path)) = segments(dir)?.pop() else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(&path).map_err(|error| io("read segment", error))?;
    let mut last: Option<Tx> = None;
    let mut open: HashMap<u64, Vec<Change>> = HashMap::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Json>(line) else {
            continue;
        };
        let Some(tx) = value["tx"].as_u64() else {
            continue;
        };
        match value["op"].as_str() {
            Some("commit") => {
                last = Some(Tx {
                    tx,
                    seq: value["seq"].as_u64().unwrap_or(0),
                    schema: value["schema"].as_i64().unwrap_or(0),
                    changes: open.remove(&tx).unwrap_or_default(),
                });
            }
            Some("abort") if last.as_ref().is_some_and(|done| done.tx == tx) => {
                last = None;
            }
            Some("abort") => {}
            Some(op) => {
                if let Some(change) = decode_change(op, &value) {
                    open.entry(tx).or_default().push(change);
                }
            }
            None => {}
        }
    }
    Ok(last)
}

fn encode_value(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Integer(n) => json!(n),
        Value::Real(x) if x.is_finite() => json!(x),
        Value::Real(x) => json!({ "real": x.to_string() }),
        Value::Text(text) => json!(text),
        Value::Blob(bytes) => json!({ "blob": hex::encode(bytes) }),
    }
}

fn decode_value(value: &Json) -> Option<Value> {
    Some(match value {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Integer(i64::from(*b)),
        Json::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Real(n.as_f64()?),
        },
        Json::String(text) => Value::Text(text.clone()),
        Json::Object(map) => {
            if let Some(hex_text) = map.get("blob").and_then(Json::as_str) {
                Value::Blob(hex::decode(hex_text).ok()?)
            } else if let Some(real) = map.get("real").and_then(Json::as_str) {
                Value::Real(real.parse().ok()?)
            } else {
                return None;
            }
        }
        Json::Array(_) => return None,
    })
}

/// Pairs as `[[column, value], ...]`: keeps the column order.
fn encode_pairs(pairs: &[(String, Value)]) -> Json {
    Json::Array(
        pairs
            .iter()
            .map(|(column, value)| json!([column, encode_value(value)]))
            .collect(),
    )
}

fn decode_pairs(value: &Json) -> Option<Vec<(String, Value)>> {
    match value {
        Json::Null => Some(Vec::new()),
        Json::Array(items) => items
            .iter()
            .map(|item| {
                let column = item.get(0)?.as_str()?.to_string();
                Some((column, decode_value(item.get(1)?)?))
            })
            .collect(),
        _ => None,
    }
}

fn decode_change(op: &str, value: &Json) -> Option<Change> {
    Some(Change {
        table: value["table"].as_str()?.to_string(),
        op: RowOp::parse(op)?,
        key: decode_pairs(&value["key"])?,
        row: decode_pairs(&value["row"])?,
    })
}

// ---------------------------------------------------------------------------
// Hooks

extern "C" {
    fn sqlite3_preupdate_hook(
        db: *mut ffi::sqlite3,
        callback: Option<
            unsafe extern "C" fn(
                *mut c_void,
                *mut ffi::sqlite3,
                c_int,
                *const c_char,
                *const c_char,
                i64,
                i64,
            ),
        >,
        arg: *mut c_void,
    ) -> *mut c_void;
    fn sqlite3_preupdate_old(
        db: *mut ffi::sqlite3,
        i: c_int,
        out: *mut *mut ffi::sqlite3_value,
    ) -> c_int;
    fn sqlite3_preupdate_new(
        db: *mut ffi::sqlite3,
        i: c_int,
        out: *mut *mut ffi::sqlite3_value,
    ) -> c_int;
    fn sqlite3_preupdate_count(db: *mut ffi::sqlite3) -> c_int;
}

#[derive(Default)]
struct Pending {
    changes: Vec<Change>,
    /// Open savepoints and how many changes came before each.
    savepoints: Vec<(String, usize)>,
    /// The transaction and `commit` seq written by the commit hook whose
    /// database commit has not been seen to finish yet.
    flushed: Option<(u64, u64)>,
}

/// One connection's hooks.
struct Capture {
    log: Arc<WriteLog>,
    pending: Mutex<Pending>,
}

static REGISTRY: Mutex<Vec<(PathBuf, Arc<WriteLog>)>> = Mutex::new(Vec::new());
static INSTALL: Once = Once::new();

fn same_file(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    let parent = path.parent()?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    Some(parent.canonicalize().ok()?.join(name))
}

/// Logs every write to `db_file` into `log`: the hooks are installed on
/// each connection opened to that file from now on. They record nothing
/// until the log is armed ([`WriteLog::arm`]).
pub fn register(db_file: &Path, log: Arc<WriteLog>) -> Result<(), WenlanError> {
    let key = same_file(db_file).ok_or_else(|| {
        WenlanError::Validation(format!("write log: no folder for {}", db_file.display()))
    })?;
    INSTALL.call_once(|| {
        // libsql sets SQLite's threading mode once, before SQLite starts,
        // and an auto extension starts SQLite: let libsql go first.
        #[allow(deprecated)]
        let _ = libsql::Database::open_in_memory();
        // SAFETY: `on_open` has the signature SQLite calls auto extensions
        // with; it is registered once for the life of the process.
        unsafe {
            ffi::sqlite3_auto_extension(Some(on_open));
        }
    });
    let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    registry.retain(|(path, _)| path != &key);
    registry.push((key, log));
    Ok(())
}

/// Stops installing hooks for `db_file` (connections already open keep
/// theirs).
pub fn unregister(db_file: &Path) {
    if let Some(key) = same_file(db_file) {
        REGISTRY
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(path, _)| path != &key);
    }
}

fn registered(path: &Path) -> Option<Arc<WriteLog>> {
    let key = same_file(path)?;
    REGISTRY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(registered, _)| registered == &key)
        .map(|(_, log)| log.clone())
}

unsafe extern "C" fn on_open(
    db: *mut ffi::sqlite3,
    _error: *mut *const c_char,
    _api: *const ffi::sqlite3_api_routines,
) -> c_int {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let name = ffi::sqlite3_db_filename(db, c"main".as_ptr());
        if name.is_null() {
            return;
        }
        let name = CStr::from_ptr(name).to_string_lossy().to_string();
        if name.is_empty() {
            return;
        }
        let Some(log) = registered(Path::new(&name)) else {
            return;
        };
        let capture = Box::into_raw(Box::new(Capture {
            log,
            pending: Mutex::new(Pending::default()),
        }));
        let arg = capture as *mut c_void;
        // A function nobody calls, so SQLite frees `capture` when the
        // connection closes. When it fails SQLite has freed it already, and
        // no hook may point to it.
        let created = ffi::sqlite3_create_function_v2(
            db,
            c"wenlan_write_log".as_ptr(),
            0,
            ffi::SQLITE_UTF8,
            arg,
            Some(noop_function),
            None,
            None,
            Some(free_capture),
        );
        if created != ffi::SQLITE_OK {
            log::error!("[write_log] hooks not installed on {name}: SQLite error {created}");
            return;
        }
        sqlite3_preupdate_hook(db, Some(on_preupdate), arg);
        ffi::sqlite3_commit_hook(db, Some(on_commit), arg);
        ffi::sqlite3_rollback_hook(db, Some(on_rollback), arg);
        ffi::sqlite3_trace_v2(
            db,
            ffi::SQLITE_TRACE_STMT as c_uint,
            Some(on_statement),
            arg,
        );
    }));
    ffi::SQLITE_OK as c_int
}

unsafe extern "C" fn noop_function(
    context: *mut ffi::sqlite3_context,
    _argc: c_int,
    _argv: *mut *mut ffi::sqlite3_value,
) {
    ffi::sqlite3_result_null(context);
}

unsafe extern "C" fn free_capture(arg: *mut c_void) {
    if !arg.is_null() {
        drop(Box::from_raw(arg as *mut Capture));
    }
}

unsafe fn read_value(value: *mut ffi::sqlite3_value) -> Value {
    match ffi::sqlite3_value_type(value) {
        ffi::SQLITE_INTEGER => Value::Integer(ffi::sqlite3_value_int64(value)),
        ffi::SQLITE_FLOAT => Value::Real(ffi::sqlite3_value_double(value)),
        ffi::SQLITE_TEXT => {
            let text = ffi::sqlite3_value_text(value);
            let len = ffi::sqlite3_value_bytes(value).max(0) as usize;
            if text.is_null() || len == 0 {
                Value::Text(String::new())
            } else {
                let bytes = std::slice::from_raw_parts(text, len);
                Value::Text(String::from_utf8_lossy(bytes).into_owned())
            }
        }
        ffi::SQLITE_BLOB => {
            let blob = ffi::sqlite3_value_blob(value) as *const u8;
            let len = ffi::sqlite3_value_bytes(value).max(0) as usize;
            if blob.is_null() || len == 0 {
                Value::Blob(Vec::new())
            } else {
                Value::Blob(std::slice::from_raw_parts(blob, len).to_vec())
            }
        }
        _ => Value::Null,
    }
}

type Reader = unsafe extern "C" fn(*mut ffi::sqlite3, c_int, *mut *mut ffi::sqlite3_value) -> c_int;

unsafe fn read_row(db: *mut ffi::sqlite3, count: usize, reader: Reader) -> Vec<Value> {
    (0..count)
        .map(|i| {
            let mut value = std::ptr::null_mut();
            if reader(db, i as c_int, &mut value) == ffi::SQLITE_OK as c_int && !value.is_null() {
                read_value(value)
            } else {
                Value::Null
            }
        })
        .collect()
}

unsafe extern "C" fn on_preupdate(
    arg: *mut c_void,
    db: *mut ffi::sqlite3,
    op: c_int,
    database: *const c_char,
    table: *const c_char,
    _old_rowid: i64,
    _new_rowid: i64,
) {
    let capture = &*(arg as *const Capture);
    if !capture.log.is_armed() || database.is_null() || table.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if CStr::from_ptr(database).to_bytes() != b"main" {
            return;
        }
        let table = CStr::from_ptr(table).to_string_lossy();
        let shapes = capture.log.shapes.read().unwrap_or_else(|e| e.into_inner());
        let Some(shape) = shapes.get(table.as_ref()) else {
            return;
        };
        let count = sqlite3_preupdate_count(db).max(0) as usize;
        // A table whose columns changed since the log was armed: logged by
        // position, and a replay maps the positions to its columns.
        let fallback;
        let shape = if count == shape.columns.len() {
            shape
        } else {
            fallback = Shape {
                columns: (0..count).map(|i| format!("#{i}")).collect(),
                skip: vec![false; count],
                key: Vec::new(),
            };
            &fallback
        };
        let op = match op {
            ffi::SQLITE_INSERT => RowOp::Insert,
            ffi::SQLITE_UPDATE => RowOp::Update,
            ffi::SQLITE_DELETE => RowOp::Delete,
            _ => return,
        };
        let old = (op != RowOp::Insert).then(|| read_row(db, count, sqlite3_preupdate_old));
        let new = (op != RowOp::Delete).then(|| read_row(db, count, sqlite3_preupdate_new));
        if let (Some(old), Some(new)) = (&old, &new) {
            let same = (0..count).all(|i| shape.skip[i] || old[i] == new[i]);
            if same {
                return;
            }
        }
        let key = old
            .as_ref()
            .map(|old| {
                shape
                    .key_positions()
                    .into_iter()
                    .map(|i| (shape.columns[i].clone(), old[i].clone()))
                    .collect()
            })
            .unwrap_or_default();
        let row = new
            .map(|new| {
                new.into_iter()
                    .enumerate()
                    .filter(|(i, _)| !shape.skip[*i])
                    .map(|(i, value)| (shape.columns[i].clone(), value))
                    .collect()
            })
            .unwrap_or_default();
        let change = Change {
            table: table.into_owned(),
            op,
            key,
            row,
        };
        capture
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .changes
            .push(change);
    }));
}

unsafe extern "C" fn on_commit(arg: *mut c_void) -> c_int {
    let capture = &*(arg as *const Capture);
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut pending = capture.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.savepoints.clear();
        // A new commit: the one before it finished.
        if let Some((_, commit)) = pending.flushed.take() {
            capture.log.mark_applied(commit);
        }
        let changes = std::mem::take(&mut pending.changes);
        if changes.is_empty() {
            return 0;
        }
        match capture.log.append(&changes) {
            Ok(flushed) => {
                pending.flushed = Some(flushed);
                0
            }
            Err(error) => {
                log::error!(
                    "[write_log] could not write {} changes to {}: {error}; the database \
                     write is refused",
                    changes.len(),
                    capture.log.dir.display()
                );
                1
            }
        }
    }));
    result.unwrap_or(1)
}

/// A rollback, including one right after the commit hook (the database
/// commit then failed): its logged transaction is taken back.
unsafe extern "C" fn on_rollback(arg: *mut c_void) {
    let capture = &*(arg as *const Capture);
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut pending = capture.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.changes.clear();
        pending.savepoints.clear();
        if let Some((tx, _)) = pending.flushed.take() {
            capture.log.abort(tx);
        }
    }));
}

/// Savepoints: a `ROLLBACK TO` takes back the changes made since its
/// savepoint, which SQLite reports to no hook.
unsafe extern "C" fn on_statement(
    kind: c_uint,
    arg: *mut c_void,
    _statement: *mut c_void,
    sql: *mut c_void,
) -> c_int {
    if kind != ffi::SQLITE_TRACE_STMT as c_uint || sql.is_null() {
        return 0;
    }
    let capture = &*(arg as *const Capture);
    let sql = CStr::from_ptr(sql as *const c_char).to_bytes();
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut pending = capture.pending.lock().unwrap_or_else(|e| e.into_inner());
        // A statement after a commit: that commit finished (a failed one
        // rolls back inside its own statement). libsql's WAL reports no
        // frames to a WAL hook, so this is where a commit is seen to land.
        if let Some((_, commit)) = pending.flushed.take() {
            capture.log.mark_applied(commit);
        }
        if let Some(step) = savepoint_step(sql) {
            apply_savepoint_step(&mut pending, step);
        }
    }));
    0
}

#[derive(Debug, PartialEq, Eq)]
enum SavepointStep {
    Open(String),
    RollbackTo(String),
    Release(String),
}

fn savepoint_name(rest: &str) -> String {
    let rest = rest.trim().trim_end_matches(';').trim();
    let rest = rest
        .strip_prefix("SAVEPOINT ")
        .or_else(|| rest.strip_prefix("savepoint "))
        .unwrap_or(rest)
        .trim();
    rest.trim_matches(|c| c == '"' || c == '\'' || c == '`' || c == '[' || c == ']')
        .to_ascii_lowercase()
}

fn savepoint_step(sql: &[u8]) -> Option<SavepointStep> {
    let head = sql.get(..sql.len().min(256))?;
    let text = std::str::from_utf8(head).ok()?.trim_start();
    let upper = text.to_ascii_uppercase();
    if let Some(rest) = upper.strip_prefix("SAVEPOINT ") {
        return Some(SavepointStep::Open(savepoint_name(rest)));
    }
    if let Some(rest) = upper.strip_prefix("ROLLBACK") {
        let rest = rest.trim_start();
        let rest = rest
            .strip_prefix("TRANSACTION")
            .map(str::trim_start)
            .unwrap_or(rest);
        if let Some(name) = rest.strip_prefix("TO ") {
            return Some(SavepointStep::RollbackTo(savepoint_name(name)));
        }
        return None;
    }
    if let Some(rest) = upper.strip_prefix("RELEASE ") {
        return Some(SavepointStep::Release(savepoint_name(rest)));
    }
    None
}

fn apply_savepoint_step(pending: &mut Pending, step: SavepointStep) {
    match step {
        SavepointStep::Open(name) => {
            let at = pending.changes.len();
            pending.savepoints.push((name, at));
        }
        SavepointStep::RollbackTo(name) => {
            if let Some(index) = pending
                .savepoints
                .iter()
                .rposition(|(open, _)| *open == name)
            {
                let at = pending.savepoints[index].1;
                pending.changes.truncate(at);
                pending.savepoints.truncate(index + 1);
            }
        }
        SavepointStep::Release(name) => {
            if let Some(index) = pending
                .savepoints
                .iter()
                .rposition(|(open, _)| *open == name)
            {
                pending.savepoints.truncate(index);
            }
        }
    }
}

#[cfg(test)]
#[path = "write_log_test.rs"]
mod write_log_test;
