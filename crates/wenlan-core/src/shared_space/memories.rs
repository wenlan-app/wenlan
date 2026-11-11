// SPDX-License-Identifier: Apache-2.0
//! The library's memories: what a person or their AI tool saved, shared
//! between the person's own computers through the library folder. Only
//! [`Scope::Library`](super::Scope::Library) does this; a Space or a page
//! share, and a team server, never carry a memory.
//!
//! ```text
//! <engine>/members/<member>/memories/<memory_id>/<sha256>.wlv
//! ```
//!
//! A version is the memory file (`store::format::memory`: the same text and
//! frontmatter the backstage export writes) with one more frontmatter key,
//! `shared`: `parent`, `merged`, `author` (`member`, `kind` person or ai,
//! `agent`), `ts`, and `deleted: true` on a tombstone (the person forgot the
//! memory here; the version keeps its last text, so a restore brings it
//! back). Versions are written once, named by the sha256 of their bytes, and
//! the head is picked the way a page's is (`version::head_of`): later `ts`
//! wins, the larger sha when they tie, the loser stays as an older version.
//!
//! What is compared is what someone stated ([`state_digest`]): the text and
//! the facts about it (kind when stated, Space, who saved it, pinned,
//! confirmed, …). What enrichment works out (title, importance, retrieval
//! cue, entity links, guessed kind and fields) travels as a seed in a new
//! file but never counts as a change, so two computers enriching the same
//! memory never trade versions. Only memory files are shared, by the same
//! rule the backstage export uses (`is_memory_file`): documents, saved
//! pages and imported chats are source material and stay where they are.
//!
//! A memory made or changed here from the folder is stored the way a capture
//! is (`MemoryDB::apply_shared_memory`): no vector yet, no enrichment
//! receipt, so the embedding catch-up and the ambient lanes take it. What
//! this Wenlan remembers per memory (`MHeld`) is in `app_metadata` under the
//! library's key; a rebuilt database remembers nothing and takes the
//! folder's memories by content, writing no version.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use wenlan_types::shared_space::SharedSyncCounts;

use super::version::{
    self, head_of, invalid, is_safe_id, sha256_hex, sub_dirs, version_file_name, version_sha,
    Author, AuthorKind, Head, Versioned, MEMBERS_DIR, MEMORIES_DIR,
};
use super::{named_space, now_ts, ShareSettings, LIBRARY_KEY};
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::store::export::memories::{is_memory_file, memory_file, one_row};
use crate::store::export::Manifest;
use crate::store::format::memory::{self, MemoryFile};

/// The frontmatter key of a version's sharing facts.
const SHARED_KEY: &str = "shared";

/// A pass that would forget more than this many memories here, and more
/// than half of them, forgets none.
const MASS_FORGET_MIN: usize = 20;

/// The `edited_by` this Wenlan writes in the activity log when it forgets or
/// restores a memory for another computer's version.
const SYNC_AGENT: &str = "library_sync";

// ===== One version =====

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MemMeta {
    pub parent: Option<String>,
    pub merged: Vec<String>,
    pub author: Author,
    pub ts: String,
    /// The version forgets the memory.
    pub deleted: bool,
}

/// A memory version read from the folder.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct MemVersion {
    /// The file's name without its extension: the sha256 of its bytes.
    pub sha: String,
    pub meta: MemMeta,
    /// The memory as written, without the `shared` key.
    pub file: MemoryFile,
    /// [`state_digest`] of `file`.
    pub digest: String,
}

impl Versioned for Arc<MemVersion> {
    fn sha(&self) -> &str {
        &self.sha
    }
    fn parents(&self) -> Vec<&str> {
        self.meta
            .parent
            .iter()
            .chain(self.meta.merged.iter())
            .map(String::as_str)
            .collect()
    }
    fn ts(&self) -> &str {
        &self.meta.ts
    }
}

fn text(value: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(value.to_string())
}

/// Writes a version file.
pub(super) fn render(file: &MemoryFile, meta: &MemMeta) -> String {
    let mut author = serde_yaml::Mapping::new();
    author.insert(text("member"), text(&meta.author.member));
    author.insert(text("kind"), text(meta.author.kind.as_str()));
    if let Some(agent) = &meta.author.agent {
        author.insert(text("agent"), text(agent));
    }
    let mut shared = serde_yaml::Mapping::new();
    if let Some(parent) = &meta.parent {
        shared.insert(text("parent"), text(parent));
    }
    if !meta.merged.is_empty() {
        shared.insert(
            text("merged"),
            serde_yaml::Value::Sequence(meta.merged.iter().map(|sha| text(sha)).collect()),
        );
    }
    shared.insert(text("author"), serde_yaml::Value::Mapping(author));
    shared.insert(text("ts"), text(&meta.ts));
    if meta.deleted {
        shared.insert(text("deleted"), serde_yaml::Value::Bool(true));
    }
    let mut file = file.clone();
    file.other.shift_remove(SHARED_KEY);
    file.other
        .insert(text(SHARED_KEY), serde_yaml::Value::Mapping(shared));
    memory::render(&file)
}

fn scalar(map: &serde_yaml::Mapping, name: &str) -> Option<String> {
    match map.get(name)? {
        serde_yaml::Value::String(value) => Some(value.clone()),
        _ => None,
    }
}

/// Reads the version file `name` from its bytes. A file whose bytes do not
/// match its name is not a whole version.
pub(super) fn parse(name: &str, bytes: &[u8]) -> Result<MemVersion, WenlanError> {
    let sha = version_sha(name).ok_or_else(|| invalid(format!("{name} is not a version")))?;
    if sha256_hex(bytes) != sha {
        return Err(invalid(format!("{name} does not match its name")));
    }
    let body = std::str::from_utf8(bytes).map_err(|_| invalid(format!("{name} is not text")))?;
    let mut file = memory::parse(body)?;
    let Some(serde_yaml::Value::Mapping(shared)) = file.other.shift_remove(SHARED_KEY) else {
        return Err(invalid(format!("{name} has no shared key")));
    };
    let Some(serde_yaml::Value::Mapping(author)) = shared.get("author") else {
        return Err(invalid(format!("{name} names no author")));
    };
    let author = Author {
        member: scalar(author, "member").ok_or_else(|| invalid(format!("{name}: no member")))?,
        kind: match scalar(author, "kind").as_deref() {
            Some("person") => AuthorKind::Person,
            Some("ai") => AuthorKind::Ai,
            _ => return Err(invalid(format!("{name}: author kind is not person or ai"))),
        },
        agent: scalar(author, "agent"),
    };
    let merged = match shared.get("merged") {
        None => Vec::new(),
        Some(serde_yaml::Value::Sequence(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| invalid(format!("{name}: merged is not a list of versions")))?,
        Some(_) => return Err(invalid(format!("{name}: merged is not a list"))),
    };
    let deleted = match shared.get("deleted") {
        None => false,
        Some(serde_yaml::Value::Bool(deleted)) => *deleted,
        Some(_) => return Err(invalid(format!("{name}: deleted is not true or false"))),
    };
    let digest = state_digest(&file);
    Ok(MemVersion {
        sha: sha.to_string(),
        meta: MemMeta {
            parent: scalar(&shared, "parent"),
            merged,
            author,
            ts: scalar(&shared, "ts").unwrap_or_default(),
            deleted,
        },
        file,
        digest,
    })
}

/// What two versions, or a version and the memory here, are compared by:
/// the memory file without what enrichment works out (title, importance,
/// retrieval cue), so enrichment on either computer is never a change, and
/// without its creation time, which no one edits.
pub(super) fn state_digest(file: &MemoryFile) -> String {
    let mut file = file.clone();
    file.title = None;
    file.importance = None;
    file.created = None;
    file.other.shift_remove("cue");
    sha256_hex(memory::render(&file).as_bytes())
}

// ===== Reading the folder =====

/// Every whole memory version in the folder, by memory id.
#[derive(Default)]
pub(super) struct FolderRead {
    pub memories: HashMap<String, Vec<Arc<MemVersion>>>,
    /// Files that are not whole versions yet.
    pub skipped: u32,
}

/// Versions already read, by folder and then sha: a version's name is the
/// sha256 of its bytes, so a file seen before is never read or parsed
/// again. Each pass keeps only what it saw.
type Seen = HashMap<PathBuf, HashMap<String, Arc<MemVersion>>>;
static SEEN: LazyLock<Mutex<Seen>> = LazyLock::new(Default::default);

pub(super) fn read_folder(folder: &Path) -> FolderRead {
    let known = SEEN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(folder)
        .unwrap_or_default();
    let mut now_known: HashMap<String, Arc<MemVersion>> = HashMap::new();
    let mut read = FolderRead::default();
    for (member, member_dir) in sub_dirs(&folder.join(MEMBERS_DIR)) {
        for (id, dir) in sub_dirs(&member_dir.join(MEMORIES_DIR)) {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                read.skipped += 1;
                continue;
            };
            for entry in entries.flatten() {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if name.ends_with(".icloud") {
                    read.skipped += 1;
                    continue;
                }
                let Some(sha) = version_sha(&name) else {
                    continue;
                };
                if now_known.contains_key(sha) {
                    continue;
                }
                let version = match known.get(sha) {
                    Some(version) => Ok(version.clone()),
                    None => std::fs::read(entry.path())
                        .map_err(WenlanError::from)
                        .and_then(|bytes| parse(&name, &bytes))
                        .map(Arc::new),
                };
                match version {
                    Ok(version)
                        if version.file.id == id && version.meta.author.member == member =>
                    {
                        now_known.insert(version.sha.clone(), version.clone());
                        read.memories.entry(id.clone()).or_default().push(version);
                    }
                    Ok(_) => {
                        log::warn!("[shared_space] {name} is filed under another memory or member");
                        read.skipped += 1;
                    }
                    Err(error) => {
                        log::debug!("[shared_space] skipping {name}: {error}");
                        read.skipped += 1;
                    }
                }
            }
        }
    }
    SEEN.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(folder.to_path_buf(), now_known);
    read
}

// ===== This Wenlan's memories =====

/// A memory here that the library shares.
#[derive(Debug, Clone)]
pub(super) struct Local {
    /// As a version carries it.
    pub file: MemoryFile,
    pub digest: String,
    /// Forgotten here (retired, restorable): the library's tombstone.
    pub forgotten: bool,
}

fn forgotten(head: &crate::db::export_reads::ExportMemoryRow) -> bool {
    head.supersede_mode.as_deref() == Some("evicted")
}

/// The memory file for `chunks`, shaped for sharing: what is stated, plus
/// seeds enrichment would otherwise have to work out again.
fn shaped(
    id: &str,
    chunks: &[&crate::db::export_reads::ExportMemoryRow],
) -> Result<Local, WenlanError> {
    let head = chunks[0];
    let mut file = memory_file(
        id,
        head,
        chunks,
        &mut HashMap::new(),
        &HashMap::new(),
        &mut Manifest::default(),
    );
    one_row(&mut file);
    let gone = forgotten(head);
    file.entity = None;
    file.about.clear();
    for key in [
        "guessed_kind",
        "guessed_fields",
        "summaries",
        "rows",
        "modified",
        "pending",
        "space_rejected",
    ] {
        file.other.shift_remove(key);
    }
    file.space = named_space(file.space.as_deref()).map(str::to_string);
    // Hidden (the default) and forgotten are not stated modes.
    if matches!(file.supersede_mode.as_deref(), Some("hide" | "evicted")) {
        file.supersede_mode = None;
    }
    let digest = state_digest(&file);
    Ok(Local {
        file,
        digest,
        forgotten: gone,
    })
}

fn group(
    rows: &[crate::db::export_reads::ExportMemoryRow],
    own_spaces: &HashSet<String>,
    left_out: &mut HashSet<String>,
) -> Result<BTreeMap<String, Local>, WenlanError> {
    let mut by_id: BTreeMap<&str, Vec<&crate::db::export_reads::ExportMemoryRow>> = BTreeMap::new();
    for row in rows {
        by_id.entry(row.source_id.as_str()).or_default().push(row);
    }
    let mut out = BTreeMap::new();
    for (id, chunks) in by_id {
        let head = chunks[0];
        if !is_memory_file(head) || !is_safe_id(id) {
            continue;
        }
        // Kept out of the library: a Space with its own share carries it.
        if named_space(head.space.as_deref()).is_some_and(|space| own_spaces.contains(space)) {
            left_out.insert(id.to_string());
            continue;
        }
        out.insert(id.to_string(), shaped(id, &chunks)?);
    }
    Ok(out)
}

/// The memories here that the library shares, by id, and the ids it leaves
/// to a Space share.
async fn local_memories(
    db: &MemoryDB,
    own_spaces: &HashSet<String>,
) -> Result<(BTreeMap<String, Local>, HashSet<String>), WenlanError> {
    let rows = db.export_shareable_memory_rows().await?;
    let mut left_out = HashSet::new();
    let local = group(&rows, own_spaces, &mut left_out)?;
    Ok((local, left_out))
}

/// One memory here, after something was written.
async fn local_memory(db: &MemoryDB, id: &str) -> Result<Option<Local>, WenlanError> {
    if !is_safe_id(id) {
        return Ok(None);
    }
    let rows = db.export_shareable_memory_rows_of(id).await?;
    Ok(group(&rows, &HashSet::new(), &mut HashSet::new())?.remove(id))
}

/// Whether the library shares memory `id`: it is here, shareable, and not
/// in a Space with its own share. A page's citation of such a memory can
/// stay a citation, because every computer of the library has the memory.
pub(super) async fn shared_by_library(
    db: &MemoryDB,
    id: &str,
    own_spaces: &HashSet<String>,
) -> Result<bool, WenlanError> {
    if !is_safe_id(id) {
        return Ok(false);
    }
    let rows = db.export_shareable_memory_rows_of(id).await?;
    Ok(group(&rows, own_spaces, &mut HashSet::new())?.contains_key(id))
}

// ===== What this Wenlan remembers =====

/// For one memory, the folder version this Wenlan was last in step with and
/// the memory's own state here at that point. It tells an edit or a delete
/// made here from a version the memory still holds, and a memory the person
/// deleted from one this Wenlan never had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct MHeld {
    pub sha: String,
    /// [`Local::digest`] then.
    pub digest: String,
    /// The memory was forgotten (or gone) here then.
    #[serde(default)]
    pub deleted: bool,
}

fn held_key() -> String {
    format!("shared_space.held_memories:{LIBRARY_KEY}")
}

async fn read_held(db: &MemoryDB) -> Result<BTreeMap<String, MHeld>, WenlanError> {
    let Some(raw) = db.get_app_metadata(&held_key()).await? else {
        return Ok(BTreeMap::new());
    };
    Ok(serde_json::from_str(&raw).unwrap_or_else(|error| {
        log::warn!("[shared_space] what the library last held of memories cannot be read ({error}); starting over");
        BTreeMap::new()
    }))
}

async fn save_held(db: &MemoryDB, held: &BTreeMap<String, MHeld>) -> Result<(), WenlanError> {
    db.set_app_metadata(&held_key(), &serde_json::to_string(held)?)
        .await
}

// ===== The pass =====

struct MemPass<'a> {
    db: &'a MemoryDB,
    settings: &'a ShareSettings,
    folder: &'a Path,
    own_spaces: &'a HashSet<String>,
    /// Version files this pass could not read yet.
    unread: u32,
}

/// The library's memories, once per pass, after its pages: what this
/// Wenlan changed is written as new versions, and what the folder's head
/// says is made, changed or forgotten here.
pub(super) async fn sync(
    db: &MemoryDB,
    settings: &ShareSettings,
    folder: &Path,
    own_spaces: &HashSet<String>,
    counts: &mut SharedSyncCounts,
) -> Result<(), WenlanError> {
    let (local, left_out) = local_memories(db, own_spaces).await?;
    let mut read = {
        let folder = folder.to_path_buf();
        tokio::task::spawn_blocking(move || read_folder(&folder))
            .await
            .map_err(|error| WenlanError::Generic(format!("shared folder task: {error}")))?
    };
    read.memories.retain(|id, _| !left_out.contains(id));
    counts.skipped += read.skipped;
    let pass = MemPass {
        db,
        settings,
        folder,
        own_spaces,
        unread: read.skipped,
    };
    let mut held = read_held(db).await?;
    let empty = Vec::new();

    // Applying the folder must not forget most of the memories at once (a
    // damaged database or an emptied folder on the other computer).
    let forgetting: HashSet<&str> = local
        .iter()
        .filter(|(_, here)| !here.forgotten)
        .filter(|(id, here)| {
            let versions = read.memories.get(id.as_str()).unwrap_or(&empty);
            let Some(head) = head_of(versions) else {
                return false;
            };
            head.winner.meta.deleted
                && held.get(id.as_str()).is_none_or(|before| {
                    before.sha != head.winner.sha && before.digest == here.digest && !before.deleted
                })
        })
        .map(|(id, _)| id.as_str())
        .collect();
    let hold = if forgetting.len() > MASS_FORGET_MIN && forgetting.len() * 2 > local.len() {
        log::warn!(
            "[shared_space] the folder would forget {} of {} memories here; none is forgotten this pass",
            forgetting.len(),
            local.len()
        );
        forgetting
    } else {
        HashSet::new()
    };

    let ids: std::collections::BTreeSet<&str> = local
        .keys()
        .map(String::as_str)
        .chain(read.memories.keys().map(String::as_str))
        .collect();
    let mut unsaved = false;
    for id in ids {
        if hold.contains(id) {
            counts.memories_held_back += 1;
            continue;
        }
        let versions = read.memories.get(id).unwrap_or(&empty);
        let before = held.get(id).cloned();
        let exported = counts.memories_exported;
        let now = pass
            .one(id, local.get(id), versions, before.as_ref(), counts)
            .await?;
        if let Some(now) = now.filter(|now| before.as_ref() != Some(now)) {
            held.insert(id.to_string(), now);
            unsaved = true;
        }
        // A version written but not remembered would be written again next
        // pass, so it is kept at once.
        if unsaved && counts.memories_exported != exported {
            save_held(db, &held).await?;
            unsaved = false;
        }
    }
    if unsaved {
        save_held(db, &held).await?;
    }
    Ok(())
}

/// What the library's pages do with a version, for a memory.
type Versions = [Arc<MemVersion>];

impl MemPass<'_> {
    fn leaves_to_own_share(&self, version: &MemVersion) -> bool {
        named_space(version.file.space.as_deref())
            .is_some_and(|space| self.own_spaces.contains(space))
    }

    fn newest_with<'v>(&self, versions: &'v Versions, here: &Local) -> Option<&'v Arc<MemVersion>> {
        versions
            .iter()
            .filter(|v| v.digest == here.digest && v.meta.deleted == here.forgotten)
            .max_by(|a, b| (&a.meta.ts, &a.sha).cmp(&(&b.meta.ts, &b.sha)))
    }

    async fn one(
        &self,
        id: &str,
        here: Option<&Local>,
        versions: &Versions,
        held: Option<&MHeld>,
        counts: &mut SharedSyncCounts,
    ) -> Result<Option<MHeld>, WenlanError> {
        let head = head_of(versions);
        match here {
            Some(here) => self.sync_here(id, here, versions, head, held, counts).await,
            None => self.sync_missing(id, head, held, counts).await,
        }
    }

    /// A memory that is here (live or forgotten).
    async fn sync_here(
        &self,
        id: &str,
        here: &Local,
        versions: &Versions,
        head: Option<Head<'_, Arc<MemVersion>>>,
        held: Option<&MHeld>,
        counts: &mut SharedSyncCounts,
    ) -> Result<Option<MHeld>, WenlanError> {
        // Forgotten before it was ever shared: it stays here only.
        if here.forgotten && held.is_none() {
            return Ok(None);
        }
        let recorded = held.and_then(|held| versions.iter().find(|v| v.sha == held.sha));
        let unchanged_since =
            held.is_some_and(|held| held.digest == here.digest && held.deleted == here.forgotten);
        // The version this memory was last in step with, and whether it was
        // changed here since.
        let (base, changed) = match (held, recorded) {
            (Some(_), Some(version)) => (Some(version), !unchanged_since),
            // The version cannot be read this pass, but the memory did not
            // change here since: not an edit made here.
            (Some(_), None) if unchanged_since => (self.newest_with(versions, here), false),
            // Changed here over a version that cannot be read: wait for the
            // folder to be whole.
            (Some(_), None) if self.unread > 0 => {
                counts.skipped += 1;
                return Ok(None);
            }
            _ => match self.newest_with(versions, here) {
                Some(version) => (Some(version), false),
                None => (None, true),
            },
        };
        let mine = |sha: &str| MHeld {
            sha: sha.to_string(),
            digest: here.digest.clone(),
            deleted: here.forgotten,
        };
        if let Some(head) = head.as_ref().filter(|head| {
            head.winner.digest == here.digest && head.winner.meta.deleted == here.forgotten
        }) {
            // Already the head.
            return Ok(Some(mine(&head.winner.sha)));
        }
        if changed && !self.settings.read_only {
            return match self.export(id, here, head.as_ref(), base).await? {
                Some(sha) => {
                    counts.memories_exported += 1;
                    Ok(Some(mine(&sha)))
                }
                None => {
                    counts.skipped += 1;
                    Ok(None)
                }
            };
        }
        let Some(head) = head else {
            return Ok(None);
        };
        // Already taken once; whatever differs since is not a change here.
        if unchanged_since && held.is_some_and(|held| held.sha == head.winner.sha) {
            return Ok(held.cloned());
        }
        if self.leaves_to_own_share(head.winner) {
            return Ok(None);
        }
        match self.apply(id, Some(here), head.winner).await? {
            Some(now) => {
                counts.memories_applied += 1;
                Ok(Some(MHeld {
                    sha: head.winner.sha.clone(),
                    digest: now.digest,
                    deleted: now.forgotten,
                }))
            }
            None => {
                counts.skipped += 1;
                Ok(None)
            }
        }
    }

    /// A memory in the folder that is not here at all.
    async fn sync_missing(
        &self,
        id: &str,
        head: Option<Head<'_, Arc<MemVersion>>>,
        held: Option<&MHeld>,
        counts: &mut SharedSyncCounts,
    ) -> Result<Option<MHeld>, WenlanError> {
        let Some(head) = head else {
            return Ok(None);
        };
        let winner = head.winner;
        // A memory of a Space that has its own share is taken through that
        // share, which carries no memories: it stays in the folder only.
        if self.leaves_to_own_share(winner) {
            return Ok(None);
        }
        let deleted = |sha: &str, digest: &str| MHeld {
            sha: sha.to_string(),
            digest: digest.to_string(),
            deleted: true,
        };
        match held {
            // The person deleted it here for good. It is not made again: the
            // others get a version that forgets it.
            Some(before) if !before.deleted => {
                if winner.meta.deleted || self.settings.read_only {
                    return Ok(Some(deleted(&winner.sha, &before.digest)));
                }
                match self.export_tombstone(id, &head).await? {
                    Some(sha) => {
                        counts.memories_exported += 1;
                        Ok(Some(deleted(&sha, &before.digest)))
                    }
                    None => {
                        counts.skipped += 1;
                        Ok(None)
                    }
                }
            }
            // Deleted here earlier: only a restore elsewhere since brings it
            // back.
            Some(before) if before.sha == winner.sha || winner.meta.deleted => {
                Ok(Some(deleted(&winner.sha, &before.digest)))
            }
            // Never here, or restored elsewhere since. A memory that was
            // forgotten before it ever reached here is not made.
            _ if winner.meta.deleted => Ok(None),
            _ => match self.apply(id, None, winner).await? {
                Some(now) => {
                    counts.memories_applied += 1;
                    Ok(Some(MHeld {
                        sha: winner.sha.clone(),
                        digest: now.digest,
                        deleted: now.forgotten,
                    }))
                }
                None => {
                    counts.skipped += 1;
                    Ok(None)
                }
            },
        }
    }

    /// Makes the head here: forgets, restores or stores the memory as a
    /// capture. Returns the memory as it is here after, `None` when it could
    /// not be written this pass.
    async fn apply(
        &self,
        id: &str,
        here: Option<&Local>,
        head: &MemVersion,
    ) -> Result<Option<Local>, WenlanError> {
        let db = self.db;
        if head.meta.deleted {
            if here.is_some_and(|here| !here.forgotten) {
                db.forget_memory(id, SYNC_AGENT, "forgotten on another computer")
                    .await?;
            }
            return local_memory(db, id).await;
        }
        if let Some(name) = named_space(head.file.space.as_deref()) {
            if !super::ensure_space(db, name).await? {
                return Ok(None);
            }
        }
        if here.is_some_and(|here| here.forgotten) {
            db.restore_forgotten_memory(id, SYNC_AGENT).await?;
        }
        let current = match here {
            Some(here) if here.forgotten => local_memory(db, id).await?,
            Some(here) => Some(here.clone()),
            None => None,
        };
        if current.as_ref().is_none_or(|now| now.digest != head.digest) {
            match db.apply_shared_memory(&head.file).await {
                Ok(()) => {}
                Err(error) => {
                    super::skipped_write(id, error)?;
                    return Ok(None);
                }
            }
        }
        local_memory(db, id).await
    }

    fn author_of(&self, here: &Local) -> Author {
        let typed = matches!(
            here.file.other.get("typed"),
            Some(serde_yaml::Value::Bool(true))
        );
        let kind = crate::authorship::memory_author(crate::authorship::MemoryWriter {
            source_id: &here.file.id,
            source_agent: here.file.agent.as_deref(),
            origin_class: here.file.origin.as_deref(),
            authored: typed,
        });
        let ai = matches!(kind, wenlan_types::authorship::SentenceAuthor::Ai);
        Author {
            member: self.settings.member_id.clone(),
            kind: if ai {
                AuthorKind::Ai
            } else {
                AuthorKind::Person
            },
            agent: if ai { here.file.agent.clone() } else { None },
        }
    }

    /// Writes the memory here as a new version over `base` (the one it was
    /// last in step with), else over the head. `None` when it could not be
    /// written as a whole version.
    async fn export(
        &self,
        id: &str,
        here: &Local,
        head: Option<&Head<'_, Arc<MemVersion>>>,
        base: Option<&Arc<MemVersion>>,
    ) -> Result<Option<String>, WenlanError> {
        let parent = base.or(head.map(|head| head.winner));
        let meta = MemMeta {
            parent: parent.map(|parent| parent.sha.clone()),
            merged: Vec::new(),
            author: self.author_of(here),
            ts: now_ts(),
            deleted: here.forgotten,
        };
        self.write(id, &here.file, &here.digest, meta).await
    }

    /// Writes a version that forgets the head, for a memory the person
    /// deleted here for good. It keeps the head's text.
    async fn export_tombstone(
        &self,
        id: &str,
        head: &Head<'_, Arc<MemVersion>>,
    ) -> Result<Option<String>, WenlanError> {
        let winner = head.winner;
        let meta = MemMeta {
            parent: Some(winner.sha.clone()),
            merged: head
                .leaves
                .iter()
                .filter(|leaf| leaf.sha != winner.sha)
                .map(|leaf| leaf.sha.clone())
                .collect(),
            author: Author {
                member: self.settings.member_id.clone(),
                kind: AuthorKind::Person,
                agent: None,
            },
            ts: now_ts(),
            deleted: true,
        };
        self.write(id, &winner.file, &winner.digest, meta).await
    }

    async fn write(
        &self,
        id: &str,
        file: &MemoryFile,
        digest: &str,
        meta: MemMeta,
    ) -> Result<Option<String>, WenlanError> {
        if !is_safe_id(id) {
            return Ok(None);
        }
        let text = render(file, &meta);
        let name = version_file_name(&sha256_hex(text.as_bytes()));
        match parse(&name, text.as_bytes()) {
            Ok(read) if read.digest == digest && read.file.text == file.text => {}
            _ => {
                log::warn!("[shared_space] memory {id} does not read back as written; not shared");
                return Ok(None);
            }
        }
        let folder = self.folder.to_path_buf();
        let member = self.settings.member_id.clone();
        let id = id.to_string();
        let (sha, _) = tokio::task::spawn_blocking(move || {
            version::write_version_under(&folder, MEMORIES_DIR, &member, &id, &text)
        })
        .await
        .map_err(|error| WenlanError::Generic(format!("shared folder task: {error}")))??;
        Ok(Some(sha))
    }
}
