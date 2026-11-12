// SPDX-License-Identifier: Apache-2.0
//! Sharing a Space's pages, or one page, through a folder (v1: pages only).
//!
//! Each member runs their own Wenlan. A Space, or one page ([`Scope`]), is
//! shared through a folder that a sync tool (Google Drive, iCloud) keeps the
//! same on every member's computer:
//!
//! ```text
//! <folder>/space.json                                  {"format":1,"space":…,"page":…,"members":[{"id":…,"name":…}]}
//! <folder>/members/<member_id>/pages/<page_id>/<sha256>.wlv  one file per page version (`.md` from older Wenlans is still read)
//! ```
//!
//! `page` is there only when the folder shares one page: its id. `space` is
//! then the page's title when it was first shared, for showing only. Each
//! member keeps a shared page in whichever Space they like (a member who
//! joins has it with no Space); moving it never stops the sharing.
//!
//! A member's Wenlan writes only under its own `members/<member_id>/` (and
//! adds itself to `space.json` the first time). It reads every member's
//! versions and works out each page's latest version (`version::head`) on
//! its own. Nothing but the shared Space's pages (or the one shared page)
//! goes into the folder: no memories, no other page, and the sources block
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
//! A third scope, [`Scope::Library`], shares the whole library between one
//! person's own computers: one folder holding every page of every Space here
//! (Unfiled and archived too), except the pages of a Space that has its own
//! share here and pages that have their own share here: those stay in their
//! own folder, so other people's sentences are never filed under this
//! person's id. Each version then carries the page's Space name in its
//! frontmatter (`space`; none for Unfiled), and the Space is part of what is
//! compared: moving a page to another Space is a change that reaches the
//! other computer, and a Space that is not there yet is made. The folder's
//! `space.json` says `"library": true`. The second computer joins with the
//! member id found there (one person, one id).
//!
//! Only the library also carries the person's memories (`memories`): one
//! `members/<member>/memories/<memory_id>/<sha256>.wlv` per version, picked
//! and applied by the same rules after the pages. A Space or a page share
//! never does, and the team server's reader walks `pages/` only.
//!
//! This Wenlan's member id, name and folder for each shared Space and page
//! are kept in `shared-spaces.json` under the data root, outside the
//! database, so a rebuilt database stays the same member.

mod follow;
pub mod library;
mod memories;
mod merge;
pub mod remote;
mod review;
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

/// `page_history.edited_by` of a version this person wrote on another of
/// their own computers (the same member id): their own sentences, and
/// their own AI tool's, never another member's.
pub const OWN_DEVICE_PERSON: &str = "own_device";
pub const OWN_DEVICE_AI: &str = "own_device_ai";

/// The key the library's pass and what it last held are kept under.
pub const LIBRARY_KEY: &str = "library:";
/// `space.json`'s name for a library folder, for showing only.
pub const LIBRARY_NAME: &str = "Wenlan library";

/// What a folder shares.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    /// Every page of the Space with this name here.
    Space(String),
    /// One page, by id, in whichever Space it is here.
    Page(String),
    /// Every page of every Space here, between one person's own computers
    /// (not the pages of a Space or a page shared on its own).
    Library,
}

impl Scope {
    /// The key its pass and what it last held are kept under. A Space keeps
    /// its name, as before pages could be shared alone.
    pub fn key(&self) -> String {
        match self {
            Scope::Space(name) => name.clone(),
            Scope::Page(id) => format!("page:{id}"),
            Scope::Library => LIBRARY_KEY.to_string(),
        }
    }
}

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

/// The `edited_by` that records, here, a version `author` wrote, when this
/// Wenlan is member `me`: the person's own (`OWN_DEVICE_*`) when they wrote
/// it on another of their computers, else that member's.
pub fn writer_edited_by(author: &Author, me: &str) -> String {
    if author.member == me {
        return match author.kind {
            AuthorKind::Person => OWN_DEVICE_PERSON,
            AuthorKind::Ai => OWN_DEVICE_AI,
        }
        .to_string();
    }
    shared_edited_by(author)
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
///
/// `member_id` is the person, not the computer: every Wenlan of one person
/// that joins the folder as the same member writes under the same
/// `members/<member_id>/`, and takes the versions found there as the
/// person's own (`OWN_DEVICE_*`). A team server gives the id; a folder
/// shared between people takes the one picked on joining (the person's
/// other computer), else a new one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareSettings {
    pub member_id: String,
    pub member_name: String,
    pub folder: String,
    /// The team server this Space syncs through; `folder` is then this
    /// Wenlan's copy of the server's folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostLink>,
    /// The team server lets this person read only: edits made here are not
    /// written as versions, and the server's version is taken over them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    /// This Wenlan hosts the Space for a team (`crate::team`): it writes
    /// the folder for every member, and a team member's AI tool's edit
    /// made here through the team listener is filed under that member.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hosted: bool,
    /// This computer's id in the library's `space.json` `devices`: made once
    /// when it first shares the library, never for a Space or a page.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub device_id: String,
}

/// A team server a Space syncs through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostLink {
    /// The server's address, such as `https://wenlan.example.com`.
    pub url: String,
    /// This person's member key.
    pub key: String,
    /// The Space's name on the server.
    pub space: String,
    /// The Space's id on the server, which a rename there keeps: the Space
    /// is followed by it when its name changes. Unknown until the server
    /// first sends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SettingsStore {
    #[serde(default)]
    spaces: BTreeMap<String, ShareSettings>,
    /// Pages shared alone, by page id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pages: BTreeMap<String, ShareSettings>,
    /// The whole library, shared between this person's own computers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    library: Option<ShareSettings>,
    /// Team Spaces this person stopped syncing here ([`left_key`]): they
    /// are not followed again until joined by hand.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    left: BTreeSet<String>,
    /// The team each server this Wenlan syncs through is for, by the
    /// server's url, as the server last said ([`follow::follow_teams`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    teams: BTreeMap<String, wenlan_types::team::TeamInfo>,
    /// The teams this person is on, by the server's url, with their key:
    /// kept from the invite link, so a team with no Space open to them yet
    /// is still followed, and its owner can manage it from here.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    memberships: BTreeMap<String, Membership>,
}

/// This person on one team server ([`SettingsStore::memberships`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    /// This person's member key.
    pub key: String,
    pub member_id: String,
    /// They own the team: they manage it from this computer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub owner: bool,
}

/// Keeps that this person is on the team at `url` as `me` says, with
/// `key`.
pub fn remember_membership(
    path: &Path,
    url: &str,
    key: &str,
    me: &wenlan_types::team::TeamMe,
) -> Result<(), WenlanError> {
    let membership = Membership {
        key: key.to_string(),
        member_id: me.member_id.clone(),
        owner: me.role == wenlan_types::team::TeamRole::Owner,
    };
    change_settings(path, |store| {
        let mut changed = false;
        if store.memberships.get(url) != Some(&membership) {
            store.memberships.insert(url.to_string(), membership);
            changed = true;
        }
        if let Some(team) = &me.team {
            if store.teams.get(url) != Some(team) {
                store.teams.insert(url.to_string(), team.clone());
                changed = true;
            }
        }
        (changed, ())
    })
}

/// Forgets the team at `url` (this person left it).
pub fn forget_membership(path: &Path, url: &str) -> Result<(), WenlanError> {
    change_settings(path, |store| (store.memberships.remove(url).is_some(), ()))
}

/// Follows every team this Wenlan is on now, as a pass does: a Space the
/// team just opened to this person is synced from now on.
pub async fn follow_teams_now(db: &MemoryDB, settings_path: &Path) {
    follow::follow_teams(db, settings_path).await;
}

/// The team servers this person is on, by url.
pub fn memberships_at(path: &Path) -> Result<BTreeMap<String, Membership>, WenlanError> {
    Ok(read_settings(path)?.memberships)
}

/// The team this person owns on another computer: its server's url and
/// their key. `None` when they own none.
pub fn owned_team(path: &Path) -> Result<Option<(String, String)>, WenlanError> {
    Ok(read_settings(path)?
        .memberships
        .into_iter()
        .find(|(_, membership)| membership.owner)
        .map(|(url, membership)| (url, membership.key)))
}

/// One team Space on one server, as [`SettingsStore::left`] names it.
fn left_key(url: &str, space: &str) -> String {
    format!("{url}\n{space}")
}

/// The same Space by its id on the server, which a rename there keeps.
fn left_id_key(url: &str, id: &str) -> String {
    format!("{url}\n\n{id}")
}

/// Whether this person stopped syncing the team Space `space` (with the
/// server's `id` for it, when known) here: by its name, or by its id under
/// any name it had.
fn was_left(left: &BTreeSet<String>, url: &str, space: &str, id: Option<&str>) -> bool {
    left.contains(&left_key(url, space))
        || id.is_some_and(|id| left.contains(&left_id_key(url, id)))
}

impl SettingsStore {
    fn get(&self, scope: &Scope) -> Option<&ShareSettings> {
        match scope {
            Scope::Space(name) => self.spaces.get(name),
            Scope::Page(id) => self.pages.get(id),
            Scope::Library => self.library.as_ref(),
        }
    }

    fn get_mut(&mut self, scope: &Scope) -> Option<&mut ShareSettings> {
        match scope {
            Scope::Space(name) => self.spaces.get_mut(name),
            Scope::Page(id) => self.pages.get_mut(id),
            Scope::Library => self.library.as_mut(),
        }
    }

    fn insert(&mut self, scope: &Scope, settings: ShareSettings) {
        match scope {
            Scope::Space(name) => {
                self.spaces.insert(name.clone(), settings);
            }
            Scope::Page(id) => {
                self.pages.insert(id.clone(), settings);
            }
            Scope::Library => self.library = Some(settings),
        }
    }

    fn remove(&mut self, scope: &Scope) -> Option<ShareSettings> {
        match scope {
            Scope::Space(name) => self.spaces.remove(name),
            Scope::Page(id) => self.pages.remove(id),
            Scope::Library => self.library.take(),
        }
    }

    /// Spaces first, then pages, the library last: the passes of what has
    /// its own folder run before the one that leaves them out.
    fn all(self) -> Vec<(Scope, ShareSettings)> {
        self.spaces
            .into_iter()
            .map(|(name, settings)| (Scope::Space(name), settings))
            .chain(
                self.pages
                    .into_iter()
                    .map(|(id, settings)| (Scope::Page(id), settings)),
            )
            .chain(self.library.map(|settings| (Scope::Library, settings)))
            .collect()
    }
}

/// The name this person set in their profile, if any: what the others see.
pub async fn person_name(db: &MemoryDB) -> Result<Option<String>, WenlanError> {
    Ok(db
        .get_profile()
        .await?
        .and_then(|profile| profile.display_name)
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty()))
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

/// This Wenlan's settings for `scope`, if it shares it.
pub fn share_settings_at(path: &Path, scope: &Scope) -> Result<Option<ShareSettings>, WenlanError> {
    Ok(read_settings(path)?.remove(scope))
}

/// Everything this Wenlan shares, Spaces first.
pub fn shares_at(path: &Path) -> Result<Vec<(Scope, ShareSettings)>, WenlanError> {
    Ok(read_settings(path)?.all())
}

/// Takes the settings file, changes it under its lock with `change`, and
/// writes it back when `change` returns true.
fn change_settings<T>(
    path: &Path,
    change: impl FnOnce(&mut SettingsStore) -> (bool, T),
) -> Result<T, WenlanError> {
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
    let (changed, out) = change(&mut store);
    if changed {
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        tmp.write_all(serde_json::to_string_pretty(&store)?.as_bytes())?;
        tmp.flush()?;
        tmp.persist(path)
            .map_err(|error| WenlanError::Io(error.error))?;
    }
    Ok(out)
}

/// This computer's id in a library folder, made once and kept beside the
/// sharing settings: the library shared again from here (stopped and
/// started, or another folder) is the same computer, so it stays the main
/// one when it was.
fn this_computer_id(settings_path: &Path) -> String {
    let file = settings_path.with_file_name("this-computer-id");
    if let Ok(id) = std::fs::read_to_string(&file) {
        let id = id.trim();
        if version::is_safe_id(id) && !id.is_empty() {
            return id.to_string();
        }
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    if let Err(error) = std::fs::write(&file, &id) {
        log::warn!("[shared_space] this computer's id is not kept ({error})");
    }
    id
}

/// Stops sharing `scope` from here: this Wenlan forgets the folder and
/// stops its passes. The pages here and everything in the folder stay as
/// they are. Returns whether it was shared.
pub fn leave_at(path: &Path, scope: &Scope) -> Result<bool, WenlanError> {
    change_settings(path, |store| {
        let removed = store.remove(scope);
        if let Some(host) = removed.as_ref().and_then(|settings| settings.host.as_ref()) {
            store.left.insert(left_key(&host.url, &host.space));
            if let Some(id) = &host.space_id {
                store.left.insert(left_id_key(&host.url, id));
            }
        }
        (removed.is_some(), removed.is_some())
    })
}

/// Moves what this Wenlan remembers of the Space `from` to `to`: the Space
/// was renamed here, and it keeps its folder, member and team server.
/// Returns whether it was shared. Refused when `to` is shared already.
pub fn rename_share(path: &Path, from: &str, to: &str) -> Result<bool, WenlanError> {
    if from == to {
        return Ok(false);
    }
    change_settings(path, |store| {
        if store.spaces.contains_key(to) {
            return (
                false,
                Err(WenlanError::Validation(format!(
                    "{to} is shared from here already"
                ))),
            );
        }
        match store.spaces.remove(from) {
            Some(settings) => {
                store.spaces.insert(to.to_string(), settings);
                (true, Ok(true))
            }
            None => (false, Ok(false)),
        }
    })?
}

/// Forgets that this person left `space` on the server at `url`: they
/// joined it again.
fn rejoined(path: &Path, host: &HostLink) -> Result<(), WenlanError> {
    change_settings(path, |store| {
        let by_name = store.left.remove(&left_key(&host.url, &host.space));
        let by_id = host
            .space_id
            .as_deref()
            .is_some_and(|id| store.left.remove(&left_id_key(&host.url, id)));
        (by_name || by_id, ())
    })
}

/// Records that this Wenlan shares `scope` through `folder` as
/// `member_name`. The member id is made once and kept.
pub fn join_at(
    path: &Path,
    scope: &Scope,
    folder: &Path,
    member_name: &str,
) -> Result<ShareSettings, WenlanError> {
    join_as(path, scope, folder, member_name, None)
}

/// [`join_at`], joining as `member_id` when this Wenlan does not share
/// `scope` yet: the person's own other computer, already a member of the
/// folder, or the id a team server gave. A share that already has another
/// member id is refused: what it remembers was written as that member.
pub fn join_as(
    path: &Path,
    scope: &Scope,
    folder: &Path,
    member_name: &str,
    member_id: Option<&str>,
) -> Result<ShareSettings, WenlanError> {
    if let Some(id) = member_id {
        if !version::is_safe_id(id) {
            return Err(WenlanError::Validation(format!("{id} is not a member id")));
        }
    }
    let folder = folder.to_string_lossy().into_owned();
    change_settings(path, |store| {
        let settings = match store.get(scope) {
            Some(existing) => {
                if member_id.is_some_and(|id| id != existing.member_id) {
                    return (
                        false,
                        Err(WenlanError::Validation(
                            "this is shared from here as another member already; stop sharing it first"
                                .into(),
                        )),
                    );
                }
                ShareSettings {
                    member_name: member_name.to_string(),
                    folder: folder.clone(),
                    ..existing.clone()
                }
            }
            None => ShareSettings {
                member_id: member_id
                    .map(str::to_string)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string()),
                member_name: member_name.to_string(),
                folder,
                host: None,
                read_only: false,
                hosted: false,
                device_id: String::new(),
            },
        };
        let mut settings = settings;
        if matches!(scope, Scope::Library) && settings.device_id.is_empty() {
            settings.device_id = this_computer_id(path);
        }
        let changed = store.get(scope) != Some(&settings);
        if changed {
            store.insert(scope, settings.clone());
        }
        (changed, Ok(settings))
    })?
}

/// Changes what this Wenlan remembers of `scope` beside its folder and
/// member (the team server, read only, hosted). Returns the settings, or
/// `None` when it does not share `scope`.
pub fn update_share(
    path: &Path,
    scope: &Scope,
    change: impl FnOnce(&mut ShareSettings),
) -> Result<Option<ShareSettings>, WenlanError> {
    change_settings(path, |store| {
        let Some(settings) = store.get_mut(scope) else {
            return (false, None);
        };
        let before = settings.clone();
        change(settings);
        let after = settings.clone();
        (before != after, Some(after))
    })
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
    /// The Space's name, or the shared page's title when it was first
    /// shared.
    pub space: String,
    /// The shared page's id when the folder shares one page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    /// The folder shares the whole library ([`Scope::Library`]); `space` is
    /// then a name for showing only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub library: bool,
    pub members: Vec<SpaceMember>,
    /// The computers of a library folder, each added once by the computer
    /// itself (never rewritten, so a sync tool sees no conflicting copies).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<SpaceDevice>,
    /// The id of the computer that writes the phone read copy, takes the
    /// inbox and runs background AI page writing. The first computer to
    /// share the library; changed by the person.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub main_device: String,
}

/// A computer of a shared library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceDevice {
    pub id: String,
    pub name: String,
}

impl SpaceFile {
    /// What the folder shares, with `space` naming the Space here.
    fn scope_check(&self, scope: &Scope) -> Result<(), String> {
        match (scope, self.page.as_deref()) {
            (Scope::Library, None) if self.library => return Ok(()),
            (Scope::Library, _) => {
                return Err("it shares a Space or a page, not the library".into())
            }
            _ if self.library => {
                return Err("it shares the whole library, not a Space or a page".into())
            }
            _ => {}
        }
        match (scope, self.page.as_deref()) {
            (Scope::Space(_), None) => Ok(()),
            (Scope::Page(id), Some(page)) if page == id => Ok(()),
            (Scope::Space(_), Some(_)) => Err("it shares one page, not a Space".into()),
            (Scope::Page(_), None) => Err("it shares a whole Space, not this page".into()),
            (Scope::Page(_), Some(_)) => Err("it shares another page".into()),
            (Scope::Library, _) => unreachable!("a library scope is decided above"),
        }
    }
}

/// `space.json` of `folder`: `None` when the folder has none. A file this
/// code cannot read is an error.
pub fn read_space_file(folder: &Path) -> Result<Option<SpaceFile>, WenlanError> {
    let path = folder.join(SPACE_FILE);
    let file = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<SpaceFile>(&bytes).map_err(|error| {
            WenlanError::Validation(format!("{} is not a shared Space: {error}", path.display()))
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if file.format != FORMAT {
        return Err(WenlanError::Validation(format!(
            "{} is format {}; this Wenlan reads format {FORMAT}",
            path.display(),
            file.format
        )));
    }
    Ok(Some(file))
}

/// Reads `space.json`, making it when the folder has none (`name` is the
/// Space's name or the page's title), and adds this member (or their new
/// name). A file this code cannot read, or one that shares something else
/// than `scope`, is left as it is and refused.
pub fn ensure_space_file(
    folder: &Path,
    scope: &Scope,
    name: &str,
    me: &SpaceMember,
) -> Result<SpaceFile, WenlanError> {
    use std::io::Write;
    std::fs::create_dir_all(folder)?;
    let path = folder.join(SPACE_FILE);
    let mut file = match read_space_file(folder)? {
        Some(file) => file,
        None => SpaceFile {
            format: FORMAT,
            space: name.to_string(),
            page: match scope {
                Scope::Space(_) | Scope::Library => None,
                Scope::Page(id) => Some(id.clone()),
            },
            library: matches!(scope, Scope::Library),
            members: Vec::new(),
            devices: Vec::new(),
            main_device: String::new(),
        },
    };
    file.scope_check(scope).map_err(|reason| {
        WenlanError::Validation(format!(
            "{} cannot be used here: {reason}",
            folder.display()
        ))
    })?;
    let before = file.clone();
    // An entry already there keeps its name: two computers of one person
    // share the id and may have different profile names, and a pass must
    // not rename them back and forth. A real rename is `rename_space_member`.
    if !file.members.iter().any(|member| member.id == me.id) {
        file.members.push(me.clone());
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

/// Sets `me`'s name in the folder's `space.json`, which must be there
/// already: the person changed the name others see.
pub fn rename_space_member(folder: &Path, me: &SpaceMember) -> Result<(), WenlanError> {
    add_space_members(folder, std::slice::from_ref(me))
}

/// Adds `members` (or their new names) to the folder's `space.json`, which
/// must be there already. Members not named stay. For a team server, which
/// lists everyone who can use a Space it hosts.
pub fn add_space_members(folder: &Path, members: &[SpaceMember]) -> Result<(), WenlanError> {
    use std::io::Write;
    let path = folder.join(SPACE_FILE);
    let Some(mut file) = read_space_file(folder)? else {
        return Err(WenlanError::NotFound(format!("{}", path.display())));
    };
    let before = file.clone();
    for wanted in members {
        match file
            .members
            .iter_mut()
            .find(|member| member.id == wanted.id)
        {
            Some(member) => member.name = wanted.name.clone(),
            None => file.members.push(wanted.clone()),
        }
    }
    if file != before {
        let mut tmp = tempfile::NamedTempFile::new_in(folder)?;
        tmp.write_all((serde_json::to_string_pretty(&file)? + "\n").as_bytes())?;
        tmp.flush()?;
        tmp.persist(&path)
            .map_err(|error| WenlanError::Io(error.error))?;
    }
    Ok(())
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
    /// The library: the page's Space here at that point (`""` is Unfiled).
    /// A Space renamed or deleted here moves its pages without a new page
    /// version; this tells that move from one still to be taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    space: Option<String>,
}

fn held_key(key: &str) -> String {
    format!("shared_space.held:{key}")
}

/// What was held is of the folder it was held with: shared again through
/// another folder (moved, or chosen again), this Wenlan starts over there,
/// comparing by content as a first pass does. Without it every page here
/// would read as in step with versions the new folder does not have, and
/// nothing would be written there.
pub(super) async fn forget_held_of_other_folder(
    db: &MemoryDB,
    held_key: &str,
    folder: &Path,
) -> Result<(), WenlanError> {
    let marker = format!("{held_key}.folder");
    let folder = folder.to_string_lossy().to_string();
    match db.get_app_metadata(&marker).await? {
        Some(known) if known == folder => return Ok(()),
        Some(_) => {
            log::info!("[shared_space] {held_key}: shared through another folder; starting over");
            db.set_app_metadata(held_key, "{}").await?;
        }
        None => {}
    }
    db.set_app_metadata(&marker, &folder).await
}

/// The Space a library page is kept in here, as [`Held::space`] holds it.
fn held_space(page: &Page) -> String {
    named_space(page.space.as_deref()).unwrap_or("").to_string()
}

/// Whether `version` is `ancestor` or comes after it: `ancestor` is on its
/// line of parents (or merged into it). A parent whose file cannot be read
/// still names its sha, so a version can be known to come after one that is
/// not in the folder this pass.
fn comes_after(versions: &[PageVersion], version: &PageVersion, ancestor: &str) -> bool {
    let mut seen = HashSet::new();
    let mut todo = vec![version];
    while let Some(at) = todo.pop() {
        if at.sha == ancestor {
            return true;
        }
        for parent in at.meta.parent.iter().chain(at.meta.merged.iter()) {
            if parent == ancestor {
                return true;
            }
            if seen.insert(parent.as_str()) {
                if let Some(next) = versions.iter().find(|v| &v.sha == parent) {
                    todo.push(next);
                }
            }
        }
    }
    false
}

async fn read_held(db: &MemoryDB, key: &str) -> Result<BTreeMap<String, Held>, WenlanError> {
    let space = key;
    let Some(raw) = db.get_app_metadata(&held_key(key)).await? else {
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
    /// The pass running now: pages done of all it goes through.
    progress: std::sync::Mutex<Option<wenlan_types::shared_space::SharedSyncProgress>>,
}

/// Keeps a running pass's progress where its status reads it, and clears
/// it when the pass ends, however it ends.
struct Progress(Arc<SpaceRun>);

impl Progress {
    fn start(settings_path: &Path, key: &str, total: usize) -> Self {
        let progress = Progress(space_run(settings_path, key));
        progress.set(0, total);
        progress
    }

    fn set(&self, done: usize, total: usize) {
        let clamp = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        *self
            .0
            .progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(wenlan_types::shared_space::SharedSyncProgress {
                done: clamp(done),
                total: clamp(total),
            });
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        *self
            .0
            .progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

/// One entry per shared Space or page of each Wenlan (its settings file).
type Runs = HashMap<(PathBuf, String), Arc<SpaceRun>>;
static RUNS: LazyLock<std::sync::Mutex<Runs>> = LazyLock::new(Default::default);

fn space_run(settings_path: &Path, key: &str) -> Arc<SpaceRun> {
    RUNS.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry((settings_path.to_path_buf(), key.to_string()))
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
            let held_back = (counts.held_back > 0).then(|| {
                let reason = format!(
                    "{} pages that the other computer archived were left as they are: \
                     that is most of the library. Check the other computer, then sync again",
                    counts.held_back
                );
                if before.as_ref().and_then(|run| run.held_back.as_ref()) != Some(&reason) {
                    log::warn!("[shared_space] {space}: {reason}");
                }
                reason
            });
            SharedSyncRun {
                at,
                counts,
                paused: None,
                error: None,
                held_back,
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
                held_back: None,
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
                held_back: None,
            }
        }
    };
    *last = Some(next.clone());
    next
}

/// How this Wenlan shares `scope` and its last pass since the daemon
/// started; `None` when it does not share it. The members are read from
/// the folder's `space.json`; when it cannot be read, this member only.
pub fn share_status_at(
    settings_path: &Path,
    scope: &Scope,
) -> Result<Option<SharedSyncStatus>, WenlanError> {
    let mut store = read_settings(settings_path)?;
    let teams = std::mem::take(&mut store.teams);
    let Some(settings) = store.remove(scope) else {
        return Ok(None);
    };
    let team = team_of(settings_path, &teams, &settings);
    Ok(Some(status_of(settings_path, scope, settings, team)))
}

/// The team a share belongs to: the one its server last named, or the team
/// this Wenlan hosts (its member list beside the settings).
fn team_of(
    settings_path: &Path,
    teams: &BTreeMap<String, wenlan_types::team::TeamInfo>,
    settings: &ShareSettings,
) -> Option<wenlan_types::team::TeamInfo> {
    match &settings.host {
        Some(host) => teams.get(&host.url).cloned(),
        None if settings.hosted => settings_path
            .parent()
            .map(|root| root.join(crate::team::TEAM_FILE))
            .and_then(|path| crate::team::read_team(&path).ok())
            .and_then(|file| file.info()),
        None => None,
    }
}

fn status_of(
    settings_path: &Path,
    scope: &Scope,
    settings: ShareSettings,
    team: Option<wenlan_types::team::TeamInfo>,
) -> SharedSyncStatus {
    let run = space_run(settings_path, &scope.key());
    let last = run
        .last
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let progress = *run
        .progress
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let space_file = read_space_file(&folder_for(scope, &settings.folder))
        .ok()
        .flatten();
    let library = matches!(scope, Scope::Library);
    let (devices, main_device) = match (&space_file, library) {
        (Some(file), true) => (
            file.devices
                .iter()
                .map(|device| wenlan_types::shared_space::SharedDevice {
                    id: device.id.clone(),
                    name: device.name.clone(),
                })
                .collect(),
            Some(file.main_device.clone()).filter(|id| !id.is_empty()),
        ),
        _ => (Vec::new(), None),
    };
    let members = space_file
        .map(|file| file.members)
        .filter(|members| !members.is_empty())
        .unwrap_or_else(|| {
            vec![SpaceMember {
                id: settings.member_id.clone(),
                name: settings.member_name.clone(),
            }]
        });
    SharedSyncStatus {
        kind: match scope {
            Scope::Space(_) => wenlan_types::shared_space::SharedKind::Space,
            Scope::Page(_) => wenlan_types::shared_space::SharedKind::Page,
            Scope::Library => wenlan_types::shared_space::SharedKind::Library,
        },
        target: match scope {
            Scope::Space(name) | Scope::Page(name) => name.clone(),
            Scope::Library => "library".to_string(),
        },
        folder: settings.folder,
        host: settings.host.map(|host| host.url),
        team,
        read_only: settings.read_only,
        hosted: settings.hosted,
        member_id: settings.member_id,
        member_name: settings.member_name,
        members: members
            .into_iter()
            .map(|member| wenlan_types::shared_space::SharedMember {
                id: member.id,
                name: member.name,
            })
            .collect(),
        last,
        devices,
        main_device,
        this_device: (library && !settings.device_id.is_empty())
            .then(|| settings.device_id.clone()),
        reading_copy_error: if library {
            library::reading_copy_error(settings_path)
        } else {
            None
        },
        progress,
    }
}

/// Everything this Wenlan shares, with each one's members and last pass.
pub fn share_statuses_at(settings_path: &Path) -> Result<Vec<SharedSyncStatus>, WenlanError> {
    let mut store = read_settings(settings_path)?;
    let teams = std::mem::take(&mut store.teams);
    Ok(store
        .all()
        .into_iter()
        .map(|(scope, settings)| {
            let team = team_of(settings_path, &teams, &settings);
            status_of(settings_path, &scope, settings, team)
        })
        .collect())
}

/// Remembers which team the server at `url` is for, as it just said.
pub fn remember_team(
    path: &Path,
    url: &str,
    team: &wenlan_types::team::TeamInfo,
) -> Result<(), WenlanError> {
    change_settings(path, |store| {
        if store.teams.get(url) == Some(team) {
            return (false, ());
        }
        store.teams.insert(url.to_string(), team.clone());
        (true, ())
    })
}

/// What a folder shares, for joining it: `None` when it shares nothing yet.
pub fn peek_folder(
    folder: &Path,
) -> Result<Option<wenlan_types::shared_space::SharedFolder>, WenlanError> {
    // A library keeps its engine files in a subfolder of the folder the
    // person chose.
    let file = match read_space_file(folder)? {
        Some(file) => file,
        None => match read_space_file(&library::engine_folder(folder)) {
            Ok(Some(file)) if file.library => file,
            _ => return Ok(None),
        },
    };
    Ok(Some(wenlan_types::shared_space::SharedFolder {
        kind: if file.library {
            wenlan_types::shared_space::SharedKind::Library
        } else if file.page.is_some() {
            wenlan_types::shared_space::SharedKind::Page
        } else {
            wenlan_types::shared_space::SharedKind::Space
        },
        name: file.space,
        page_id: file.page,
        members: file
            .members
            .into_iter()
            .map(|member| wenlan_types::shared_space::SharedMember {
                id: member.id,
                name: member.name,
            })
            .collect(),
    }))
}

// ===== The sync pass =====

/// Where a sync pass reads and writes.
pub struct SyncTarget<'a> {
    /// The local Space (as stored in `pages.space`), or the page.
    pub scope: &'a Scope,
    pub folder: &'a Path,
    pub member_name: &'a str,
    /// `shared-spaces.json`; [`settings_path`] in the daemon.
    pub settings_path: &'a Path,
    /// The pages folder, so applied pages get their file too.
    pub knowledge_path: Option<&'a Path>,
    /// Join as this member (see [`join_as`]): the person's own other
    /// computer, or the id a team server gave.
    pub member_id: Option<&'a str>,
    /// Sync through this team server; `folder` is then this Wenlan's copy of
    /// its folder ([`remote::mirror_folder`]).
    pub host: Option<HostLink>,
}

/// One sync pass of `target.scope` with its folder (see the module docs),
/// asked for by the person. The first one makes the folder's `space.json`
/// (or adds this member to it) and remembers the folder. It waits for a
/// pass of the same Space or page that is already running.
///
/// A page is shared alone only when its Space is not shared from here, and
/// it must be here or be the page the folder shares (joining it). Sharing a
/// Space stops sharing alone the pages of it that were.
pub async fn sync_space(
    db: &MemoryDB,
    target: SyncTarget<'_>,
) -> Result<SharedSyncCounts, WenlanError> {
    let scope = match target.scope {
        Scope::Space(name) => {
            let space = name.trim();
            if space.is_empty() || space == UNFILED_SPACE_ID || space == "uncategorized" {
                return Err(WenlanError::Validation(
                    "only a named Space can be shared".into(),
                ));
            }
            Scope::Space(space.to_string())
        }
        Scope::Library => Scope::Library,
        Scope::Page(id) => {
            if !version::is_safe_id(id) {
                return Err(WenlanError::Validation(format!(
                    "page id {id} cannot name a folder"
                )));
            }
            match db.get_page(id).await? {
                Some(page) => {
                    if let Some(space) = page
                        .space
                        .as_deref()
                        .filter(|space| *space != UNFILED_SPACE_ID)
                    {
                        if share_settings_at(
                            target.settings_path,
                            &Scope::Space(space.to_string()),
                        )?
                        .is_some()
                        {
                            return Err(WenlanError::Validation(format!(
                                "the page is in {space}, which is shared already"
                            )));
                        }
                    }
                }
                None => {
                    let folder = target.folder.to_path_buf();
                    let shares = tokio::task::spawn_blocking(move || read_space_file(&folder))
                        .await
                        .map_err(|error| {
                            WenlanError::Generic(format!("shared folder task: {error}"))
                        })??
                        .is_some_and(|file| file.page.as_deref() == Some(id.as_str()));
                    if !shares {
                        return Err(WenlanError::NotFound(format!("page {id}")));
                    }
                }
            }
            target.scope.clone()
        }
    };
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
    if target.host.is_none() {
        if let Some(id) = target.member_id {
            // Joining as a member already in the folder: one of this
            // person's other computers.
            let already = share_settings_at(target.settings_path, &scope)?.is_some();
            let folder = folder_for(&scope, &target.folder.to_string_lossy());
            let listed = tokio::task::spawn_blocking(move || read_space_file(&folder))
                .await
                .map_err(|error| WenlanError::Generic(format!("shared folder task: {error}")))??
                .is_some_and(|file| file.members.iter().any(|member| member.id == id));
            if !already && !listed {
                return Err(WenlanError::Validation(format!(
                    "{id} is not a member of {}",
                    target.folder.display()
                )));
            }
        }
    }
    let key = scope.key();
    let run = space_run(target.settings_path, &key);
    let _pass = run.lock.lock().await;
    let result = async {
        let mut settings = join_as(
            target.settings_path,
            &scope,
            target.folder,
            member_name,
            target.member_id,
        )?;
        if let Some(host) = target.host.clone() {
            rejoined(target.settings_path, &host)?;
            settings = update_share(target.settings_path, &scope, |settings| {
                settings.host = Some(host);
            })?
            .unwrap_or(settings);
        }
        if let Scope::Space(space) = &scope {
            stop_page_shares_in(db, target.settings_path, space).await?;
        }
        run_pass(
            db,
            &scope,
            settings,
            target.settings_path,
            target.knowledge_path,
            false,
        )
        .await
    }
    .await;
    match result {
        Ok(Ok(counts)) => {
            finish(&run, &key, Outcome::Done(counts));
            Ok(counts)
        }
        Ok(Err(reason)) => {
            finish(&run, &key, Outcome::Paused(reason.clone()));
            Err(WenlanError::Validation(reason))
        }
        Err(error) => {
            finish(&run, &key, Outcome::Failed(error.to_string()));
            Err(error)
        }
    }
}

/// One pass of `scope`, with the team server's pull before it and push
/// after it when the Space syncs through one. `Ok(Err(reason))` when it is
/// paused: the server or the folder cannot be reached, nothing was
/// changed here. `automatic` passes also wait for the folder to be whole
/// ([`ready`]).
async fn run_pass(
    db: &MemoryDB,
    scope: &Scope,
    mut settings: ShareSettings,
    settings_path: &Path,
    knowledge_path: Option<&Path>,
    automatic: bool,
) -> Result<Result<SharedSyncCounts, String>, WenlanError> {
    let folder = folder_for(scope, &settings.folder);
    if matches!(scope, Scope::Library) {
        // Two computers that each started a library in the same folder
        // (the second one before its cloud drive had downloaded the first
        // one's) come together in one: the library of the smallest member
        // id, which every computer picks alike. This one takes that id;
        // its pages go into that library next, nothing is deleted.
        let engine = folder.clone();
        let one = tokio::task::spawn_blocking(move || read_space_file(&engine))
            .await
            .map_err(|error| WenlanError::Generic(format!("shared folder task: {error}")))?;
        if let Ok(Some(file)) = one {
            let ids: Vec<&str> = file.members.iter().map(|m| m.id.as_str()).collect();
            if let Some(id) = ids
                .iter()
                .min()
                .filter(|_| !ids.contains(&settings.member_id.as_str()))
            {
                log::info!("[shared_space] library: the folder holds another computer's library; joining it");
                let id = id.to_string();
                if let Some(now) = update_share(settings_path, scope, |settings| {
                    settings.member_id = id.clone();
                })? {
                    settings = now;
                }
            }
        }
    }
    let listed = match settings.host.clone() {
        None => None,
        Some(host) => match remote::pull(&host, &folder).await {
            Err(reason) => return Ok(Err(reason)),
            Ok(listed) => {
                let read_only = listed.access == wenlan_types::team::TeamAccess::Read;
                if read_only != settings.read_only {
                    if let Some(now) = update_share(settings_path, scope, |settings| {
                        settings.read_only = read_only;
                    })? {
                        settings = now;
                    }
                }
                Some((host, listed))
            }
        },
    };
    if automatic {
        if let Err(reason) = ready(db, scope, &settings).await {
            return Ok(Err(reason));
        }
    }
    if settings.hosted {
        if let Scope::Space(space) = scope {
            list_team_members(settings_path, &folder, space).await;
        }
    }
    let edit_settle = if automatic { EDIT_SETTLE_SECS } else { 0 };
    let counts = pass(
        db,
        scope,
        &settings,
        settings_path,
        knowledge_path,
        edit_settle,
    )
    .await?;
    if matches!(scope, Scope::Library) {
        library::main_jobs(db, settings_path, &settings, &counts).await;
    }
    if let Some((host, listed)) = listed {
        if let Err(reason) = remote::push(&host, &folder, &settings.member_id, &listed).await {
            return Ok(Err(reason));
        }
    }
    Ok(Ok(counts))
}

/// A Space this Wenlan hosts: lists everyone who can use it in its
/// `space.json` before the pass (see `crate::team`).
async fn list_team_members(settings_path: &Path, folder: &Path, space: &str) {
    let team = settings_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(crate::team::TEAM_FILE);
    let folder = folder.to_path_buf();
    let space = space.to_string();
    let done = tokio::task::spawn_blocking(move || {
        let members = crate::team::read_team(&team)?.members;
        if folder.join(SPACE_FILE).exists() {
            crate::team::list_members_in_space_file(&folder, &space, &members)?;
        }
        Ok::<(), WenlanError>(())
    })
    .await;
    match done {
        Ok(Ok(())) => {}
        Ok(Err(error)) => log::warn!("[shared_space] team members not listed: {error}"),
        Err(error) => log::warn!("[shared_space] team members not listed: {error}"),
    }
}

/// Stops sharing alone the pages of `space`, now that the whole Space is
/// shared: they go on in the Space's folder.
async fn stop_page_shares_in(
    db: &MemoryDB,
    settings_path: &Path,
    space: &str,
) -> Result<(), WenlanError> {
    for (scope, _) in shares_at(settings_path)? {
        let Scope::Page(id) = &scope else { continue };
        if db
            .get_page(id)
            .await?
            .is_some_and(|page| page.space.as_deref() == Some(space))
        {
            log::info!("[shared_space] page {id} is shared with all of {space} now; stopped sharing it alone");
            leave_at(settings_path, &scope)?;
        }
    }
    Ok(())
}

/// The daemon's automatic pass (about every 30 seconds): one sync pass of
/// every Space and page in `shared-spaces.json`. One whose pass is already
/// running is left to it. One whose folder is not there or cannot be read
/// is paused: nothing here is changed, and the next pass that finds the
/// folder carries on. `person_name` is the name in this Wenlan's profile:
/// when it is set and differs, every share takes it. Returns each one's
/// result, by [`Scope::key`].
pub async fn sync_shared_spaces(
    db: &MemoryDB,
    settings_path: &Path,
    knowledge_path: Option<&Path>,
    person_name: Option<&str>,
) -> Vec<(String, SharedSyncRun)> {
    follow::follow_teams(db, settings_path).await;
    let shares = match shares_at(settings_path) {
        Ok(shares) => shares,
        Err(error) => {
            log::warn!(
                "[shared_space] {} cannot be read: {error}",
                settings_path.display()
            );
            return Vec::new();
        }
    };
    let person_name = person_name.map(str::trim).filter(|name| !name.is_empty());
    let mut out = Vec::new();
    for (scope, mut settings) in shares {
        let key = scope.key();
        let run = space_run(settings_path, &key);
        let Ok(_pass) = run.lock.try_lock() else {
            log::debug!("[shared_space] {key}: a pass is already running");
            continue;
        };
        // A team server's member is named by the server.
        if let Some(name) = person_name
            .filter(|name| *name != settings.member_name)
            .filter(|_| settings.host.is_none())
        {
            let renamed = join_at(settings_path, &scope, Path::new(&settings.folder), name)
                .and_then(|renamed| {
                    rename_space_member(
                        &folder_for(&scope, &renamed.folder),
                        &SpaceMember {
                            id: renamed.member_id.clone(),
                            name: renamed.member_name.clone(),
                        },
                    )?;
                    Ok(renamed)
                });
            match renamed {
                Ok(renamed) => settings = renamed,
                Err(error) => log::warn!("[shared_space] {key}: name not changed: {error}"),
            }
        }
        let outcome =
            match run_pass(db, &scope, settings, settings_path, knowledge_path, true).await {
                Ok(Ok(counts)) => Outcome::Done(counts),
                Ok(Err(reason)) => Outcome::Paused(reason),
                Err(error) => Outcome::Failed(error.to_string()),
            };
        out.push((key.clone(), finish(&run, &key, outcome)));
    }
    out
}

/// Whether an automatic pass of `scope` may read and write its folder. A
/// folder that is not there (a drive unplugged, a cloud folder not mounted)
/// or has no readable `space.json` yet is never made here: an empty folder
/// would look like every other member's pages had gone.
async fn ready(db: &MemoryDB, scope: &Scope, settings: &ShareSettings) -> Result<(), String> {
    if let Scope::Space(space) = scope {
        match db.registered_space_or_none(Some(space)).await {
            Ok(Some(_)) => {}
            Ok(None) => return Err(format!("there is no Space named {space} here")),
            Err(error) => return Err(error.to_string()),
        }
    }
    let folder = folder_for(scope, &settings.folder);
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

/// A Space name that names a Space: the library's `space` of a page with
/// none (Unfiled) is left out.
fn named_space(space: Option<&str>) -> Option<&str> {
    space.map(str::trim).filter(|space| {
        !space.is_empty() && *space != UNFILED_SPACE_ID && *space != "uncategorized"
    })
}

/// What the library leaves to the folders that have their own share here:
/// the Spaces and the pages shared on their own.
fn own_shares(settings_path: &Path) -> Result<(HashSet<String>, HashSet<String>), WenlanError> {
    let mut spaces = HashSet::new();
    let mut pages = HashSet::new();
    for (scope, _) in shares_at(settings_path)? {
        match scope {
            Scope::Space(name) => {
                spaces.insert(name);
            }
            Scope::Page(id) => {
                pages.insert(id);
            }
            Scope::Library => {}
        }
    }
    Ok((spaces, pages))
}

/// The folder a pass of `scope` reads and writes: for the library, the
/// engine subfolder of the folder the person chose (`settings.folder`).
pub(crate) fn folder_for(scope: &Scope, folder: &str) -> PathBuf {
    match scope {
        Scope::Library => library::engine_folder(Path::new(folder)),
        _ => PathBuf::from(folder),
    }
}

/// A library pass that would archive more than this many pages here, and
/// more than half of the library's, archives none of them.
const MASS_ARCHIVE_MIN: usize = 20;
/// The page here, as its versions are compared.
fn local_state(page: &Page) -> version::State {
    version::State {
        title: page.title.clone(),
        body_digest: version::body_digest(&page.content),
        archived: page.status == "archived",
        locked: page.review_required,
        space: None,
    }
}

/// `page` differs from `version` in its lock only.
fn lock_only(version: &PageVersion, page: &Page) -> bool {
    let state = local_state(page);
    version.meta.locked != state.locked
        && version::State {
            locked: state.locked,
            ..version.state()
        } == state
}

/// What one pass reads once and every page uses.
struct Pass<'a> {
    db: &'a MemoryDB,
    scope: &'a Scope,
    /// The Space new pages are made in and titles are kept apart in: the
    /// shared Space, or the shared page's Space here (none when it is not
    /// here yet).
    space: String,
    settings: &'a ShareSettings,
    folder: PathBuf,
    knowledge_path: Option<&'a Path>,
    /// The Space's pages here, active and archived: id to title.
    space_pages: HashMap<String, String>,
    /// Member id to name, from `space.json`.
    names: HashMap<String, String>,
    /// Version files in the folder this pass could not read yet.
    unread: u32,
    /// The scope is the library: the Space is part of what is compared.
    library: bool,
    /// The library leaves out these Spaces, which have their own share here.
    own_spaces: HashSet<String>,
    /// Seconds the person's typing must have stopped before an edit is
    /// written as a version; 0 writes at once.
    edit_settle: i64,
}

/// How long an automatic pass waits after the person's last save of a page
/// before writing it as a version, so a typing session is one version
/// rather than one every pass. A sync the person asks for writes at once.
const EDIT_SETTLE_SECS: i64 = if cfg!(test) { 0 } else { 120 };

/// While the person keeps typing, a version is still written this often,
/// counted from the version the page was last in step with (Notion: "every
/// 10 minutes as you actively edit it", plus one after the last edit).
const EDIT_EVERY_SECS: i64 = 10 * 60;

/// Whether to wait with this page's edit: the person is still typing (its
/// newest save is by the editor or a file edit, less than `settle` seconds
/// ago) and the version it was last in step with, written at `base_at`, is
/// less than [`EDIT_EVERY_SECS`] old.
fn still_typing(
    last: Option<&crate::db::PageHistoryEntry>,
    base_at: Option<i64>,
    now: i64,
    settle: i64,
) -> bool {
    settle > 0
        && base_at.is_some_and(|at| now - at < EDIT_EVERY_SECS)
        && last.is_some_and(|row| {
            crate::page_history::folds(Some(row.edited_by.as_str()))
                && now - row.created_at < settle
        })
}

/// One pass of `scope` with the folder in `settings`.
async fn pass(
    db: &MemoryDB,
    scope: &Scope,
    settings: &ShareSettings,
    settings_path: &Path,
    knowledge_path: Option<&Path>,
    edit_settle: i64,
) -> Result<SharedSyncCounts, WenlanError> {
    let folder = folder_for(scope, &settings.folder);
    let me = SpaceMember {
        id: settings.member_id.clone(),
        name: settings.member_name.clone(),
    };

    let mut local: HashMap<String, Page> = HashMap::new();
    let (own_spaces, own_pages) = if matches!(scope, Scope::Library) {
        own_shares(settings_path)?
    } else {
        Default::default()
    };
    // Library pages that stay in their own folder.
    let mut left_out: HashSet<String> = own_pages.clone();
    let space = match scope {
        Scope::Space(space) => {
            for status in ["active", "archived"] {
                for page in db
                    .list_pages_by_space(status, Some(space), 1_000_000, 0)
                    .await?
                {
                    if page.space.as_deref() == Some(space.as_str()) {
                        local.insert(page.id.clone(), page);
                    }
                }
            }
            space.clone()
        }
        Scope::Page(id) => match db.get_page(id).await? {
            Some(page) if matches!(page.status.as_str(), "active" | "archived") => {
                let space = page
                    .space
                    .clone()
                    .unwrap_or_else(|| UNFILED_SPACE_ID.to_string());
                local.insert(page.id.clone(), page);
                space
            }
            _ => UNFILED_SPACE_ID.to_string(),
        },
        Scope::Library => {
            for status in ["active", "archived"] {
                for page in db.list_pages_by_space(status, None, 1_000_000, 0).await? {
                    if own_pages.contains(&page.id)
                        || page
                            .space
                            .as_deref()
                            .is_some_and(|space| own_spaces.contains(space))
                    {
                        left_out.insert(page.id.clone());
                    } else {
                        local.insert(page.id.clone(), page);
                    }
                }
            }
            // The Space of each page is its own; this is not used.
            UNFILED_SPACE_ID.to_string()
        }
    };
    let name = match scope {
        Scope::Space(space) => space.clone(),
        Scope::Page(id) => local
            .get(id)
            .map(|page| page.title.clone())
            .unwrap_or_else(|| id.clone()),
        Scope::Library => LIBRARY_NAME.to_string(),
    };

    let space_file = {
        let folder = folder.clone();
        let scope = scope.clone();
        tokio::task::spawn_blocking(move || ensure_space_file(&folder, &scope, &name, &me))
            .await
            .map_err(|error| WenlanError::Generic(format!("shared folder task: {error}")))??
    };
    let space_file = if matches!(scope, Scope::Library) {
        let folder = folder.clone();
        let device = SpaceDevice {
            id: settings.device_id.clone(),
            name: library::computer_name(),
        };
        tokio::task::spawn_blocking(move || library::add_device(&folder, &device))
            .await
            .map_err(|error| WenlanError::Generic(format!("shared folder task: {error}")))??
    } else {
        space_file
    };
    let mut read = {
        let folder = folder.clone();
        tokio::task::spawn_blocking(move || version::read_folder(&folder))
            .await
            .map_err(|error| WenlanError::Generic(format!("shared folder task: {error}")))?
    };
    if let Scope::Page(id) = scope {
        // A page folder holds that page only; anything else in it is not
        // taken.
        read.pages.retain(|page, _| page == id);
        read.proposals.retain(|page, _| page == id);
        read.decisions.retain(|page, _| page == id);
    }
    if matches!(scope, Scope::Library) {
        // These keep their own folder: another person's sentences on them
        // must not be filed under this person's id here.
        read.pages.retain(|page, _| !left_out.contains(page));
    }
    let mut counts = SharedSyncCounts {
        skipped: read.skipped,
        ..SharedSyncCounts::default()
    };

    let key = scope.key();
    let ctx = Pass {
        db,
        scope,
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
        unread: read.skipped,
        library: matches!(scope, Scope::Library),
        own_spaces,
        edit_settle,
    };
    forget_held_of_other_folder(db, &held_key(&key), &ctx.folder).await?;
    let mut held = read_held(db, &key).await?;

    let ids: BTreeSet<&str> = local
        .keys()
        .map(String::as_str)
        .chain(read.pages.keys().map(String::as_str))
        .collect();
    let empty = Vec::new();
    let mut unsaved = false;
    let mut failed: Option<WenlanError> = None;
    // Applying the folder must not archive most of the library at once (a
    // damaged database or an emptied folder on the other computer): then
    // none of those pages is archived this pass.
    let mut hold: HashSet<&str> = HashSet::new();
    if ctx.library {
        let archiving: Vec<&str> = local
            .values()
            .filter(|page| page.status != "archived")
            .filter(|page| {
                let versions = read.pages.get(page.id.as_str()).unwrap_or(&empty);
                let Some(head) = version::head(versions) else {
                    return false;
                };
                let before = held.get(&page.id);
                head.winner.meta.archived
                    && before.is_none_or(|held| {
                        held.sha != head.winner.sha && held.version == Some(page.version)
                    })
            })
            .map(|page| page.id.as_str())
            .collect();
        // Measured against the pages still in use: pages archived long ago
        // must not make a large share of the live library look small.
        let active = local
            .values()
            .filter(|page| page.status != "archived")
            .count();
        if archiving.len() > MASS_ARCHIVE_MIN && archiving.len() * 2 > active {
            log::warn!(
                "[shared_space] {key}: the folder would archive {} of {} pages in use here; none is archived this pass",
                archiving.len(),
                active
            );
            hold.extend(archiving);
        }
    }
    if ctx.library {
        // The library's memories go the same way, before its pages: a page
        // taken here cites them, and a citation of a memory that is here
        // links to it.
        memories::sync(db, settings, &ctx.folder, &ctx.own_spaces, &mut counts).await?;
    }
    let total = ids.len();
    let progress = Progress::start(settings_path, &key, total);
    for (done, id) in ids.into_iter().enumerate() {
        progress.set(done, total);
        if hold.contains(id) {
            counts.held_back += 1;
            continue;
        }
        let versions = read.pages.get(id).unwrap_or(&empty);
        let before = held.get(id).cloned();
        let exported = counts.exported;
        let now = match local.get(id) {
            Some(page) => {
                ctx.sync_page(page, versions, before.as_ref(), &mut counts)
                    .await
            }
            None => {
                ctx.sync_missing(id, versions, before.as_ref(), &mut counts)
                    .await
            }
        };
        // One page that cannot be synced does not stop the others, and what
        // the pages before it took is still kept below.
        let now = match now {
            Ok(now) => now,
            Err(error) => {
                log::warn!("[shared_space] {key}: page {id} not synced this pass: {error}");
                counts.skipped += 1;
                failed.get_or_insert(error);
                None
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
            save_held(db, &key, &held).await?;
            unsaved = false;
        }
    }
    if unsaved {
        save_held(db, &key, &held).await?;
    }
    let heads: HashMap<String, String> = held
        .iter()
        .filter(|(_, held)| held.version.is_some())
        .map(|(id, held)| (id.clone(), held.sha.clone()))
        .collect();
    ctx.review(&read.proposals, &read.decisions, &heads).await?;
    match failed {
        Some(error) => Err(error),
        None => Ok(counts),
    }
}

async fn save_held(
    db: &MemoryDB,
    key: &str,
    held: &BTreeMap<String, Held>,
) -> Result<(), WenlanError> {
    db.set_app_metadata(&held_key(key), &serde_json::to_string(held)?)
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
    /// The page here, as its versions are compared. The library compares
    /// the page's Space too; a Space or page share does not.
    fn local_state(&self, page: &Page) -> version::State {
        version::State {
            title: page.title.clone(),
            body_digest: version::body_digest(&page.content),
            archived: page.status == "archived",
            locked: page.review_required,
            space: if self.library {
                named_space(page.space.as_deref()).map(str::to_string)
            } else {
                None
            },
        }
    }

    /// A version, as it is compared with [`Self::local_state`].
    fn version_state(&self, version: &PageVersion) -> version::State {
        version::State {
            space: if self.library {
                named_space(version.space()).map(str::to_string)
            } else {
                None
            },
            ..version.state()
        }
    }

    /// The newest version with this content.
    fn newest_with<'a>(
        &self,
        versions: &'a [PageVersion],
        state: &version::State,
    ) -> Option<&'a PageVersion> {
        versions
            .iter()
            .filter(|version| &self.version_state(version) == state)
            .max_by(|a, b| (&a.meta.ts, &a.sha).cmp(&(&b.meta.ts, &b.sha)))
    }

    /// The Space a version's page is kept in here: the library's own Space
    /// for it (`None` is Unfiled), else this pass's Space.
    fn target_space(&self, version: &PageVersion) -> String {
        if self.library {
            named_space(version.space())
                .unwrap_or(UNFILED_SPACE_ID)
                .to_string()
        } else {
            self.space.clone()
        }
    }

    /// Whether the library leaves this version's Space to its own share.
    fn leaves_to_own_share(&self, version: &PageVersion) -> bool {
        self.library
            && named_space(version.space()).is_some_and(|space| self.own_spaces.contains(space))
    }

    /// Makes the Space `name` here when there is none (a page of another
    /// computer's Space). `false` when it could not be made this pass.
    async fn ensure_space(&self, name: &str) -> Result<bool, WenlanError> {
        ensure_space(self.db, name).await
    }

    /// A page of the Space that is here. Returns what to remember of it.
    async fn sync_page(
        &self,
        page: &Page,
        versions: &[PageVersion],
        held: Option<&Held>,
        counts: &mut SharedSyncCounts,
    ) -> Result<Option<Held>, WenlanError> {
        if page.status == "archived" && versions.is_empty() && !self.library {
            // Archived before it was ever shared: it stays here only. (The
            // library takes every page of the library with it, archived
            // ones too.)
            return Ok(None);
        }
        let head = version::head(versions);
        let state = self.local_state(page);
        let here_space = self.library.then(|| held_space(page));
        // In step with what was held and untouched here since: no new page
        // version, and (the library) the page still in the Space it was in.
        // A Space renamed or deleted here moves its pages with no new page
        // version, so that move counts as a change made here.
        let untouched = |held: &Held| {
            held.version == Some(page.version)
                && !(self.library && held.space.is_some() && held.space != here_space)
        };
        let recorded = held.and_then(|held| versions.iter().find(|v| v.sha == held.sha));
        // A version written as an edit of a version that cannot be read: its
        // parent named all the same, so an edit made elsewhere since is a
        // second leaf (the conflict is kept), never written over.
        let mut parent_sha = None;
        // The version this page was last in step with, and whether it was
        // changed here since.
        let (base, changed) = match (held, recorded) {
            // Locking or unlocking a page keeps its version, so a lock that
            // differs from the version it was last in step with was changed
            // here (an apply sets the lock to the version's).
            (Some(held), Some(version)) => {
                let changed = (!untouched(held) && self.version_state(version) != state)
                    || version.meta.locked != page.review_required;
                // A later version taken here but not remembered (the pass
                // stopped before what it held was kept): the page is that
                // version, not an edit made here. Sharing it as one would
                // win over whatever the others wrote since.
                let later = versions
                    .iter()
                    .filter(|_| changed)
                    .filter(|v| v.sha != version.sha && self.version_state(v) == state)
                    .filter(|v| comes_after(versions, v, &version.sha))
                    .max_by(|a, b| (&a.meta.ts, &a.sha).cmp(&(&b.meta.ts, &b.sha)));
                match later {
                    Some(later) => (Some(later), false),
                    None => (Some(version), changed),
                }
            }
            // The version it was last in step with cannot be read this pass
            // (a cloud file not downloaded yet, one still arriving, or one
            // gone), but the page did not change here since: it is not an
            // edit made here. Only a head known to come after that version
            // is taken; an older one never is, and the page waits.
            (Some(held), None) if untouched(held) => {
                if let Some(head) = head.as_ref() {
                    if self.version_state(head.winner) != state
                        && !comes_after(versions, head.winner, &held.sha)
                    {
                        counts.skipped += 1;
                        return Ok(None);
                    }
                }
                (self.newest_with(versions, &state), false)
            }
            // Changed here, over a version that cannot be read this pass:
            // writing it now would be written over whatever the others did
            // since. Wait until the folder is whole.
            (Some(_), None) if self.unread > 0 => {
                counts.skipped += 1;
                return Ok(None);
            }
            // Changed here, over a version that is not in the folder at all.
            (Some(held), None) => match self.newest_with(versions, &state) {
                Some(version) => (Some(version), false),
                None => {
                    parent_sha = Some(held.sha.as_str());
                    (None, true)
                }
            },
            _ => match self.newest_with(versions, &state) {
                Some(version) => (Some(version), false),
                None => (None, true),
            },
        };
        let at = |sha: &str| Held {
            sha: sha.to_string(),
            version: Some(page.version),
            space: here_space.clone(),
        };
        if let Some(head) = head
            .as_ref()
            .filter(|head| self.version_state(head.winner) == state)
        {
            // Already the head (an edit made here and elsewhere alike, or a
            // version taken earlier).
            return Ok(Some(at(&head.winner.sha)));
        }
        // A member who may only read writes no version: the team's version
        // is taken over the edit, which stays in this page's history.
        let changed = changed && !self.settings.read_only;
        if changed && self.edit_settle > 0 {
            let last = self.db.list_page_history(&page.id, 1).await?;
            let base_at = base
                .and_then(|base| version::ts_key(&base.meta.ts))
                .map(|at| at.timestamp());
            let now = chrono::Utc::now().timestamp();
            if still_typing(last.first(), base_at, now, self.edit_settle) {
                // Written once the typing stops; the head waits too, so it
                // is never taken over the edit.
                return Ok(None);
            }
        }
        if changed {
            return match self
                .export_page(page, versions, head.as_ref(), base, parent_sha)
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
        // Only the computer that wrote the losing edit keeps it: another
        // computer that took it before the head arrived holds the same text,
        // and would keep a second copy under its own name.
        let own = self.own_device();
        let mine = head.leaves.iter().copied().find(|leaf| {
            leaf.sha != head.winner.sha
                && self.version_state(leaf) == state
                && (leaf.meta.device.is_none() || leaf.meta.device == own)
        });
        let lost = mine.is_some();
        // Two edits a person made at the same time on two computers (or two
        // members): put together when they changed different lines, else
        // both kept, the one made here as a second page.
        let together = match mine {
            Some(mine) => together(versions, mine, head.winner),
            None => Together::Notice,
        };
        // Versions written while this Wenlan was away, between the one it
        // was in step with and the head: each goes into this page's history
        // too, so every computer lists the same edits.
        let mut page = page.clone();
        if !lost {
            for between in versions_between(versions, base, head.winner) {
                let applied = self.apply_version(&page, between).await?;
                if !applied.complete {
                    break;
                }
                page = self.reload(&page.id).await?;
            }
        }
        let page = &page;
        let applied = self.apply_version(page, head.winner).await?;
        if applied.wrote && applied.complete {
            counts.applied += 1;
            if lost {
                let merged = match (&together, mine) {
                    (Together::Merged(merged), _) => {
                        self.write_merged(&page.id, applied.at, merged).await?
                    }
                    (Together::Both, Some(mine)) => {
                        self.keep_as_second_page(page, mine).await?;
                        false
                    }
                    _ => false,
                };
                if !merged {
                    counts.conflicts += 1;
                    // The person is told once, on this page, that their edit
                    // lost and where it is.
                    let now = self.reload(&page.id).await?;
                    self.db
                        .mark_concurrent_edit(&page.id, now.version, page.version)
                        .await?;
                }
            }
        } else if !applied.complete {
            counts.skipped += 1;
        }
        if !(applied.wrote || applied.complete) {
            return Ok(None);
        }
        Ok(Some(Held {
            sha: head.winner.sha.clone(),
            version: Some(applied.at),
            space: self.held_space_of(&page.id).await?,
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
                self.scope.key()
            );
            counts.skipped += 1;
            return Ok(None);
        }
        let winner = head.winner;
        // A page of a Space that has its own share here is taken through
        // that share, not the library.
        if self.leaves_to_own_share(winner) {
            return Ok(None);
        }
        let deleted = |sha: &str| Held {
            sha: sha.to_string(),
            version: None,
            space: None,
        };
        // Deleted here over a version that is no longer the head: someone
        // edited it elsewhere since, never seen here. That edit is not
        // archived by a delete made over an older copy; the page comes back.
        let edited_since = matches!(
            held,
            Some(Held { sha, version: Some(_), .. })
                if sha != &winner.sha
                    && !winner.meta.archived
                    && versions.iter().any(|version| &version.sha == sha)
        );
        match held {
            // The person deleted it here. It is not made again: the others
            // get a version that archives it, which a restore undoes.
            Some(Held {
                version: Some(_), ..
            }) if !edited_since => {
                if winner.meta.archived || self.settings.read_only {
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
            Some(Held {
                sha, version: None, ..
            }) if sha == &winner.sha || winner.meta.archived => Ok(Some(deleted(&winner.sha))),
            // Never here, or restored elsewhere since it was deleted here.
            // An archived page that was never here stays in the folder only
            // (the library makes it here, archived).
            _ if winner.meta.archived && !self.library => Ok(None),
            _ => match self.apply_create(winner).await? {
                Some(mut at) => {
                    counts.applied += 1;
                    if winner.meta.archived {
                        let made = self.reload(id).await?;
                        if let Some(archived) = self.archive_here(id, made.version).await? {
                            // A title taken here (`at` 0) is still shared
                            // as an edit made here next pass.
                            if at != 0 {
                                at = archived.version;
                            }
                        }
                    }
                    Ok(Some(Held {
                        sha: winner.sha.clone(),
                        version: Some(at),
                        space: self.held_space_of(id).await?,
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
        let taken = self.taken_titles(id, writer).await?;
        if !taken.contains(&MemoryDB::page_title_key(wanted)) {
            return Ok(wanted.to_string());
        }
        let name = self
            .names
            .get(&writer.meta.author.member)
            .map(String::as_str)
            .unwrap_or("shared");
        let candidate = free_name(&taken, wanted, name);
        log::info!("[shared_space] {id}: the title {wanted:?} is taken here; it is {candidate:?}");
        Ok(candidate)
    }

    /// The titles other pages hold in the Space `writer`'s page goes to.
    async fn taken_titles(
        &self,
        id: &str,
        writer: &PageVersion,
    ) -> Result<HashSet<String>, WenlanError> {
        // The library keeps titles apart within the Space the page goes to.
        let space = self.target_space(writer);
        let taken: HashSet<String> = self
            .db
            .list_pages_by_space("active", Some(&space), 1_000_000, 0)
            .await?
            .into_iter()
            .filter(|page| {
                page.id != id
                    && if self.library {
                        page.space.as_deref() == named_space(Some(&space))
                    } else {
                        page.space.as_deref() == Some(self.space.as_str())
                    }
            })
            .map(|page| MemoryDB::page_title_key(&page.title))
            .collect();
        Ok(taken)
    }

    /// Writes two edits put together onto the page, which holds the head
    /// now. The next pass shares it as a version after both. `false` when
    /// it could not be written: the person is told as for a lost edit.
    async fn write_merged(
        &self,
        id: &str,
        applied_at: i64,
        merged: &Merged,
    ) -> Result<bool, WenlanError> {
        let mut current = self.reload(id).await?;
        // Edited here since the head was applied: that edit is never written
        // over; it goes to the others as an edit of its own next pass.
        if current.status != "active" || current.version != applied_at {
            return Ok(false);
        }
        if current.content != merged.body {
            let write = crate::post_write::PageWrite::UpdatePreservingSources {
                page_id: id,
                req: wenlan_types::requests::UpdatePageRequest {
                    content: merged.body.clone(),
                    source_memory_ids: Vec::new(),
                    expected_version: Some(current.version),
                    caller_id: None,
                    operation_id: None,
                },
                edited_by: OWN_DEVICE_PERSON,
                knowledge_path: self.knowledge_path,
                basis: crate::db::BasisWrite::default(),
            };
            match crate::post_write::page_write(self.db, write).await {
                Ok(result) if result.wrote => {}
                Ok(_) => return Ok(false),
                Err(error) => {
                    skipped_write(id, error)?;
                    return Ok(false);
                }
            }
            if self.library {
                let mut file = PageFile::new(&merged.title, &merged.body);
                file.citations = merged.citations.clone();
                let quote =
                    crate::store::format::page::Locator::Other(rewrite::QUOTE_LOCATOR.to_string());
                file.citations.retain(|citation| citation.locator != quote);
                let json = crate::store::rebuild::pages::citations_of(&file, id, &mut Vec::new());
                self.db.set_page_citations(id, json.as_deref()).await?;
            }
            current = self.reload(id).await?;
        }
        if current.title != merged.title {
            let renamed = self
                .db
                .rename_page(
                    id,
                    &merged.title,
                    current.version,
                    &ReadScope::Global,
                    self.knowledge_path,
                )
                .await;
            if let Err(error) = renamed {
                // The text is together; the title waits for the next edit.
                log::warn!("[shared_space] {id}: merged title not set: {error}");
            }
        }
        log::info!("[shared_space] {id}: two edits made at the same time put together");
        Ok(true)
    }

    /// Keeps `mine`, an edit made here that could not be put together with
    /// the head, as a second page next to it, named after this computer (the
    /// library) or this member.
    async fn keep_as_second_page(
        &self,
        page: &Page,
        mine: &PageVersion,
    ) -> Result<(), WenlanError> {
        let who = if self.library {
            library::computer_name()
        } else {
            self.settings.member_name.clone()
        };
        // Named by the edit it keeps, so a pass that stopped halfway, or a
        // second computer with the same edit, never makes it twice.
        let id = second_page_id(&page.id, &mine.sha);
        if self.db.get_page(&id).await?.is_some() {
            return Ok(());
        }
        let taken = self.taken_titles(&id, mine).await?;
        let title = free_name(&taken, &mine.page.title, &who);
        let space = match self.scope {
            Scope::Space(space) => Some(space.clone()).into(),
            Scope::Page(_) | Scope::Library => match named_space(page.space.as_deref()) {
                Some(space) => {
                    wenlan_types::space_context::WriteSpaceTarget::Named(space.to_string())
                }
                None => wenlan_types::space_context::WriteSpaceTarget::Uncategorized,
            },
        };
        let distillation = crate::tuning::DistillationConfig::default();
        let write = crate::post_write::PageWrite::Create {
            page_id: Some(&id),
            req: wenlan_types::requests::CreateConceptRequest {
                title: title.clone(),
                content: mine.page.body.clone(),
                summary: None,
                entity_id: None,
                space,
                source_memory_ids: Vec::new(),
                creation_kind: Some("authored".to_string()),
                workspace: None,
            },
            agent: OWN_DEVICE_PERSON,
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
            Ok(result) if result.wrote => {
                self.apply_sources(&id, mine).await?;
                let made = self.reload(&id).await?;
                self.db
                    .record_page_writer(&id, made.version, OWN_DEVICE_PERSON)
                    .await?;
                log::info!(
                    "[shared_space] {}: an edit made at the same time kept as {title:?}",
                    page.id
                );
            }
            Ok(_) => {}
            Err(error) => {
                skipped_write(&page.id, error)?;
            }
        }
        Ok(())
    }

    /// The library: this computer's id, written into each version it makes.
    fn own_device(&self) -> Option<String> {
        (self.library && !self.settings.device_id.is_empty())
            .then(|| self.settings.device_id.clone())
    }

    /// The library: the Space page `id` is in here now, to hold with it.
    async fn held_space_of(&self, id: &str) -> Result<Option<String>, WenlanError> {
        if !self.library {
            return Ok(None);
        }
        Ok(self.db.get_page(id).await?.as_ref().map(held_space))
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
            if !apply_update(
                self.db,
                &current,
                head,
                self.knowledge_path,
                &self.settings.member_id,
            )
            .await?
            {
                return Ok(stopped(wrote, &current));
            }
            wrote = true;
            self.apply_sources(id, head).await?;
            current = self.reload(id).await?;
        }
        if self.library
            && current.status == "active"
            && !self.leaves_to_own_share(head)
            && named_space(current.space.as_deref()) != named_space(head.space())
        {
            match self.move_to_space(&current, head).await? {
                Some(moved) => {
                    wrote = true;
                    current = moved;
                }
                None => return Ok(stopped(wrote, &current)),
            }
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
        if current.review_required != head.meta.locked {
            self.db
                .set_page_review_required(id, head.meta.locked)
                .await?;
            wrote = true;
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

    /// Archives the page `id` at `version` and its file, as the app does.
    /// `None` when the page moved since.
    async fn archive_here(&self, id: &str, version: i64) -> Result<Option<Page>, WenlanError> {
        let Some(archived_at) = self.db.archive_page_at_version(id, version).await? else {
            log::warn!("[shared_space] {id} not archived this pass: it moved");
            return Ok(None);
        };
        if let Some(root) = self.knowledge_path {
            let projection =
                crate::export::knowledge::KnowledgeProjectionWrite::new(root.into(), self.db);
            if let Err(error) = projection
                .archive_page_if_still_archived(self.db, id, archived_at)
                .await
            {
                log::warn!("[shared_space] archived {id} but its file stays: {error}");
            }
        }
        Ok(Some(self.reload(id).await?))
    }

    /// Moves `page` into the Space `head` names (made here when there is
    /// none), through the same write the app uses. `None` when it is not
    /// moved this pass.
    async fn move_to_space(
        &self,
        page: &Page,
        head: &PageVersion,
    ) -> Result<Option<Page>, WenlanError> {
        let target = named_space(head.space());
        if let Some(name) = target {
            if !self.ensure_space(name).await? {
                return Ok(None);
            }
        }
        match self
            .db
            .set_page_space(&page.id, target, page.version, &ReadScope::Global)
            .await
        {
            Ok(_) => {}
            Err(crate::db::SetPageSpaceError::Refused { code, .. }) => {
                log::warn!("[shared_space] {} not moved this pass: {code:?}", page.id);
                return Ok(None);
            }
            Err(crate::db::SetPageSpaceError::Failed(error)) => {
                skipped_write(&page.id, error)?;
                return Ok(None);
            }
        }
        let moved = self.reload(&page.id).await?;
        if let Some(root) = self.knowledge_path {
            let projection =
                crate::export::knowledge::KnowledgeProjectionWrite::new(root.into(), self.db);
            if let Err(error) = projection.write_page_gated(self.db, &moved).await {
                log::warn!("[shared_space] moved {} but not its file: {error}", page.id);
            }
        }
        Ok(Some(moved))
    }

    /// Makes the head here as a new page. Returns the version to remember.
    /// The library takes a version's sources along: the person's other
    /// computer has the memories and pages they cite, so the page here links
    /// back to them and its memories count as written into a page (nightly
    /// organize does not write them again). A Space or page share keeps
    /// sources private, as quotes.
    async fn apply_sources(&self, id: &str, head: &PageVersion) -> Result<(), WenlanError> {
        if !self.library {
            return Ok(());
        }
        let mut file = head.page.clone();
        let quote = crate::store::format::page::Locator::Other(rewrite::QUOTE_LOCATOR.to_string());
        file.citations.retain(|citation| citation.locator != quote);
        let json = crate::store::rebuild::pages::citations_of(&file, id, &mut Vec::new());
        self.db.set_page_citations(id, json.as_deref()).await
    }

    async fn apply_create(&self, head: &PageVersion) -> Result<Option<i64>, WenlanError> {
        let Some(id) = head.page.id.as_deref() else {
            return Ok(None);
        };
        if let Some(space) = named_space(head.space()).filter(|_| self.library) {
            if !self.ensure_space(space).await? {
                return Ok(None);
            }
        }
        let title = self.free_title(id, &head.page.title, head).await?;
        let agent = writer_edited_by(&head.meta.author, &self.settings.member_id);
        if head.page.body.trim().is_empty() {
            return self
                .apply_create_untitled_body(id, &title, head, &agent)
                .await;
        }
        let distillation = crate::tuning::DistillationConfig::default();
        let write = crate::post_write::PageWrite::Create {
            page_id: Some(id),
            req: wenlan_types::requests::CreateConceptRequest {
                title: title.clone(),
                content: head.page.body.clone(),
                summary: None,
                entity_id: None,
                space: match self.scope {
                    Scope::Space(space) => Some(space.clone()).into(),
                    // A page shared alone is made with no Space; the person
                    // moves it where they like.
                    Scope::Page(_) => wenlan_types::space_context::WriteSpaceTarget::Uncategorized,
                    // The library keeps each page in the Space it is in on
                    // the other computer.
                    Scope::Library => match named_space(head.space()) {
                        Some(space) => {
                            wenlan_types::space_context::WriteSpaceTarget::Named(space.to_string())
                        }
                        None => wenlan_types::space_context::WriteSpaceTarget::Uncategorized,
                    },
                },
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
        self.apply_sources(id, head).await?;
        // A page made under another title is shared under it next pass.
        let page = self.reload(id).await?;
        // A create keeps no changelog; record the member so the page's rows
        // say who wrote it.
        self.db.record_page_writer(id, page.version, &agent).await?;
        if head.meta.locked {
            self.db.set_page_review_required(id, true).await?;
        }
        Ok(Some(if title == head.page.title {
            page.version
        } else {
            0
        }))
    }

    /// A page with a title and no text yet: made the way the app makes one
    /// (a draft, then published), since a page write refuses an empty body.
    async fn apply_create_untitled_body(
        &self,
        id: &str,
        title: &str,
        head: &PageVersion,
        agent: &str,
    ) -> Result<Option<i64>, WenlanError> {
        let space = match &self.scope {
            Scope::Space(space) => Some(space.as_str()),
            Scope::Page(_) => None,
            Scope::Library => named_space(head.space()),
        };
        let draft = match self
            .db
            .create_page_draft_with_id(id, title, &head.page.body, space, None)
            .await
        {
            Ok(draft) => draft,
            Err(error) => {
                skipped_write(id, error)?;
                return Ok(None);
            }
        };
        let page = match self.db.publish_page_draft(id, draft.version).await {
            Ok(crate::pages::PageDraftPublishOutcome::Published(page)) => page,
            Ok(_) => {
                log::info!("[shared_space] {id}: the empty page could not be published this pass");
                return Ok(None);
            }
            Err(error) => {
                skipped_write(id, error)?;
                return Ok(None);
            }
        };
        if let Some(root) = self.knowledge_path {
            crate::export::knowledge::KnowledgeProjectionWrite::new(root.to_path_buf(), self.db)
                .write_page_gated(self.db, &page)
                .await?;
        }
        self.db.record_page_writer(id, page.version, agent).await?;
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
        parent_sha: Option<&str>,
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
        // An edit of a version that is not in the folder names that version
        // as its parent, whatever else the folder holds.
        let (parent, merged) = match parent_sha {
            Some(_) => (None, Vec::new()),
            None => base_of(&history, versions, head, base),
        };
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
            keep_all: self.library,
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
        if self.library {
            // The Space, so a move or a new Space follows to the other
            // computer. Unfiled has none.
            if let Some(space) = named_space(page.space.as_deref()) {
                file.other.insert(
                    serde_yaml::Value::String(version::SPACE_KEY.to_string()),
                    serde_yaml::Value::String(space.to_string()),
                );
            }
        }
        file.created = Some(page.created_at.clone());
        file.modified = Some(page.last_modified.clone());
        file.citations = citations;
        file.basis = basis;
        let meta = VersionMeta {
            parent: parent
                .map(|parent| parent.sha.clone())
                .or(parent_sha.map(str::to_string)),
            merged,
            body_digest: version::body_digest(&page.content),
            author: match parent.filter(|parent| lock_only(parent, page)) {
                // Only the lock changed: the person here did that.
                Some(_) => Author {
                    member: self.settings.member_id.clone(),
                    kind: AuthorKind::Person,
                    agent: None,
                },
                None => self.author_of(page).await?,
            },
            ts: edit_ts(&page.last_modified),
            archived: page.status == "archived",
            locked: page.review_required,
            proposal: false,
            decides: None,
            device: self.own_device(),
        };
        let member = meta.author.member.clone();
        let text = version::render(&file, &meta);
        let name = version::version_file_name(&version::sha256_hex(text.as_bytes()));
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
        self.write_as(&member, &page.id, text).await.map(Some)
    }

    /// Who wrote the page's current version here. On a team server, an edit
    /// a member's AI tool made through the team listener (a team agent name,
    /// or `shared_*:<member>` on a page it made) is that member's, when they
    /// are in the Space's member list.
    async fn author_of(&self, page: &Page) -> Result<Author, WenlanError> {
        let author = local_author(self.db, page, &self.settings.member_id).await?;
        if !self.settings.hosted {
            return Ok(author);
        }
        let edited_by = self
            .db
            .list_page_history(&page.id, 1)
            .await?
            .into_iter()
            .next()
            .map(|row| row.edited_by)
            .unwrap_or_default();
        let from_create =
            shared_writer(&edited_by).map(|(kind, member)| (kind, member.to_string(), None));
        let from_agent = author.agent.as_deref().and_then(|agent| {
            let (tool, member) = wenlan_types::team::split_team_agent(agent)?;
            Some((author.kind, member.to_string(), Some(tool.to_string())))
        });
        match from_agent.or(from_create) {
            Some((kind, member, agent)) if self.names.contains_key(&member) => Ok(Author {
                member,
                kind,
                agent,
            }),
            _ => Ok(Author {
                // A team agent name of no member here: the tool only.
                agent: author.agent.map(|agent| {
                    wenlan_types::team::split_team_agent(&agent)
                        .map(|(tool, _)| tool.to_string())
                        .unwrap_or(agent)
                }),
                ..author
            }),
        }
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
            locked: winner.meta.locked,
            proposal: false,
            decides: None,
            device: self.own_device(),
        };
        let text = version::render(&winner.page, &meta);
        self.write(&id, text).await.map(Some)
    }

    async fn write(&self, page_id: &str, text: String) -> Result<String, WenlanError> {
        let member = self.settings.member_id.clone();
        self.write_as(&member, page_id, text).await
    }

    /// Writes a version under `member`'s folder: this Wenlan's own, or, on a
    /// team server, the member it was written for ([`Self::author_of`]).
    async fn write_as(
        &self,
        member: &str,
        page_id: &str,
        text: String,
    ) -> Result<String, WenlanError> {
        self.write_kind(member, version::PAGES_DIR, page_id, text)
            .await
    }

    /// [`Self::write_as`] into `kind` (one of [`version::KIND_DIRS`]).
    async fn write_kind(
        &self,
        member: &str,
        kind: &'static str,
        page_id: &str,
        text: String,
    ) -> Result<String, WenlanError> {
        let folder = self.folder.clone();
        let member = member.to_string();
        let page_id = page_id.to_string();
        let (sha, _) = tokio::task::spawn_blocking(move || {
            version::write_file(&folder, &member, kind, &page_id, &text)
        })
        .await
        .map_err(|error| WenlanError::Generic(format!("shared folder task: {error}")))??;
        Ok(sha)
    }
}

fn now_ts() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A version's `ts`: when the page was last changed here, so of two edits
/// made apart the later edit wins even when the earlier computer was
/// offline and writes its version last. Now when that time cannot be read
/// or is ahead of this computer's clock.
fn edit_ts(last_modified: &str) -> String {
    let now = chrono::Utc::now();
    match version::ts_key(last_modified) {
        Some(at) if at <= now => at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        _ => now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    }
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
        // A change the person asked their AI tool for is the person's to
        // share: its sentences are theirs (UX-WHO-2).
        crate::authorship::VersionAuthor::PersonAsked => AuthorKind::Person,
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
/// Most versions taken in between on one pass, oldest kept out first.
const BETWEEN_MAX: usize = 20;

/// The versions after `base` and before `head` on head's line, oldest
/// first, leaving out archives. Empty when `base` is not on that line or a
/// version on the way cannot be read this pass.
fn versions_between<'a>(
    versions: &'a [PageVersion],
    base: Option<&PageVersion>,
    head: &PageVersion,
) -> Vec<&'a PageVersion> {
    let Some(base) = base else {
        return Vec::new();
    };
    let mut line = Vec::new();
    let mut at = head.meta.parent.as_deref();
    while let Some(sha) = at {
        if sha == base.sha {
            line.reverse();
            let skip = line.len().saturating_sub(BETWEEN_MAX);
            return line
                .into_iter()
                .skip(skip)
                .filter(|version: &&PageVersion| !version.meta.archived)
                .collect();
        }
        let Some(version) = versions.iter().find(|version| version.sha == sha) else {
            return Vec::new();
        };
        if line.len() > versions.len() {
            return Vec::new();
        }
        line.push(version);
        at = version.meta.parent.as_deref();
    }
    Vec::new()
}

/// `wanted (who)`, or `wanted (who N)` when that is taken too.
/// The id of the second page that keeps `sha`, an edit of page `id` that
/// lost to another made at the same time: the same on every pass and every
/// computer.
fn second_page_id(id: &str, sha: &str) -> String {
    let digest = version::sha256_hex(format!("{id}\n{sha}").as_bytes());
    let mut bytes = [0u8; 16];
    for (n, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digest[n * 2..n * 2 + 2], 16).unwrap_or(0);
    }
    format!(
        "page_{}",
        uuid::Builder::from_random_bytes(bytes).into_uuid()
    )
}

fn free_name(taken: &HashSet<String>, wanted: &str, who: &str) -> String {
    let mut candidate = format!("{wanted} ({who})");
    let mut n = 2;
    while taken.contains(&MemoryDB::page_title_key(&candidate)) {
        candidate = format!("{wanted} ({who} {n})");
        n += 1;
    }
    candidate
}

/// What becomes of an edit made here at the same time as the head, which
/// won over it.
enum Together {
    /// Not two people's edits of the text (an AI tool's, an archive, a
    /// move): the head is taken and the person told.
    Notice,
    /// Both edits, put together.
    Merged(Merged),
    /// Both changed the same lines: the edit here stays as a second page.
    Both,
}

struct Merged {
    title: String,
    body: String,
    citations: Vec<crate::store::format::page::Citation>,
}

/// [`Together`] for `mine`, made here, and `winner`, the head.
fn together(versions: &[PageVersion], mine: &PageVersion, winner: &PageVersion) -> Together {
    use version::Versioned;
    if !(mine.by_person() && winner.by_person())
        || mine.meta.archived
        || winner.meta.archived
        || mine.space() != winner.space()
    {
        return Together::Notice;
    }
    let Some(base) = common_base(versions, mine, winner) else {
        return Together::Both;
    };
    let title = match pick(&base.page.title, &mine.page.title, &winner.page.title) {
        Some(title) => title.to_string(),
        None => return Together::Both,
    };
    let Some(body) = merge::merge3(&base.page.body, &mine.page.body, &winner.page.body) else {
        return Together::Both;
    };
    // The sources of both; a number each gave a different source cannot
    // be told apart in the text.
    let mut citations = winner.page.citations.clone();
    for citation in &mine.page.citations {
        match citations.iter().find(|kept| kept.marker == citation.marker) {
            Some(kept) if kept.locator != citation.locator => return Together::Both,
            Some(_) => {}
            None => citations.push(citation.clone()),
        }
    }
    citations.sort_by_key(|citation| citation.marker);
    Together::Merged(Merged {
        title,
        body,
        citations,
    })
}

/// One value two edits of `base` may each have changed.
fn pick<'a>(base: &str, ours: &'a str, theirs: &'a str) -> Option<&'a str> {
    if ours == theirs || theirs == base {
        Some(ours)
    } else if ours == base {
        Some(theirs)
    } else {
        None
    }
}

/// The nearest version both `a` and `b` come from.
fn common_base<'a>(
    versions: &'a [PageVersion],
    a: &'a PageVersion,
    b: &'a PageVersion,
) -> Option<&'a PageVersion> {
    use version::Versioned;
    let by_sha: HashMap<&str, &PageVersion> = versions
        .iter()
        .map(|version| (version.sha.as_str(), version))
        .collect();
    let ancestors = |from: &'a PageVersion| -> Vec<&'a str> {
        let mut order: Vec<&'a str> = Vec::new();
        let mut seen: HashSet<&'a str> = HashSet::new();
        let mut queue: std::collections::VecDeque<&'a str> = from.parents().into();
        while let Some(sha) = queue.pop_front() {
            if !seen.insert(sha) {
                continue;
            }
            order.push(sha);
            if let Some(version) = by_sha.get(sha) {
                queue.extend(version.parents());
            }
        }
        order
    };
    let of_a: HashSet<&str> = ancestors(a).into_iter().collect();
    ancestors(b)
        .into_iter()
        .find(|sha| of_a.contains(sha))
        .and_then(|sha| by_sha.get(sha).copied())
}

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
    // Never the head when this page is not known to have held it: an edit
    // of an older copy written as the head's child would replace the head
    // without ever having had its text. With no proven parent it is a
    // version of its own, and the two are put together or both kept.
    let parent = known.or_else(|| history.iter().find_map(newest));
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

/// Makes the Space `name` here when there is none (a page or memory of
/// another computer's Space). `false` when it could not be made this pass.
async fn ensure_space(db: &MemoryDB, name: &str) -> Result<bool, WenlanError> {
    if db.get_space(name).await?.is_some() {
        return Ok(true);
    }
    match db.create_space(name, None, false).await {
        Ok(_) => {
            log::info!("[shared_space] made the Space {name:?} for the library");
            Ok(true)
        }
        // Made meanwhile, or a name no Space can have.
        Err(error) => {
            if db.get_space(name).await?.is_some() {
                return Ok(true);
            }
            skipped_write(name, error).map(|_| false)
        }
    }
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
    me: &str,
) -> Result<bool, WenlanError> {
    let edited_by = writer_edited_by(&head.meta.author, me);
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
