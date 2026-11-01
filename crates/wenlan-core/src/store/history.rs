// SPDX-License-Identifier: Apache-2.0
//! The append-only history log (storage migration spec, section 4).
//!
//! Layout under the history root:
//! - `YYYY-MM/<seq-start>.jsonl`: one JSON object per line, append-only. A new
//!   file starts when the current one reaches the rotation size or an entry's
//!   month differs from the file's. `<seq-start>` is the seq of the file's
//!   first entry, zero-padded, and files are read in seq order whatever their
//!   month.
//! - `blobs/<sha256>`: any text over the blob threshold, content-addressed so
//!   a page version repeated across entries is stored once. A blob is written
//!   and synced before the entry that names it, so a crash leaves at most an
//!   unused blob.
//! - `.lock`: held while a log is open for writing, so two daemons never
//!   append to the same log.
//!
//! Every line ends with `"crc":"<8 hex>"`: the first 8 hex digits of the
//! SHA-256 of the line with that field taken out. Entries are grouped by `tx`,
//! the seq of the group's first entry. A group counts only once its
//! `op: "commit"` entry is on disk, and the commit records how many entries the
//! group has. Each entry is synced before `append` returns.
//!
//! Opening a log cuts a torn last line (a crash mid-write) off the last file.
//! Replay returns the entries of complete, committed groups in seq order and
//! reports what it left out: groups with no commit, committed groups that lost
//! an entry, and lines that fail their check.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::WenlanError;

/// A log file is rotated once it reaches this size.
pub const ROTATE_BYTES: u64 = 64 * 1024 * 1024;
/// Text longer than this many bytes goes to `blobs/`.
pub const BLOB_THRESHOLD: usize = 32 * 1024;
/// The op that ends a group of entries.
pub const COMMIT_OP: &str = "commit";

const BLOBS_DIR: &str = "blobs";
const LOCK_FILE: &str = ".lock";
const SEQ_WIDTH: usize = 12;
const CRC_LEN: usize = 8;
const CRC_PREFIX: &[u8] = b",\"crc\":\"";
// `,"crc":"` + 8 hex + `"}`
const CRC_SUFFIX_LEN: usize = 8 + CRC_LEN + 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryConfig {
    pub rotate_bytes: u64,
    pub blob_threshold: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            rotate_bytes: ROTATE_BYTES,
            blob_threshold: BLOB_THRESHOLD,
        }
    }
}

/// Who made a change. `Unknown` is for history seeded from rows that never
/// said (for example `migration_87`); it is protected like the person's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    Person,
    Ai,
    Original,
    Import,
    System,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub kind: ActorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

impl Actor {
    pub fn new(kind: ActorKind) -> Self {
        Self {
            kind,
            name: None,
            agent: None,
            session: None,
        }
    }

    pub fn system() -> Self {
        Self::new(ActorKind::System)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
}

impl Target {
    pub fn new(kind: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
        }
    }
}

/// The receipt of an idempotent operation, so a retry after a rebuild can be
/// answered from history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub caller_id: String,
    pub operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_digest: Option<String>,
}

/// One line of the log, without its `crc`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub seq: u64,
    /// Unix seconds.
    pub ts: i64,
    pub tx: u64,
    pub actor: Actor,
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// SHA-256 of the text, when the text is in `blobs/` instead of `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<Receipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// What a caller appends. `seq` and `tx` are assigned by the log; `ts`
/// defaults to now, and history seeded from older rows passes their time.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEntry {
    pub ts: Option<i64>,
    pub actor: Actor,
    pub op: String,
    pub target: Option<Target>,
    pub version: Option<i64>,
    pub why: Option<String>,
    pub text: Option<String>,
    pub digest_before: Option<String>,
    pub digest_after: Option<String>,
    pub receipt: Option<Receipt>,
    pub data: Option<serde_json::Value>,
}

impl NewEntry {
    pub fn new(actor: Actor, op: impl Into<String>) -> Self {
        Self {
            ts: None,
            actor,
            op: op.into(),
            target: None,
            version: None,
            why: None,
            text: None,
            digest_before: None,
            digest_after: None,
            receipt: None,
            data: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenReport {
    /// Bytes cut off the end of the last file: a torn or failing last line.
    pub repaired_tail_bytes: u64,
    pub files: usize,
    pub next_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadLine {
    pub file: PathBuf,
    /// 1-based.
    pub line: usize,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayReport {
    /// Groups with no commit on disk, by tx.
    pub uncommitted: Vec<u64>,
    /// Committed groups whose entry count does not match their commit, by tx.
    pub damaged: Vec<u64>,
    pub bad_lines: Vec<BadLine>,
    /// Seqs that were not above the one before them; those lines are skipped.
    pub out_of_order: Vec<u64>,
}

impl ReplayReport {
    pub fn is_clean(&self) -> bool {
        self.uncommitted.is_empty()
            && self.damaged.is_empty()
            && self.bad_lines.is_empty()
            && self.out_of_order.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Replay {
    /// Entries of complete, committed groups in seq order, commits left out.
    pub entries: Vec<HistoryEntry>,
    pub report: ReplayReport,
}

#[derive(Debug, Clone)]
struct LogFile {
    seq_start: u64,
    month: String,
    path: PathBuf,
}

struct OpenFile {
    month: String,
    file: File,
    len: u64,
}

/// A history log open for appending. Dropping it releases the lock.
pub struct HistoryLog {
    root: PathBuf,
    config: HistoryConfig,
    _lock: File,
    next_seq: u64,
    last_file: Option<LogFile>,
    current: Option<OpenFile>,
}

impl HistoryLog {
    pub fn open(root: &Path) -> Result<(Self, OpenReport), WenlanError> {
        Self::open_with(root, HistoryConfig::default())
    }

    pub fn open_with(
        root: &Path,
        config: HistoryConfig,
    ) -> Result<(Self, OpenReport), WenlanError> {
        fs::create_dir_all(root.join(BLOBS_DIR))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(LOCK_FILE))?;
        if lock.try_lock_exclusive().is_err() {
            return Err(WenlanError::Conflict(format!(
                "history log {} is open elsewhere",
                root.display()
            )));
        }

        let mut files = list_log_files(root)?;
        let mut repaired_tail_bytes = 0;
        if let Some(last) = files.last() {
            let (cut, now_empty) = repair_tail(&last.path)?;
            repaired_tail_bytes = cut;
            if now_empty {
                fs::remove_file(&last.path)?;
                sync_dir(last.path.parent().unwrap_or(root));
                files.pop();
            }
        }

        let next_seq = last_valid_seq(&files)?.map_or(1, |seq| seq + 1);
        let report = OpenReport {
            repaired_tail_bytes,
            files: files.len(),
            next_seq,
        };
        Ok((
            Self {
                root: root.to_path_buf(),
                config,
                _lock: lock,
                next_seq,
                last_file: files.pop(),
                current: None,
            },
            report,
        ))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The seq the next entry will get.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Starts a group. Its entries count once `Tx::commit` returns; a group
    /// dropped without a commit stays on disk and replay reports it.
    pub fn begin(&mut self, actor: Actor) -> Tx<'_> {
        Tx {
            log: self,
            actor,
            id: None,
            entries: 0,
            last_ts: None,
        }
    }

    /// Appends one entry as its own committed group and returns its seq.
    pub fn write(&mut self, entry: NewEntry) -> Result<u64, WenlanError> {
        let mut tx = self.begin(entry.actor.clone());
        let seq = tx.append(entry)?;
        tx.commit()?;
        Ok(seq)
    }

    pub fn replay(&self) -> Result<Replay, WenlanError> {
        replay(&self.root)
    }

    pub fn read_text(&self, entry: &HistoryEntry) -> Result<Option<String>, WenlanError> {
        read_text(&self.root, entry)
    }

    fn append_entry(
        &mut self,
        tx: Option<u64>,
        new: NewEntry,
        ts: i64,
    ) -> Result<u64, WenlanError> {
        let seq = self.next_seq;
        let (text, blob) = match new.text {
            Some(text) if text.len() > self.config.blob_threshold => {
                let digest = write_blob(&self.root, &text)?;
                (None, Some(digest))
            }
            text => (text, None),
        };
        let entry = HistoryEntry {
            seq,
            ts,
            tx: tx.unwrap_or(seq),
            actor: new.actor,
            op: new.op,
            target: new.target,
            version: new.version,
            why: new.why,
            text,
            blob,
            digest_before: new.digest_before,
            digest_after: new.digest_after,
            receipt: new.receipt,
            data: new.data,
        };
        let line = encode_line(&entry)?;
        let month = month_of(ts);
        let current = self.file_for(seq, &month)?;
        let written = current
            .file
            .write_all(&line)
            .and_then(|()| current.file.sync_data());
        if let Err(error) = written {
            // Never leave half a line for the next entry to follow.
            let _ = current.file.set_len(current.len);
            let _ = current.file.sync_data();
            self.current = None;
            return Err(error.into());
        }
        current.len += line.len() as u64;
        self.next_seq += 1;
        Ok(seq)
    }

    fn file_for(&mut self, seq: u64, month: &str) -> Result<&mut OpenFile, WenlanError> {
        let rotate_bytes = self.config.rotate_bytes;
        let fits = |m: &str, len: u64| m == month && len < rotate_bytes;
        if let Some(open) = &self.current {
            if !fits(&open.month, open.len) {
                self.current = None;
            }
        }
        if self.current.is_none() {
            if let Some(last) = &self.last_file {
                let len = fs::metadata(&last.path)?.len();
                if fits(&last.month, len) {
                    let file = OpenOptions::new().append(true).open(&last.path)?;
                    self.current = Some(OpenFile {
                        month: last.month.clone(),
                        file,
                        len,
                    });
                }
            }
        }
        let reuse = self.current.is_some();
        if !reuse {
            let dir = self.root.join(month);
            fs::create_dir_all(&dir)?;
            let path = dir.join(format!("{seq:0width$}.jsonl", width = SEQ_WIDTH));
            let file = OpenOptions::new()
                .create_new(true)
                .append(true)
                .open(&path)?;
            sync_dir(&dir);
            sync_dir(&self.root);
            self.last_file = Some(LogFile {
                seq_start: seq,
                month: month.to_string(),
                path,
            });
            self.current = Some(OpenFile {
                month: month.to_string(),
                file,
                len: 0,
            });
        }
        Ok(self.current.as_mut().expect("current file was just set"))
    }
}

/// A group of entries that counts once committed.
pub struct Tx<'a> {
    log: &'a mut HistoryLog,
    actor: Actor,
    id: Option<u64>,
    entries: u64,
    last_ts: Option<i64>,
}

impl Tx<'_> {
    /// The group's tx: the seq of its first entry, once it has one.
    pub fn id(&self) -> Option<u64> {
        self.id
    }

    pub fn append(&mut self, entry: NewEntry) -> Result<u64, WenlanError> {
        let ts = entry.ts.unwrap_or_else(now);
        let seq = self.log.append_entry(self.id, entry, ts)?;
        self.id.get_or_insert(seq);
        self.entries += 1;
        self.last_ts = Some(ts);
        Ok(seq)
    }

    /// Writes the commit and returns the group's tx. An empty group writes
    /// nothing and returns `None`.
    pub fn commit(self) -> Result<Option<u64>, WenlanError> {
        let Some(id) = self.id else {
            return Ok(None);
        };
        let mut commit = NewEntry::new(self.actor, COMMIT_OP);
        commit.data = Some(serde_json::json!({ "entries": self.entries }));
        // The commit carries the group's time so seeded history stays in
        // its own month's file.
        let ts = self.last_ts.unwrap_or_else(now);
        self.log.append_entry(Some(id), commit, ts)?;
        Ok(Some(id))
    }
}

/// Reads a log without opening it for writing (for `doctor` and rebuild).
pub fn replay(root: &Path) -> Result<Replay, WenlanError> {
    let mut report = ReplayReport::default();
    let mut groups: BTreeMap<u64, Vec<HistoryEntry>> = BTreeMap::new();
    let mut commits: BTreeMap<u64, Option<u64>> = BTreeMap::new();
    let mut last_seq = 0;

    for file in list_log_files(root)? {
        let bytes = fs::read(&file.path)?;
        for (index, line) in split_lines(&bytes).enumerate() {
            let entry = match decode_line(line.bytes) {
                Ok(entry) => entry,
                Err(reason) => {
                    report.bad_lines.push(BadLine {
                        file: file.path.clone(),
                        line: index + 1,
                        reason: if line.terminated {
                            reason
                        } else {
                            format!("{reason} (no line end)")
                        },
                    });
                    continue;
                }
            };
            if entry.seq <= last_seq {
                report.out_of_order.push(entry.seq);
                continue;
            }
            last_seq = entry.seq;
            if entry.op == COMMIT_OP {
                let expected = entry
                    .data
                    .as_ref()
                    .and_then(|data| data.get("entries"))
                    .and_then(serde_json::Value::as_u64);
                commits.insert(entry.tx, expected);
            } else {
                groups.entry(entry.tx).or_default().push(entry);
            }
        }
    }

    let mut entries = Vec::new();
    for (tx, group) in groups {
        match commits.remove(&tx) {
            Some(Some(expected)) if expected == group.len() as u64 => entries.extend(group),
            Some(_) => report.damaged.push(tx),
            None => report.uncommitted.push(tx),
        }
    }
    // A commit whose every entry was lost.
    report.damaged.extend(commits.into_keys());
    report.damaged.sort_unstable();
    entries.sort_by_key(|entry| entry.seq);
    Ok(Replay { entries, report })
}

/// The entry's text, read from `blobs/` when it was stored there.
pub fn read_text(root: &Path, entry: &HistoryEntry) -> Result<Option<String>, WenlanError> {
    if let Some(text) = &entry.text {
        return Ok(Some(text.clone()));
    }
    match &entry.blob {
        Some(digest) => read_blob(root, digest).map(Some),
        None => Ok(None),
    }
}

pub fn read_blob(root: &Path, digest: &str) -> Result<String, WenlanError> {
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(WenlanError::Validation(format!(
            "not a blob digest: {digest}"
        )));
    }
    let path = root.join(BLOBS_DIR).join(digest);
    let mut bytes = Vec::new();
    match File::open(&path) {
        Ok(mut file) => file.read_to_end(&mut bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(WenlanError::NotFound(format!("history blob {digest}")));
        }
        Err(error) => return Err(error.into()),
    };
    if sha256_hex(&bytes) != digest {
        return Err(WenlanError::Generic(format!(
            "history blob {digest} does not match its digest"
        )));
    }
    String::from_utf8(bytes)
        .map_err(|_| WenlanError::Generic(format!("history blob {digest} is not UTF-8")))
}

fn write_blob(root: &Path, text: &str) -> Result<String, WenlanError> {
    let digest = sha256_hex(text.as_bytes());
    let dir = root.join(BLOBS_DIR);
    let path = dir.join(&digest);
    if path.exists() {
        return Ok(digest);
    }
    let tmp = dir.join(format!(".tmp-{digest}-{}", std::process::id()));
    {
        let mut file = File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, &path)?;
    sync_dir(&dir);
    Ok(digest)
}

fn encode_line(entry: &HistoryEntry) -> Result<Vec<u8>, WenlanError> {
    let mut json = serde_json::to_vec(entry)?;
    let crc = crc_of(&json);
    // `{...}` becomes `{...,"crc":"xxxxxxxx"}\n`.
    json.pop();
    json.extend_from_slice(CRC_PREFIX);
    json.extend_from_slice(crc.as_bytes());
    json.extend_from_slice(b"\"}\n");
    Ok(json)
}

fn decode_line(line: &[u8]) -> Result<HistoryEntry, String> {
    if line.len() < CRC_SUFFIX_LEN + 2 || !line.ends_with(b"\"}") {
        return Err("no crc".to_string());
    }
    let split = line.len() - CRC_SUFFIX_LEN;
    let (body, suffix) = line.split_at(split);
    if !suffix.starts_with(CRC_PREFIX) {
        return Err("no crc".to_string());
    }
    let crc = &suffix[CRC_PREFIX.len()..CRC_PREFIX.len() + CRC_LEN];
    let mut json = body.to_vec();
    json.push(b'}');
    if crc_of(&json).as_bytes() != crc {
        return Err("crc mismatch".to_string());
    }
    serde_json::from_slice(&json).map_err(|error| format!("bad entry: {error}"))
}

fn crc_of(bytes: &[u8]) -> String {
    let mut hex = sha256_hex(bytes);
    hex.truncate(CRC_LEN);
    hex
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

struct Line<'a> {
    bytes: &'a [u8],
    terminated: bool,
}

fn split_lines(bytes: &[u8]) -> impl Iterator<Item = Line<'_>> {
    let mut rest = bytes;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        match rest.iter().position(|&b| b == b'\n') {
            Some(end) => {
                let line = &rest[..end];
                rest = &rest[end + 1..];
                Some(Line {
                    bytes: line,
                    terminated: true,
                })
            }
            None => {
                let line = rest;
                rest = &[];
                Some(Line {
                    bytes: line,
                    terminated: false,
                })
            }
        }
    })
}

/// Cuts the file back to the end of its last line that passes its check.
/// Returns the bytes cut and whether the file is now empty.
fn repair_tail(path: &Path) -> Result<(u64, bool), WenlanError> {
    let bytes = fs::read(path)?;
    let mut good_len = 0usize;
    let mut offset = 0usize;
    for line in split_lines(&bytes) {
        let end = offset + line.bytes.len() + usize::from(line.terminated);
        if line.terminated && decode_line(line.bytes).is_ok() {
            good_len = end;
        }
        offset = end;
    }
    // Only a failing run at the very end is a torn write; a bad line followed
    // by good ones is left for replay to report.
    let cut = (bytes.len() - good_len) as u64;
    if cut > 0 {
        let file = OpenOptions::new().write(true).open(path)?;
        file.set_len(good_len as u64)?;
        file.sync_all()?;
    }
    Ok((cut, good_len == 0))
}

fn last_valid_seq(files: &[LogFile]) -> Result<Option<u64>, WenlanError> {
    for file in files.iter().rev() {
        let bytes = fs::read(&file.path)?;
        let max = split_lines(&bytes)
            .filter_map(|line| decode_line(line.bytes).ok())
            .map(|entry| entry.seq)
            .max();
        if max.is_some() {
            return Ok(max);
        }
    }
    Ok(None)
}

fn list_log_files(root: &Path) -> Result<Vec<LogFile>, WenlanError> {
    let mut files = Vec::new();
    let dirs = match fs::read_dir(root) {
        Ok(dirs) => dirs,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(files),
        Err(error) => return Err(error.into()),
    };
    for dir in dirs {
        let dir = dir?;
        let month = dir.file_name().to_string_lossy().into_owned();
        if !is_month(&month) || !dir.file_type()?.is_dir() {
            continue;
        }
        for file in fs::read_dir(dir.path())? {
            let file = file?;
            let name = file.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".jsonl") else {
                continue;
            };
            if stem.len() != SEQ_WIDTH || !stem.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            if let Ok(seq_start) = stem.parse() {
                files.push(LogFile {
                    seq_start,
                    month: month.clone(),
                    path: file.path(),
                });
            }
        }
    }
    files.sort_by_key(|file| file.seq_start);
    Ok(files)
}

fn is_month(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 7
        && bytes[4] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..].iter().all(u8::is_ascii_digit)
}

fn month_of(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|time| time.format("%Y-%m").to_string())
        .unwrap_or_else(|| "1970-01".to_string())
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Makes a new or renamed directory entry durable. Where a directory cannot
/// be opened as a file (Windows), the open fails and the file system's own
/// journal covers the entry.
fn sync_dir(dir: &Path) {
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
}

#[cfg(test)]
#[path = "history_test.rs"]
mod tests;
