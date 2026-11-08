// SPDX-License-Identifier: Apache-2.0
//! Sharing a Space's pages through a folder (v1: pages only).
//!
//! Each member runs their own Wenlan. The Space is shared through a folder
//! that a sync tool (Google Drive, iCloud) keeps the same on every member's
//! computer:
//!
//! ```text
//! <folder>/space.json                                  {"format":1,"space":…,"members":[{"id":…,"name":…}]}
//! <folder>/members/<member_id>/pages/<page_id>/<sha256>.md   one file per page version
//! ```
//!
//! A member's Wenlan writes only under its own `members/<member_id>/` (and
//! adds itself to `space.json` the first time). It reads every member's
//! versions and works out each page's latest version (`version::head`) on
//! its own. Nothing but the shared Space's pages goes into the folder: no
//! memories, no other Space, no page without a Space, and the sources block
//! is rewritten first (`rewrite`).
//!
//! A version's content is the page's title, body and whether it is
//! archived ([`version::State`]). For each page of the Space here (active or
//! archived) and in the folder, a pass:
//!
//! 1. Finds the folder's head of the page.
//! 2. Finds the version this page was last in step with: the one this
//!    Wenlan remembers (`Held`), or, when it remembers none, the newest
//!    version with this page's content.
//! 3. Export: when the page changed here since that version, writes it as a
//!    new version over it. An edit made over an older version than the head
//!    is a second leaf: an edit made at the same time. An archive or a
//!    restore made here is such a change too.
//! 4. Apply: when the page did not change here and the head is different,
//!    or the page is not here yet, makes the head here through the same
//!    writes the app uses: the body as `shared_person:<member>` or
//!    `shared_ai:<member>` (who wrote it is kept on each history row), then
//!    restore, rename or archive. A title another page here already has is
//!    given the writer's name, and that title is shared as an edit made
//!    here, so neither page is lost and every member ends with the same.
//!
//! A page the person deleted here is not made again: the next pass writes a
//! version that archives it for the others (they can restore it), and only
//! a restore made after that brings it back here.
//!
//! What this Wenlan remembers per page (the folder version and the page's
//! version here at that point) is in the database, not the folder or the
//! settings file: a rebuilt database remembers nothing, so it also had
//! nothing deleted, and it takes the folder's pages by content as a first
//! pass does, without writing a version. Remembering it in the settings
//! file instead would make a rebuilt database look like every page had been
//! deleted here.
//!
//! The daemon runs a pass of every shared Space about every 30 seconds
//! ([`sync_shared_spaces`]); the route runs one when asked ([`sync_space`]).
//! Passes of one Space never overlap. A folder that is not there or cannot
//! be read pauses the Space; nothing is changed until it is back.
//!
//! Sentences another member wrote, in person or through their AI tool, are
//! protected here like the person's own (`authorship::row_is_protected`):
//! an AI here can only propose a change to them. The person here can still
//! edit them, and that line is then theirs.
//!
//! This Wenlan's member id, name and folder for each shared Space are kept in
//! `shared-spaces.json` under the data root, outside the database, so a
//! rebuilt database stays the same member.

pub mod rewrite;
pub mod version;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use serde::{Deserialize, Serialize};
use wenlan_types::shared_space::{SharedSyncCounts, SharedSyncRun, SharedSyncStatus};

use crate::db::{MemoryDB, UNFILED_SPACE_ID};
use crate::error::WenlanError;
use crate::pages::Page;
use crate::read_scope::ReadScope;
use crate::store::format::page::{Basis, Citation, PageFile, PageKind};
use version::{Author, AuthorKind, PageVersion, VersionMeta};

/// The folder format this code reads and writes.
pub const FORMAT: u32 = 1;
/// The file at the folder's top that names the Space and its members.
pub const SPACE_FILE: &str = "space.json";
/// This Wenlan's shared Spaces, under the data root.
pub const SETTINGS_FILE: &str = "shared-spaces.json";

/// `page_history.edited_by` of a version another member wrote: the prefix,
/// then `:` and their member id.
pub const SHARED_PERSON: &str = "shared_person";
pub const SHARED_AI: &str = "shared_ai";

/// Who wrote a version that came from another member, from its
/// `page_history.edited_by`: `shared_person:<member>` or
/// `shared_ai:<member>`. `None` for every other writer.
pub fn shared_writer(edited_by: &str) -> Option<(AuthorKind, &str)> {
    let (kind, member) = edited_by.split_once(':')?;
    let kind = match kind {
        SHARED_PERSON => AuthorKind::Person,
        SHARED_AI => AuthorKind::Ai,
        _ => return None,
    };
    version::is_safe_id(member).then_some((kind, member))
}

/// The `edited_by` that records a version `author` wrote.
pub fn shared_edited_by(author: &Author) -> String {
    let kind = match author.kind {
        AuthorKind::Person => SHARED_PERSON,
        AuthorKind::Ai => SHARED_AI,
    };
    format!("{kind}:{}", author.member)
}

// ===== This Wenlan's settings =====

/// This Wenlan's part in one shared Space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareSettings {
    pub member_id: String,
    pub member_name: String,
    pub folder: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SettingsStore {
    #[serde(default)]
    spaces: BTreeMap<String, ShareSettings>,
}

/// `shared-spaces.json` under the data root.
pub fn settings_path() -> PathBuf {
    crate::config::data_root().join(SETTINGS_FILE)
}

fn read_settings(path: &Path) -> Result<SettingsStore, WenlanError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(serde_json::from_str(&text)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SettingsStore::default()),
        Err(error) => Err(error.into()),
    }
}

/// This Wenlan's settings for `space`, if it shares it.
pub fn share_settings_at(path: &Path, space: &str) -> Result<Option<ShareSettings>, WenlanError> {
    Ok(read_settings(path)?.spaces.remove(space))
}

/// Records that this Wenlan shares `space` through `folder` as
/// `member_name`. The member id is made once and kept.
pub fn join_at(
    path: &Path,
    space: &str,
    folder: &Path,
    member_name: &str,
) -> Result<ShareSettings, WenlanError> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| WenlanError::Validation(format!("no folder for {}", path.display())))?;
    std::fs::create_dir_all(parent)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    let mut store = read_settings(path)?;
    let folder = folder.to_string_lossy().into_owned();
    let settings = match store.spaces.get(space) {
        Some(existing) => ShareSettings {
            member_id: existing.member_id.clone(),
            member_name: member_name.to_string(),
            folder,
        },
        None => ShareSettings {
            member_id: uuid::Uuid::new_v4().simple().to_string(),
            member_name: member_name.to_string(),
            folder,
        },
    };
    if store.spaces.get(space) != Some(&settings) {
        store.spaces.insert(space.to_string(), settings.clone());
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        tmp.write_all(serde_json::to_string_pretty(&store)?.as_bytes())?;
        tmp.flush()?;
        tmp.persist(path)
            .map_err(|error| WenlanError::Io(error.error))?;
    }
    Ok(settings)
}

// ===== space.json =====

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceMember {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceFile {
    pub format: u32,
    pub space: String,
    pub members: Vec<SpaceMember>,
}

/// Reads `space.json`, making it when the folder has none, and adds this
/// member (or their new name). A file this code cannot read is left as it
/// is and refused.
pub fn ensure_space_file(
    folder: &Path,
    space: &str,
    me: &SpaceMember,
) -> Result<SpaceFile, WenlanError> {
    use std::io::Write;
    std::fs::create_dir_all(folder)?;
    let path = folder.join(SPACE_FILE);
    let mut file = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<SpaceFile>(&bytes).map_err(|error| {
            WenlanError::Validation(format!("{} is not a shared Space: {error}", path.display()))
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SpaceFile {
            format: FORMAT,
            space: space.to_string(),
            members: Vec::new(),
        },
        Err(error) => return Err(error.into()),
    };
    if file.format != FORMAT {
        return Err(WenlanError::Validation(format!(
            "{} is format {}; this Wenlan reads format {FORMAT}",
            path.display(),
            file.format
        )));
    }
    let before = file.clone();
    match file.members.iter_mut().find(|member| member.id == me.id) {
        Some(member) => member.name = me.name.clone(),
        None => file.members.push(me.clone()),
    }
    if file != before || !path.exists() {
        let mut tmp = tempfile::NamedTempFile::new_in(folder)?;
        tmp.write_all((serde_json::to_string_pretty(&file)? + "\n").as_bytes())?;
        tmp.flush()?;
        tmp.persist(&path)
            .map_err(|error| WenlanError::Io(error.error))?;
    }
    Ok(file)
}

// ===== What this Wenlan last held of each page =====

/// For each page of a shared Space, the folder version this Wenlan was last
/// in step with and the page's version here at that point, kept in the
/// database (`app_metadata`). It tells an edit made here from an older
/// version the page still holds, an archive or restore made here from one
/// made elsewhere, and a page the person deleted here from one this Wenlan
/// never had. A rebuilt database has none of it, and then had nothing
/// deleted: the pass falls back to comparing the page with the folder's
/// versions, as a first pass does, so nothing is applied twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Held {
    sha: String,
    /// `None` once the page was deleted here and a version that archives it
    /// was written for the others.
    #[serde(default)]
    version: Option<i64>,
}

fn held_key(space: &str) -> String {
    format!("shared_space.held:{space}")
}

async fn read_held(db: &MemoryDB, space: &str) -> Result<BTreeMap<String, Held>, WenlanError> {
    let Some(raw) = db.get_app_metadata(&held_key(space)).await? else {
        return Ok(BTreeMap::new());
    };
    Ok(serde_json::from_str(&raw).unwrap_or_else(|error| {
        log::warn!("[shared_space] what {space} last held cannot be read ({error}); starting over");
        BTreeMap::new()
    }))
}

// ===== One pass at a time, and the last result =====

#[derive(Default)]
struct SpaceRun {
    /// Held through a pass: the route waits for it, the daemon's poll skips.
    lock: tokio::sync::Mutex<()>,
    last: std::sync::Mutex<Option<SharedSyncRun>>,
}

/// One entry per shared Space of each Wenlan (its settings file).
type Runs = HashMap<(PathBuf, String), Arc<SpaceRun>>;
static RUNS: LazyLock<std::sync::Mutex<Runs>> = LazyLock::new(Default::default);

fn space_run(settings_path: &Path, space: &str) -> Arc<SpaceRun> {
    RUNS.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry((settings_path.to_path_buf(), space.to_string()))
        .or_default()
        .clone()
}

/// How a pass ended.
enum Outcome {
    Done(SharedSyncCounts),
    Paused(String),
    Failed(String),
}

/// Keeps `outcome` as the Space's last pass. A pause or a failure is logged
/// when it starts (or its reason changes), not on every poll.
fn finish(run: &SpaceRun, space: &str, outcome: Outcome) -> SharedSyncRun {
    let mut last = run
        .last
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let before = last.take();
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let next = match outcome {
        Outcome::Done(counts) => {
            if before
                .as_ref()
                .is_some_and(|run| run.paused.is_some() || run.error.is_some())
            {
                log::info!("[shared_space] {space}: syncing again");
            }
            SharedSyncRun {
                at,
                counts,
                paused: None,
                error: None,
            }
        }
        Outcome::Paused(reason) => {
            if before.as_ref().and_then(|run| run.paused.as_ref()) != Some(&reason) {
                log::warn!("[shared_space] {space} paused: {reason}");
            }
            SharedSyncRun {
                at,
                counts: SharedSyncCounts::default(),
                paused: Some(reason),
                error: None,
            }
        }
        Outcome::Failed(error) => {
            if before.as_ref().and_then(|run| run.error.as_ref()) != Some(&error) {
                log::warn!("[shared_space] {space} sync failed: {error}");
            }
            SharedSyncRun {
                at,
                counts: SharedSyncCounts::default(),
                paused: None,
                error: Some(error),
            }
        }
    };
    *last = Some(next.clone());
    next
}

/// How this Wenlan shares `space` and its last pass since the daemon
/// started; `None` when it does not share it.
pub fn share_status_at(
    settings_path: &Path,
    space: &str,
) -> Result<Option<SharedSyncStatus>, WenlanError> {
    let Some(settings) = share_settings_at(settings_path, space)? else {
        return Ok(None);
    };
    let last = space_run(settings_path, space)
        .last
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    Ok(Some(SharedSyncStatus {
        folder: settings.folder,
        member_name: settings.member_name,
        last,
    }))
}

// ===== The sync pass =====

/// Where a sync pass reads and writes.
pub struct SyncTarget<'a> {
    /// The local Space, as stored in `pages.space`.
    pub space: &'a str,
    pub folder: &'a Path,
    pub member_name: &'a str,
    /// `shared-spaces.json`; [`settings_path`] in the daemon.
    pub settings_path: &'a Path,
    /// The pages folder, so applied pages get their file too.
    pub knowledge_path: Option<&'a Path>,
}

/// One sync pass of `target.space` with its folder (see the module docs),
/// asked for by the person. The first one makes the folder's `space.json`
/// (or adds this member to it) and remembers the folder. It waits for a
/// pass of the same Space that is already running.
pub async fn sync_space(
    db: &MemoryDB,
    target: SyncTarget<'_>,
) -> Result<SharedSyncCounts, WenlanError> {
    let space = target.space.trim();
    if space.is_empty() || space == UNFILED_SPACE_ID || space == "uncategorized" {
        return Err(WenlanError::Validation(
            "only a named Space can be shared".into(),
        ));
    }
    let member_name = target.member_name.trim();
    if member_name.is_empty() {
        return Err(WenlanError::Validation("a member name is required".into()));
    }
    if !target.folder.is_absolute() {
        return Err(WenlanError::Validation(format!(
            "the shared folder must be an absolute path (got {})",
            target.folder.display()
        )));
    }
    let run = space_run(target.settings_path, space);
    let _pass = run.lock.lock().await;
    let result = async {
        let settings = join_at(target.settings_path, space, target.folder, member_name)?;
        pass(db, space, &settings, target.knowledge_path).await
    }
    .await;
    finish(
        &run,
        space,
        match &result {
            Ok(counts) => Outcome::Done(*counts),
            Err(error) => Outcome::Failed(error.to_string()),
        },
    );
    result
}

/// The daemon's automatic pass (about every 30 seconds): one sync pass of
/// every Space in `shared-spaces.json`. A Space whose pass is already
/// running is left to it. A Space whose folder is not there or cannot be
/// read is paused: nothing here is changed, and the next pass that finds
/// the folder carries on. Returns each Space's result.
pub async fn sync_shared_spaces(
    db: &MemoryDB,
    settings_path: &Path,
    knowledge_path: Option<&Path>,
) -> Vec<(String, SharedSyncRun)> {
    let store = match read_settings(settings_path) {
        Ok(store) => store,
        Err(error) => {
            log::warn!(
                "[shared_space] {} cannot be read: {error}",
                settings_path.display()
            );
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for (space, settings) in store.spaces {
        let run = space_run(settings_path, &space);
        let Ok(_pass) = run.lock.try_lock() else {
            log::debug!("[shared_space] {space}: a pass is already running");
            continue;
        };
        let outcome = match ready(db, &space, &settings).await {
            Err(reason) => Outcome::Paused(reason),
            Ok(()) => match pass(db, &space, &settings, knowledge_path).await {
                Ok(counts) => Outcome::Done(counts),
                Err(error) => Outcome::Failed(error.to_string()),
            },
        };
        out.push((space.clone(), finish(&run, &space, outcome)));
    }
    out
}

/// Whether an automatic pass of `space` may read and write its folder. A
/// folder that is not there (a drive unplugged, a cloud folder not mounted)
/// or has no readable `space.json` yet is never made here: an empty folder
/// would look like every other member's pages had gone.
async fn ready(db: &MemoryDB, space: &str, settings: &ShareSettings) -> Result<(), String> {
    match db.registered_space_or_none(Some(space)).await {
        Ok(Some(_)) => {}
        Ok(None) => return Err(format!("there is no Space named {space} here")),
        Err(error) => return Err(error.to_string()),
    }
    let folder = PathBuf::from(&settings.folder);
    tokio::task::spawn_blocking(move || folder_ready(&folder))
        .await
        .map_err(|error| format!("shared folder task: {error}"))?
}

fn folder_ready(folder: &Path) -> Result<(), String> {
    let not_readable = |what: &Path, error: std::io::Error| match error.kind() {
        std::io::ErrorKind::NotFound => format!("{} is not there", what.display()),
        _ => format!("{} cannot be read: {error}", what.display()),
    };
    let meta = std::fs::metadata(folder).map_err(|error| not_readable(folder, error))?;
    if !meta.is_dir() {
        return Err(format!("{} is not a folder", folder.display()));
    }
    let space_file = folder.join(SPACE_FILE);
    let bytes = std::fs::read(&space_file).map_err(|error| not_readable(&space_file, error))?;
    let file: SpaceFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} cannot be read: {error}", space_file.display()))?;
    if file.format != FORMAT {
        return Err(format!(
            "{} is format {}; this Wenlan reads format {FORMAT}",
            space_file.display(),
            file.format
        ));
    }
    let members = folder.join(version::MEMBERS_DIR);
    match std::fs::read_dir(&members) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(not_readable(&members, error)),
    }
}

/// The page here, as its versions are compared.
fn local_state(page: &Page) -> version::State {
    version::State {
        title: page.title.clone(),
        body_digest: version::body_digest(&page.content),
        archived: page.status == "archived",
    }
}

/// The newest version with this content.
fn newest_with<'a>(versions: &'a [PageVersion], state: &version::State) -> Option<&'a PageVersion> {
    versions
        .iter()
        .filter(|version| &version.state() == state)
        .max_by(|a, b| (&a.meta.ts, &a.sha).cmp(&(&b.meta.ts, &b.sha)))
}

/// What one pass reads once and every page uses.
struct Pass<'a> {
    db: &'a MemoryDB,
    space: &'a str,
    settings: &'a ShareSettings,
    folder: PathBuf,
    knowledge_path: Option<&'a Path>,
    /// The Space's pages here, active and archived: id to title.
    space_pages: HashMap<String, String>,
    /// Member id to name, from `space.json`.
    names: HashMap<String, String>,
}

/// One pass of `space` with the folder in `settings`.
async fn pass(
    db: &MemoryDB,
    space: &str,
    settings: &ShareSettings,
    knowledge_path: Option<&Path>,
) -> Result<SharedSyncCounts, WenlanError> {
    let folder = PathBuf::from(&settings.folder);
    let me = SpaceMember {
        id: settings.member_id.clone(),
        name: settings.member_name.clone(),
    };
    let space_file = {
        let folder = folder.clone();
        let space = space.to_string();
        tokio::task::spawn_blocking(move || ensure_space_file(&folder, &space, &me))
            .await
            .map_err(|error| WenlanError::Generic(format!("shared folder task: {error}")))??
    };
    let read = {
        let folder = folder.clone();
        tokio::task::spawn_blocking(move || version::read_folder(&folder))
            .await
            .map_err(|error| WenlanError::Generic(format!("shared folder task: {error}")))?
    };
    let mut counts = SharedSyncCounts {
        skipped: read.skipped,
        ..SharedSyncCounts::default()
    };

    let mut local: HashMap<String, Page> = HashMap::new();
    for status in ["active", "archived"] {
        for page in db
            .list_pages_by_space(status, Some(space), 1_000_000, 0)
            .await?
        {
            if page.space.as_deref() == Some(space) {
                local.insert(page.id.clone(), page);
            }
        }
    }
    let ctx = Pass {
        db,
        space,
        settings,
        folder,
        knowledge_path,
        space_pages: local
            .values()
            .map(|page| (page.id.clone(), page.title.clone()))
            .collect(),
        names: space_file
            .members
            .into_iter()
            .map(|member| (member.id, member.name))
            .collect(),
    };
    let mut held = read_held(db, space).await?;

    let ids: BTreeSet<&str> = local
        .keys()
        .map(String::as_str)
        .chain(read.pages.keys().map(String::as_str))
        .collect();
    let empty = Vec::new();
    let mut unsaved = false;
    for id in ids {
        let versions = read.pages.get(id).unwrap_or(&empty);
        let before = held.get(id).cloned();
        let exported = counts.exported;
        let now = match local.get(id) {
            Some(page) => {
                ctx.sync_page(page, versions, before.as_ref(), &mut counts)
                    .await?
            }
            None => {
                ctx.sync_missing(id, versions, before.as_ref(), &mut counts)
                    .await?
            }
        };
        if let Some(now) = now.filter(|now| before.as_ref() != Some(now)) {
            held.insert(id.to_string(), now);
            unsaved = true;
        }
        // A version written but not remembered would be written again next
        // pass, so it is kept at once; the rest once at the end (forgetting
        // an apply only means comparing by content next pass).
        if unsaved && counts.exported != exported {
            save_held(db, space, &held).await?;
            unsaved = false;
        }
    }
    if unsaved {
        save_held(db, space, &held).await?;
    }
    Ok(counts)
}

async fn save_held(
    db: &MemoryDB,
    space: &str,
    held: &BTreeMap<String, Held>,
) -> Result<(), WenlanError> {
    db.set_app_metadata(&held_key(space), &serde_json::to_string(held)?)
        .await
}

/// What an apply did.
struct Applied {
    /// Something was written here.
    wrote: bool,
    /// Every step landed; `false` when one was refused this pass.
    complete: bool,
    /// The page's version to remember with the head. The version before the
    /// apply when the title was taken here and the page got another one, so
    /// the next pass shares that title as an edit made here.
    at: i64,
}

impl Pass<'_> {
    /// A page of the Space that is here. Returns what to remember of it.
    async fn sync_page(
        &self,
        page: &Page,
        versions: &[PageVersion],
        held: Option<&Held>,
        counts: &mut SharedSyncCounts,
    ) -> Result<Option<Held>, WenlanError> {
        if page.status == "archived" && versions.is_empty() {
            // Archived before it was ever shared: it stays here only.
            return Ok(None);
        }
        let head = version::head(versions);
        let state = local_state(page);
        let recorded = held.and_then(|held| versions.iter().find(|v| v.sha == held.sha));
        // The version this page was last in step with, and whether it was
        // changed here since.
        let (base, changed) = match (held, recorded) {
            (Some(held), Some(version)) => (
                Some(version),
                held.version != Some(page.version) && version.state() != state,
            ),
            _ => match newest_with(versions, &state) {
                Some(version) => (Some(version), false),
                None => (None, true),
            },
        };
        let at = |sha: &str| Held {
            sha: sha.to_string(),
            version: Some(page.version),
        };
        if let Some(head) = head.as_ref().filter(|head| head.winner.state() == state) {
            // Already the head (an edit made here and elsewhere alike, or a
            // version taken earlier).
            return Ok(Some(at(&head.winner.sha)));
        }
        if changed {
            return match self
                .export_page(page, versions, head.as_ref(), base)
                .await?
            {
                Some(sha) => {
                    counts.exported += 1;
                    Ok(Some(at(&sha)))
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
        // This page is an earlier version, or an edit made at the same time
        // as the head that lost to it: take the head. What it had stays in
        // this page's history.
        let lost = head
            .leaves
            .iter()
            .any(|leaf| leaf.sha != head.winner.sha && leaf.state() == state);
        let applied = self.apply_version(page, head.winner).await?;
        if applied.wrote && applied.complete {
            counts.applied += 1;
            if lost {
                counts.conflicts += 1;
            }
        } else if !applied.complete {
            counts.skipped += 1;
        }
        Ok((applied.wrote || applied.complete).then(|| Held {
            sha: head.winner.sha.clone(),
            version: Some(applied.at),
        }))
    }

    /// A page in the folder that is not here as a page of this Space.
    async fn sync_missing(
        &self,
        id: &str,
        versions: &[PageVersion],
        held: Option<&Held>,
        counts: &mut SharedSyncCounts,
    ) -> Result<Option<Held>, WenlanError> {
        let Some(head) = version::head(versions) else {
            return Ok(None);
        };
        // A page with this id that is here but not in this Space is never
        // written over, and never shared.
        if self.db.get_page(id).await?.is_some() {
            log::warn!(
                "[shared_space] {id} is in the folder but here it is not a page of {}; left as it is",
                self.space
            );
            counts.skipped += 1;
            return Ok(None);
        }
        let winner = head.winner;
        let deleted = |sha: &str| Held {
            sha: sha.to_string(),
            version: None,
        };
        match held {
            // The person deleted it here. It is not made again: the others
            // get a version that archives it, which a restore undoes.
            Some(Held {
                version: Some(_), ..
            }) => {
                if winner.meta.archived {
                    return Ok(Some(deleted(&winner.sha)));
                }
                match self.export_tombstone(&head).await? {
                    Some(sha) => {
                        counts.exported += 1;
                        Ok(Some(deleted(&sha)))
                    }
                    None => {
                        counts.skipped += 1;
                        Ok(None)
                    }
                }
            }
            // Deleted here earlier: only a member who restored it since
            // brings it back.
            Some(Held { sha, version: None }) if sha == &winner.sha || winner.meta.archived => {
                Ok(Some(deleted(&winner.sha)))
            }
            // Never here, or restored elsewhere since it was deleted here.
            // An archived page that was never here stays in the folder only.
            _ if winner.meta.archived => Ok(None),
            _ => match self.apply_create(winner).await? {
                Some(at) => {
                    counts.applied += 1;
                    Ok(Some(Held {
                        sha: winner.sha.clone(),
                        version: Some(at),
                    }))
                }
                None => {
                    counts.skipped += 1;
                    Ok(None)
                }
            },
        }
    }

    /// `wanted`, or, when another active page of this Space here has that
    /// title, the title with the writer's name (and a number if that is
    /// taken too), so neither page is lost or merged.
    async fn free_title(
        &self,
        id: &str,
        wanted: &str,
        writer: &PageVersion,
    ) -> Result<String, WenlanError> {
        let taken: HashSet<String> = self
            .db
            .list_pages_by_space("active", Some(self.space), 1_000_000, 0)
            .await?
            .into_iter()
            .filter(|page| page.id != id && page.space.as_deref() == Some(self.space))
            .map(|page| MemoryDB::page_title_key(&page.title))
            .collect();
        if !taken.contains(&MemoryDB::page_title_key(wanted)) {
            return Ok(wanted.to_string());
        }
        let name = self
            .names
            .get(&writer.meta.author.member)
            .map(String::as_str)
            .unwrap_or("shared");
        let mut candidate = format!("{wanted} ({name})");
        let mut n = 2;
        while taken.contains(&MemoryDB::page_title_key(&candidate)) {
            candidate = format!("{wanted} ({name} {n})");
            n += 1;
        }
        log::info!("[shared_space] {id}: the title {wanted:?} is taken here; it is {candidate:?}");
        Ok(candidate)
    }

    async fn reload(&self, id: &str) -> Result<Page, WenlanError> {
        self.db
            .get_page(id)
            .await?
            .ok_or_else(|| WenlanError::NotFound(format!("page {id}")))
    }

    /// Makes `page` the head's content: restore, body, title, archive, each
    /// through the same path the app uses. A step refused this pass (the
    /// page moved since it was read) stops there; the next pass goes on.
    async fn apply_version(&self, page: &Page, head: &PageVersion) -> Result<Applied, WenlanError> {
        let id = page.id.as_str();
        let mut current = page.clone();
        let mut wrote = false;
        let stopped = |wrote: bool, current: &Page| Applied {
            wrote,
            complete: false,
            at: current.version,
        };
        let title = if head.meta.archived && current.status == "archived" {
            current.title.clone()
        } else {
            self.free_title(id, &head.page.title, head).await?
        };
        if current.status == "archived" && !head.meta.archived {
            let retitle = (title != current.title).then_some(title.as_str());
            let restored = self
                .db
                .restore_page(id, retitle, current.version, &ReadScope::Global)
                .await;
            match restored {
                Ok(_) => {}
                Err(crate::db::RestorePageError::Refused { code, .. }) => {
                    log::warn!("[shared_space] {id} not restored this pass: {code:?}");
                    return Ok(stopped(wrote, &current));
                }
                Err(crate::db::RestorePageError::Failed(error)) => {
                    skipped_write(id, error)?;
                    return Ok(stopped(wrote, &current));
                }
            }
            wrote = true;
            current = self.reload(id).await?;
            if let Some(root) = self.knowledge_path {
                let projection =
                    crate::export::knowledge::KnowledgeProjectionWrite::new(root.into(), self.db);
                if let Err(error) = projection.write_page_gated(self.db, &current).await {
                    log::warn!("[shared_space] restored {id} but not its file: {error}");
                }
            }
        }
        if current.status == "active"
            && version::body_digest(&current.content) != head.meta.body_digest
        {
            if !apply_update(self.db, &current, head, self.knowledge_path).await? {
                return Ok(stopped(wrote, &current));
            }
            wrote = true;
            current = self.reload(id).await?;
        }
        if current.status == "active" && current.title != title {
            let renamed = self
                .db
                .rename_page(
                    id,
                    &title,
                    current.version,
                    &ReadScope::Global,
                    self.knowledge_path,
                )
                .await;
            if let Err(error) = renamed {
                skipped_write(id, error)?;
                return Ok(stopped(wrote, &current));
            }
            wrote = true;
            current = self.reload(id).await?;
        }
        if head.meta.archived && current.status == "active" {
            let Some(version) = self.db.archive_page_at_version(id, current.version).await? else {
                log::warn!("[shared_space] {id} not archived this pass: it moved");
                return Ok(stopped(wrote, &current));
            };
            wrote = true;
            current = self.reload(id).await?;
            if let Some(root) = self.knowledge_path {
                let projection =
                    crate::export::knowledge::KnowledgeProjectionWrite::new(root.into(), self.db);
                if let Err(error) = projection
                    .archive_page_if_still_archived(self.db, id, version)
                    .await
                {
                    log::warn!("[shared_space] archived {id} but its file stays: {error}");
                }
            }
        }
        Ok(Applied {
            wrote,
            complete: true,
            at: if title == head.page.title {
                current.version
            } else {
                page.version
            },
        })
    }

    /// Makes the head here as a new page. Returns the version to remember.
    async fn apply_create(&self, head: &PageVersion) -> Result<Option<i64>, WenlanError> {
        let Some(id) = head.page.id.as_deref() else {
            return Ok(None);
        };
        let title = self.free_title(id, &head.page.title, head).await?;
        let agent = shared_edited_by(&head.meta.author);
        let distillation = crate::tuning::DistillationConfig::default();
        let write = crate::post_write::PageWrite::Create {
            page_id: Some(id),
            req: wenlan_types::requests::CreateConceptRequest {
                title: title.clone(),
                content: head.page.body.clone(),
                summary: None,
                entity_id: None,
                space: Some(self.space.to_string()).into(),
                source_memory_ids: Vec::new(),
                creation_kind: Some("authored".to_string()),
                workspace: None,
            },
            agent: &agent,
            knowledge_path: self.knowledge_path,
            page_min_cluster_size: distillation.page_min_cluster_size,
            page_match_threshold: distillation.page_match_threshold,
            citations_json: None,
            basis_marks: Vec::new(),
            basis_space_guard: None,
            skip_protected_title_match: false,
            research_write: None,
        };
        match crate::post_write::page_write(self.db, write).await {
            Ok(result) if result.wrote => {}
            Ok(_) => return Ok(None),
            Err(error) => {
                skipped_write(id, error)?;
                return Ok(None);
            }
        }
        // A page made under another title is shared under it next pass.
        let page = self.reload(id).await?;
        Ok(Some(if title == head.page.title {
            page.version
        } else {
            0
        }))
    }

    /// Writes `page` as a new version (see [`base_of`] for what it is
    /// written over) and returns its sha. `None` when it could not be
    /// written as a whole version (an id that is no folder name, a page the
    /// version file cannot carry unchanged).
    async fn export_page(
        &self,
        page: &Page,
        versions: &[PageVersion],
        head: Option<&version::Head<'_>>,
        base: Option<&PageVersion>,
    ) -> Result<Option<String>, WenlanError> {
        let db = self.db;
        if !version::is_safe_id(&page.id) {
            log::warn!(
                "[shared_space] page id {} cannot name a folder; not shared",
                page.id
            );
            return Ok(None);
        }
        let history: Vec<String> = db
            .list_page_history(&page.id, HISTORY_SCAN)
            .await?
            .iter()
            .map(|row| version::body_digest(&row.content))
            .collect();
        let (parent, merged) = base_of(&history, versions, head, base);
        let (citations, basis) = local_sources(db, page).await?;
        let memory_ids: Vec<String> = citations
            .iter()
            .filter_map(|citation| match &citation.locator {
                crate::store::format::page::Locator::Wenlan { id, .. } => Some(id.clone()),
                crate::store::format::page::Locator::Other(text) => {
                    text.strip_prefix("wenlan:").map(str::to_string)
                }
            })
            .filter(|id| !self.space_pages.contains_key(id))
            .collect();
        let cited_text = db.export_memory_texts(&memory_ids).await?;
        let ctx = rewrite::RewriteContext {
            space_pages: &self.space_pages,
            cited_text: &cited_text,
            member_name: &self.settings.member_name,
        };
        let (mut citations, mut basis) = rewrite::rewrite_sources(&citations, &basis, &ctx);
        // A source another member gave a sentence that is still on the page
        // stays: this Wenlan has no record of it, the version it came in does.
        if let Some(parent) = parent {
            let markers: HashSet<u32> = citations.iter().map(|c| c.marker).collect();
            for citation in &parent.page.citations {
                if !markers.contains(&citation.marker)
                    && page.content.contains(&format!("[{}]", citation.marker))
                {
                    citations.push(citation.clone());
                }
            }
            citations.sort_by_key(|citation| citation.marker);
            for line in &parent.page.basis {
                if !basis.iter().any(|kept| kept.locator == line.locator) {
                    basis.push(line.clone());
                }
            }
        }

        let mut file = PageFile::new(&page.title, &page.content);
        file.id = Some(page.id.clone());
        file.kind = PageKind::Page;
        file.created = Some(page.created_at.clone());
        file.modified = Some(page.last_modified.clone());
        file.citations = citations;
        file.basis = basis;
        let meta = VersionMeta {
            parent: parent.map(|parent| parent.sha.clone()),
            merged,
            body_digest: version::body_digest(&page.content),
            author: local_author(db, page, &self.settings.member_id).await?,
            ts: now_ts(),
            archived: page.status == "archived",
        };
        let text = version::render(&file, &meta);
        let name = format!("{}.md", version::sha256_hex(text.as_bytes()));
        match version::parse(&name, text.as_bytes()) {
            Ok(read) if read.page.body == page.content && read.page.title == page.title => {}
            _ => {
                log::warn!(
                    "[shared_space] {} does not read back as written; not shared",
                    page.id
                );
                return Ok(None);
            }
        }
        self.write(&page.id, text).await.map(Some)
    }

    /// Writes a version that archives the head, for a page the person
    /// deleted here. It keeps the head's title and body, so a restore
    /// elsewhere brings the page back as it was.
    async fn export_tombstone(
        &self,
        head: &version::Head<'_>,
    ) -> Result<Option<String>, WenlanError> {
        let winner = head.winner;
        let Some(id) = winner.page.id.clone().filter(|id| version::is_safe_id(id)) else {
            return Ok(None);
        };
        let meta = VersionMeta {
            parent: Some(winner.sha.clone()),
            merged: head
                .leaves
                .iter()
                .filter(|leaf| leaf.sha != winner.sha)
                .map(|leaf| leaf.sha.clone())
                .collect(),
            body_digest: winner.meta.body_digest.clone(),
            author: Author {
                member: self.settings.member_id.clone(),
                kind: AuthorKind::Person,
                agent: None,
            },
            ts: now_ts(),
            archived: true,
        };
        let text = version::render(&winner.page, &meta);
        self.write(&id, text).await.map(Some)
    }

    async fn write(&self, page_id: &str, text: String) -> Result<String, WenlanError> {
        let folder = self.folder.clone();
        let member = self.settings.member_id.clone();
        let page_id = page_id.to_string();
        let (sha, _) = tokio::task::spawn_blocking(move || {
            version::write_version(&folder, &member, &page_id, &text)
        })
        .await
        .map_err(|error| WenlanError::Generic(format!("shared folder task: {error}")))??;
        Ok(sha)
    }
}

fn now_ts() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The page's sources block as it is stored here, before the rewrite: one
/// line per marker (its first occurrence) and one per live basis.
async fn local_sources(
    db: &MemoryDB,
    page: &Page,
) -> Result<(Vec<Citation>, Vec<Basis>), WenlanError> {
    let mut stored = page.citations.clone();
    stored.sort_by_key(|citation| citation.occurrence);
    let mut seen = HashSet::new();
    let citations = stored
        .iter()
        .filter(|citation| seen.insert(citation.marker))
        .map(|citation| Citation {
            marker: citation.marker,
            label: String::new(),
            locator: crate::store::export::pages::locator_of(citation),
        })
        .collect();
    let basis = db
        .list_page_basis(&page.id)
        .await?
        .into_iter()
        .filter(|basis| matches!(basis.state.as_str(), "ok" | "pending"))
        .map(|basis| Basis {
            label: String::new(),
            locator: crate::store::format::page::Locator::Wenlan {
                id: basis.basis_page_id,
                version: basis.confirmed_version,
            },
        })
        .collect();
    Ok((citations, basis))
}

/// Who wrote the page's current version here, for the version's author.
async fn local_author(db: &MemoryDB, page: &Page, member: &str) -> Result<Author, WenlanError> {
    let edited_by = db
        .list_page_history(&page.id, 1)
        .await?
        .into_iter()
        .next()
        .map(|row| row.edited_by)
        .unwrap_or_default();
    let kind = match crate::authorship::version_author(&edited_by, &page.creation_kind) {
        crate::authorship::VersionAuthor::Ai | crate::authorship::VersionAuthor::Original => {
            AuthorKind::Ai
        }
        _ => AuthorKind::Person,
    };
    let agent = match kind {
        AuthorKind::Person => None,
        AuthorKind::Ai => page
            .changelog
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Vec<serde_json::Value>>(raw).ok())
            .and_then(|entries| {
                entries.into_iter().find(|entry| {
                    entry.get("version").and_then(serde_json::Value::as_i64) == Some(page.version)
                })
            })
            .and_then(|entry| entry.get("agent")?.as_str().map(str::to_string)),
    };
    Ok(Author {
        member: member.to_string(),
        kind,
        agent,
    })
}

/// Versions read from this page's history here, newest first.
const HISTORY_SCAN: i64 = 200;

/// The version an edit made here is written over: `known` (the version
/// this page was last in step with), else the newest version this page held
/// here found by its text in the page's history, else the head. An edit
/// made over an older version than the head is then a second leaf, an edit
/// made at the same time, not a silent overwrite. The other leaves this page
/// held here are replaced by the new version.
fn base_of<'a>(
    history: &[String],
    versions: &'a [PageVersion],
    head: Option<&version::Head<'a>>,
    known: Option<&'a PageVersion>,
) -> (Option<&'a PageVersion>, Vec<String>) {
    let newest = |digest: &String| {
        versions
            .iter()
            .filter(|version| &version.meta.body_digest == digest)
            .max_by(|a, b| (&a.meta.ts, &a.sha).cmp(&(&b.meta.ts, &b.sha)))
    };
    let parent = known
        .or_else(|| history.iter().find_map(newest))
        .or_else(|| head.map(|head| head.winner));
    let merged = head
        .map(|head| {
            head.leaves
                .iter()
                .filter(|leaf| Some(leaf.sha.as_str()) != parent.map(|p| p.sha.as_str()))
                .filter(|leaf| history.contains(&leaf.meta.body_digest))
                .map(|leaf| leaf.sha.clone())
                .collect()
        })
        .unwrap_or_default();
    (parent, merged)
}

/// A write refused for this page only (it moved since it was read, or the
/// text is not a page this Wenlan can hold) is skipped until the next pass.
fn skipped_write(id: &str, error: WenlanError) -> Result<bool, WenlanError> {
    match error {
        WenlanError::Conflict(_) | WenlanError::Validation(_) | WenlanError::NotFound(_) => {
            log::warn!("[shared_space] {id} not written this pass: {error}");
            Ok(false)
        }
        other => Err(other),
    }
}

async fn apply_update(
    db: &MemoryDB,
    page: &Page,
    head: &PageVersion,
    knowledge_path: Option<&Path>,
) -> Result<bool, WenlanError> {
    let edited_by = shared_edited_by(&head.meta.author);
    let write = crate::post_write::PageWrite::UpdatePreservingSources {
        page_id: &page.id,
        req: wenlan_types::requests::UpdatePageRequest {
            content: head.page.body.clone(),
            source_memory_ids: Vec::new(),
            expected_version: Some(page.version),
            caller_id: None,
            operation_id: None,
        },
        edited_by: &edited_by,
        knowledge_path,
        basis: crate::db::BasisWrite::default(),
    };
    match crate::post_write::page_write(db, write).await {
        Ok(result) => Ok(result.wrote),
        Err(error) => skipped_write(&page.id, error),
    }
}

#[cfg(test)]
#[path = "shared_space_test.rs"]
mod tests;

#[cfg(test)]
#[path = "sync_test.rs"]
mod sync_tests;
