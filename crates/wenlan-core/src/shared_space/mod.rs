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
use crate::rulesync_team::{self, TeamRuleLayer};
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
    /// The last approved Rules version pulled for each Space on each server.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    team_rules: BTreeMap<String, BTreeMap<String, TeamRuleLayer>>,
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
    /// Per-Space grants from the last authenticated response. A refused
    /// grant also fences any earlier pull that hasn't completed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rule_grants: Option<BTreeMap<String, Option<String>>>,
}

/// Keeps that this person is on the team at `url` as `me` says, with
/// `key`.
pub fn remember_membership(
    path: &Path,
    url: &str,
    key: &str,
    me: &wenlan_types::team::TeamMe,
) -> Result<(), WenlanError> {
    let grants: BTreeMap<_, _> = me
        .spaces
        .iter()
        .map(|entry| (entry.space.clone(), entry.id.clone()))
        .collect();
    let membership = Membership {
        key: key.to_string(),
        member_id: me.member_id.clone(),
        owner: me.role == wenlan_types::team::TeamRole::Owner,
        rule_grants: Some(grants.clone()),
    };
    change_settings(path, |store| {
        let mut changed = false;
        if store
            .memberships
            .get(url)
            .is_some_and(|before| before.member_id != membership.member_id)
        {
            changed |= store.team_rules.remove(url).is_some();
        }
        for share in store
            .spaces
            .values_mut()
            .filter(|share| share.member_id == me.member_id)
        {
            if let Some(host) = share
                .host
                .as_mut()
                .filter(|host| host.url == url && host.key != key)
            {
                host.key = key.to_string();
                changed = true;
            }
        }
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
        if let Some(layers) = store.team_rules.get_mut(url) {
            let before = layers.len();
            layers.retain(|space, layer| {
                grants.get(space).is_some_and(|id| {
                    id.as_ref().is_none_or(|id| {
                        layer
                            .project
                            .as_ref()
                            .is_none_or(|project| &project.space_id == id)
                    })
                })
            });
            changed |= layers.len() != before;
            if layers.is_empty() {
                store.team_rules.remove(url);
            }
        }
        (changed, ())
    })
}

/// Forgets the team at `url` (this person left it).
pub fn forget_membership(path: &Path, url: &str) -> Result<(), WenlanError> {
    change_settings(path, |store| {
        let membership = store.memberships.remove(url).is_some();
        let rules = store.team_rules.remove(url).is_some();
        (membership || rules, ())
    })
}

pub(crate) fn cached_team_rule_layers(path: &Path) -> Result<Vec<TeamRuleLayer>, WenlanError> {
    let store = read_settings(path)?;
    Ok(store
        .spaces
        .values()
        .filter_map(|share| share.host.as_ref())
        .filter(|host| rule_grant_matches(&store, host))
        .filter_map(|host| store.team_rules.get(&host.url)?.get(&host.space).cloned())
        .collect())
}

pub(crate) fn cached_project_rule_layer_for_space(
    path: &Path,
    local_space: &str,
) -> Result<Option<TeamRuleLayer>, WenlanError> {
    let store = read_settings(path)?;
    let Some(host) = store
        .spaces
        .get(local_space)
        .and_then(|share| share.host.as_ref())
    else {
        return Ok(None);
    };
    if !rule_grant_matches(&store, host) {
        return Ok(None);
    }
    Ok(store
        .team_rules
        .get(&host.url)
        .and_then(|layers| layers.get(&host.space))
        .filter(|layer| layer.project.is_some())
        .cloned())
}

fn rule_grant_matches(store: &SettingsStore, host: &HostLink) -> bool {
    store.memberships.get(&host.url).is_none_or(|membership| {
        membership.key == host.key
            && membership.rule_grants.as_ref().is_none_or(|grants| {
                grants.get(&host.space).is_some_and(|id| {
                    id.as_ref()
                        .is_none_or(|id| host.space_id.as_ref().is_none_or(|known| known == id))
                })
            })
    })
}

pub(crate) fn team_rule_proposal_target(
    path: &Path,
    team_id: &str,
    revision: &str,
) -> Result<(HostLink, ShareSettings), WenlanError> {
    let store = read_settings(path)?;
    let Some((url, space)) = store.team_rules.iter().find_map(|(url, layers)| {
        layers
            .iter()
            .find(|(_, layer)| layer.id == team_id && layer.revision == revision)
            .map(|(space, _)| (url, space))
    }) else {
        return Err(WenlanError::Conflict(
            "sync the approved team Rules before proposing a change".into(),
        ));
    };
    store
        .spaces
        .values()
        .find_map(|share| {
            let host = share.host.as_ref()?;
            (host.url == *url && host.space == *space && rule_grant_matches(&store, host))
                .then(|| (host.clone(), share.clone()))
        })
        .ok_or_else(|| WenlanError::NotFound("the team Rules share".into()))
}

/// Keep the last successful pull through network failures. A current team
/// credential that is explicitly refused cannot keep its renderer layer.
fn clear_team_rule_layer(
    path: &Path,
    url: &str,
    key: &str,
    space: Option<&str>,
) -> Result<(), WenlanError> {
    change_settings(path, |store| {
        let current_key = match store.memberships.get(url) {
            Some(membership) => membership.key == key,
            None => store.spaces.values().any(|share| {
                share
                    .host
                    .as_ref()
                    .is_some_and(|host| host.url == url && host.key == key)
            }),
        };
        if !current_key {
            return (false, ());
        }
        let mut changed = match space {
            Some(space) => remove_cached_rule_scope(store, url, space),
            None => store.team_rules.remove(url).is_some(),
        };
        if let Some(membership) = store.memberships.get_mut(url) {
            changed |= membership.rule_grants.is_none();
            let grants = membership.rule_grants.get_or_insert_with(BTreeMap::new);
            match space {
                Some(space) => changed |= grants.remove(space).is_some(),
                None => {
                    changed |= !grants.is_empty();
                    grants.clear();
                }
            }
        }
        (changed, ())
    })
}

/// Manual sync checks the member's grant before it requests a Space file
/// list. Retire cached Rules even if that preflight refuses the sync.
pub async fn team_space_grant_for_sync(
    path: &Path,
    url: &str,
    key: &str,
    space: &str,
) -> Result<wenlan_types::team::TeamMe, WenlanError> {
    let me = match remote::me_sync(url, key).await {
        Ok(me) => me,
        Err(error) => {
            if matches!(
                error,
                remote::SyncError::Revoked(_) | remote::SyncError::Refused(_)
            ) {
                clear_team_rule_layer(path, url, key, None)?;
            }
            return Err(WenlanError::Validation(error.to_string()));
        }
    };
    if !me.spaces.iter().any(|grant| grant.space == space) {
        clear_team_rule_layer(path, url, key, Some(space))?;
    }
    Ok(me)
}

fn remove_cached_rule_scope(store: &mut SettingsStore, url: &str, space: &str) -> bool {
    let Some(layers) = store.team_rules.get_mut(url) else {
        return false;
    };
    let changed = layers.remove(space).is_some();
    if layers.is_empty() {
        store.team_rules.remove(url);
    }
    changed
}

fn remember_team_rule_pull(
    path: &Path,
    scope: &Scope,
    host: &HostLink,
    layer: Option<TeamRuleLayer>,
) -> Result<(), WenlanError> {
    change_settings(path, |store| {
        // A leave or a rejoin under a new key may have won while pulling.
        if store
            .spaces
            .get(&scope.key())
            .and_then(|share| share.host.as_ref())
            != Some(host)
            || !rule_grant_matches(store, host)
        {
            return (false, ());
        }
        match layer {
            Some(layer) => {
                let layers = store.team_rules.entry(host.url.clone()).or_default();
                if layers.get(&host.space).is_some_and(|before| {
                    before.id == layer.id
                        && before.name == layer.name
                        && before.revision == layer.revision
                        && before.project == layer.project
                        && before.skills == layer.skills
                        && before.synced_at.get(..10) == layer.synced_at.get(..10)
                }) {
                    return (false, ());
                }
                layers.insert(host.space.clone(), layer);
                (true, ())
            }
            None => (remove_cached_rule_scope(store, &host.url, &host.space), ()),
        }
    })
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

/// The teams this person is on, for showing: each server's address, the
/// team as the server last said, and whether they own it. Never a key.
pub fn membership_infos_at(
    path: &Path,
) -> Result<Vec<wenlan_types::team::TeamMembershipInfo>, WenlanError> {
    let store = read_settings(path)?;
    Ok(store
        .memberships
        .iter()
        .map(|(url, membership)| wenlan_types::team::TeamMembershipInfo {
            url: url.clone(),
            team: store.teams.get(url).cloned(),
            owner: membership.owner,
        })
        .collect())
}

/// The team each team Space here belongs to, by Space name: Spaces synced
/// with a team server, and Spaces this computer hosts for its own team.
/// `None` for a team whose name this computer does not know yet.
pub fn team_names_by_space(path: &Path) -> Result<BTreeMap<String, Option<String>>, WenlanError> {
    let store = read_settings(path)?;
    let own_team = || -> Result<Option<String>, WenlanError> {
        Ok(crate::team::read_team(&crate::team::team_path())?
            .info()
            .and_then(|info| info.name))
    };
    let mut out = BTreeMap::new();
    for (space, settings) in &store.spaces {
        let name = match (&settings.host, settings.hosted) {
            (Some(link), _) => store
                .teams
                .get(&link.url)
                .and_then(|team| team.name.clone()),
            (None, true) => own_team()?,
            (None, false) => continue,
        };
        out.insert(space.clone(), name.filter(|name| !name.trim().is_empty()));
    }
    Ok(out)
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
            remove_cached_rule_scope(store, &host.url, &host.space);
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

/// Exact version hashes the host has published in the current page stay.
/// A member-supplied stay token alone cannot authorize a file: this receipt is
/// minted only by the hosted sync pass after validating its lineage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct MemberViewReceipt {
    pub stay: String,
    pub hashes: HashSet<String>,
    /// For a page deleted here: the one archive verified as its final
    /// version in this stay. Only it is served, and only it stands in for
    /// the deletion ID; an older archive from the same stay never does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_archive: Option<String>,
}

fn member_view_receipts_key(space: &str) -> String {
    format!("shared_space.member_view_receipts:{space}")
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

/// Page identities the old host may have sent on this folder. The folder
/// witness keeps receipts for a different share of the same name out.
pub async fn held_hosted_page_ids(
    db: &MemoryDB,
    space: &str,
    folder: &Path,
) -> Result<impl Iterator<Item = String>, WenlanError> {
    let marker = format!("{}.folder", held_key(space));
    let matched =
        db.get_app_metadata(&marker).await?.as_deref() == Some(folder.to_string_lossy().as_ref());
    let held = if matched {
        read_held(db, space).await?
    } else {
        BTreeMap::new()
    };
    Ok(held.into_keys())
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
            Err(reason) => {
                clear_rule_layer_if_refused(settings_path, &host, &reason)?;
                return Ok(Err(reason.to_string()));
            }
            Ok((listed, layer)) => {
                remember_team_rule_pull(settings_path, scope, &host, layer)?;
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
            if crate::rulesync_personal::enabled() {
                let path = settings_path.to_path_buf();
                let scope = space.clone();
                tokio::task::spawn_blocking(move || {
                    rulesync_team::ensure_hosted_master(&path, &scope)
                })
                .await
                .map_err(|error| WenlanError::Generic(format!("team Rules task: {error}")))??;
            }
            list_team_members(settings_path, &folder, space).await;
        }
    }
    let edit_settle = if automatic { EDIT_SETTLE_SECS } else { 0 };
    let deleted_pages = listed
        .as_ref()
        .map(|(_, listed)| listed.deleted_pages.as_slice())
        .unwrap_or_default();
    let counts = pass(
        db,
        scope,
        &settings,
        settings_path,
        knowledge_path,
        edit_settle,
        deleted_pages,
    )
    .await?;
    if matches!(scope, Scope::Library) {
        library::main_jobs(db, settings_path, &settings, &counts).await;
    }
    if let Some((host, listed)) = listed {
        if let Err(reason) = remote::push(&host, &folder, &settings.member_id, &listed).await {
            clear_rule_layer_if_refused(settings_path, &host, &reason)?;
            return Ok(Err(reason.to_string()));
        }
    }
    Ok(Ok(counts))
}

fn clear_rule_layer_if_refused(
    settings_path: &Path,
    host: &HostLink,
    reason: &remote::SyncError,
) -> Result<(), WenlanError> {
    match reason {
        remote::SyncError::Revoked(_) => {
            clear_team_rule_layer(settings_path, &host.url, &host.key, None)
        }
        remote::SyncError::Refused(_) => {
            clear_team_rule_layer(settings_path, &host.url, &host.key, Some(&host.space))
        }
        remote::SyncError::Unavailable(_) => Ok(()),
    }
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
    /// Page identities the hosted Space has deleted without a safe archive
    /// body. These are applied locally by identity and never re-exported.
    deleted_pages: HashSet<String>,
    /// Member id to name, from `space.json`.
    names: HashMap<String, String>,
    team_owner: Option<String>,
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

fn anchored_member_hashes(
    versions: &[PageVersion],
    root: &str,
    stay: &str,
    space_pages: &HashMap<String, String>,
) -> HashSet<String> {
    let eligible: Vec<&PageVersion> = versions
        .iter()
        .filter(|version| {
            crate::member_view::MemberSnapshot::matches_in_stay(version, space_pages, stay)
        })
        .collect();
    if !eligible.iter().any(|version| version.sha == root) {
        return HashSet::new();
    }
    let mut hashes = HashSet::from([root.to_string()]);
    loop {
        let before = hashes.len();
        for version in &eligible {
            let references = version.meta.parent.iter().chain(version.meta.merged.iter());
            if hashes.contains(&version.sha) {
                hashes.extend(references.cloned());
            } else if references.into_iter().any(|sha| hashes.contains(sha)) {
                hashes.insert(version.sha.clone());
            }
        }
        if hashes.len() == before {
            break;
        }
    }
    hashes
        .into_iter()
        .filter(|sha| eligible.iter().any(|version| &version.sha == sha))
        .collect()
}

fn member_view_receipts(
    pages: &HashMap<String, String>,
    current_stays: &HashMap<String, String>,
    held: &BTreeMap<String, Held>,
    read: &version::FolderRead,
) -> HashMap<String, MemberViewReceipt> {
    let mut receipts = HashMap::new();
    for page_id in pages.keys() {
        let Some(stay) = current_stays.get(page_id) else {
            continue;
        };
        let Some(root) = held
            .get(page_id)
            .filter(|held| held.version.is_some())
            .map(|held| held.sha.as_str())
        else {
            continue;
        };
        let hashes = anchored_member_hashes(
            read.pages
                .get(page_id)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            root,
            stay,
            pages,
        );
        if hashes.is_empty() {
            continue;
        }
        let mut all = hashes.clone();
        if let Some(proposals) = read.proposals.get(page_id) {
            let proposal_hashes: HashSet<String> = proposals
                .iter()
                .filter(|proposal| {
                    proposal.meta.stay.as_deref() == Some(stay.as_str())
                        && proposal
                            .meta
                            .parent
                            .as_ref()
                            .is_some_and(|parent| hashes.contains(parent))
                        && crate::member_view::MemberSnapshot::matches_in_stay(
                            proposal, pages, stay,
                        )
                })
                .map(|proposal| proposal.sha.clone())
                .collect();
            all.extend(proposal_hashes.iter().cloned());
            if let Some(decisions) = read.decisions.get(page_id) {
                all.extend(
                    decisions
                        .iter()
                        .filter(|decision| {
                            decision.meta.stay.as_deref() == Some(stay.as_str())
                                && decision.meta.decides.as_ref().is_some_and(|decides| {
                                    proposal_hashes.contains(&decides.proposal)
                                })
                                && crate::member_view::MemberSnapshot::matches_in_stay(
                                    decision, pages, stay,
                                )
                        })
                        .map(|decision| decision.sha.clone()),
                );
            }
        }
        receipts.insert(
            page_id.clone(),
            MemberViewReceipt {
                stay: stay.clone(),
                hashes: all,
                final_archive: None,
            },
        );
    }
    receipts
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
    deleted_pages: &[String],
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
    if !matches!(scope, Scope::Page(_)) {
        // A "How I work" page is this person's own: rules taken from what
        // they wrote, for their own AI tools. It never travels with the
        // Space or the library; a teammate's Wenlan keeps its own. Sharing
        // that one page on purpose still works.
        for id in db.personal_rule_page_ids().await? {
            local.remove(&id);
            left_out.insert(id);
        }
    }
    if matches!(scope, Scope::Space(_)) {
        // A source page can be a verbatim projection of a local import.
        local.retain(|id, page| {
            let shared = !matches!(page.creation_kind.as_str(), "source" | "imported")
                && !crate::document_enrichment::is_source_page_id(id);
            if !shared {
                left_out.insert(id.clone());
            }
            shared
        });
    }
    if settings.hosted {
        let eligible = db.shared_folder_pages(&space).await?;
        local.retain(|id, _| {
            // The project master has its own approval and download flow.
            let allowed = eligible.shared.contains_key(id) || rulesync_team::is_master_page(id);
            if !allowed {
                left_out.insert(id.clone());
            }
            allowed
        });
    }
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
    let team_host = matches!(scope, Scope::Space(_)) && settings.hosted;
    if !team_host {
        local.retain(|id, _| !rulesync_team::is_master_page(id));
        read.pages
            .retain(|id, _| !rulesync_team::is_master_page(id));
        read.proposals
            .retain(|id, _| !rulesync_team::is_master_page(id));
        read.decisions
            .retain(|id, _| !rulesync_team::is_master_page(id));
    }
    if let Scope::Page(id) = scope {
        // A page folder holds that page only; anything else in it is not
        // taken.
        read.pages.retain(|page, _| page == id);
        read.proposals.retain(|page, _| page == id);
        read.decisions.retain(|page, _| page == id);
    }
    if !matches!(scope, Scope::Page(_)) {
        // Library pages with their own folder keep it: another person's
        // sentences on them must not be filed under this person's id here.
        // The same holds for this person's own "How I work" pages.
        let shared = |page: &String| {
            !left_out.contains(page)
                && (!matches!(scope, Scope::Space(_))
                    || !crate::document_enrichment::is_source_page_id(page))
        };
        read.pages.retain(|page, _| shared(page));
        read.proposals.retain(|page, _| shared(page));
        read.decisions.retain(|page, _| shared(page));
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
            .filter(|page| !rulesync_team::is_master_page(&page.id))
            .map(|page| (page.id.clone(), page.title.clone()))
            .collect(),
        deleted_pages: deleted_pages.iter().cloned().collect(),
        names: space_file
            .members
            .into_iter()
            .map(|member| (member.id, member.name))
            .collect(),
        team_owner: if team_host {
            rulesync_team::hosted_owner(settings_path)?
        } else {
            None
        },
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
        .chain(deleted_pages.iter().map(String::as_str))
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
        if ctx.deleted_pages.contains(id) && !rulesync_team::is_master_page(id) {
            if let Some(page) = local.get(id).filter(|page| page.status != "archived") {
                if apply_server_deletion(db, page, ctx.knowledge_path).await? {
                    counts.applied += 1;
                }
            }
            continue;
        }
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
    let team_approvals = read
        .pages
        .iter()
        .filter(|(page, _)| rulesync_team::is_master_page(page))
        .flat_map(|(page, versions)| versions.iter().map(move |version| (page, version)))
        .filter_map(|(page, version)| {
            Some((page.as_str(), rulesync_team::approved_proposal(version)?))
        })
        .collect::<Vec<_>>();
    ctx.review(
        &read.pages,
        &read.proposals,
        &read.decisions,
        &heads,
        team_approvals,
    )
    .await?;
    if team_host {
        // Only hashes connected to the host's persisted current head are
        // downloadable. Re-read after page sync/review so newly written roots
        // and decisions can receive receipts in this pass.
        let folder = ctx.folder.clone();
        let published = tokio::task::spawn_blocking(move || version::read_folder(&folder))
            .await
            .map_err(|error| WenlanError::Generic(format!("shared folder task: {error}")))?;
        let current = db.shared_folder_pages(&ctx.space).await?;
        let mut receipts = member_view_receipts(&current.shared, &current.stays, &held, &published);
        for id in &current.deleted {
            let Some(stay) = current.stays.get(id) else {
                continue;
            };
            let Some(mut receipt) = current
                .receipts
                .get(id)
                .filter(|receipt| &receipt.stay == stay)
                .cloned()
            else {
                continue;
            };
            // Verified again each pass: a final archive that no longer
            // checks out stops standing in for the deletion.
            receipt.final_archive = None;
            if let Some(archive) = held
                .get(id)
                .filter(|held| held.version.is_none())
                .and_then(|held| {
                    published
                        .pages
                        .get(id)?
                        .iter()
                        .find(|version| version.sha == held.sha && version.meta.archived)
                })
                .filter(|archive| {
                    archive
                        .meta
                        .parent
                        .as_ref()
                        .is_some_and(|parent| receipt.hashes.contains(parent))
                        && crate::member_view::MemberSnapshot::matches_in_stay(
                            archive,
                            &current.shared,
                            stay,
                        )
                })
            {
                receipt.hashes.insert(archive.sha.clone());
                receipt.final_archive = Some(archive.sha.clone());
            }
            receipts.insert(id.clone(), receipt);
        }
        db.set_app_metadata(
            &member_view_receipts_key(&ctx.space),
            &serde_json::to_string(&receipts)?,
        )
        .await?;
    }
    match failed {
        Some(error) => Err(error),
        None => Ok(counts),
    }
}

async fn apply_server_deletion(
    db: &MemoryDB,
    page: &Page,
    knowledge_path: Option<&Path>,
) -> Result<bool, WenlanError> {
    let Some(version) = db.archive_page_at_version(&page.id, page.version).await? else {
        return Ok(false);
    };
    if let Some(root) = knowledge_path {
        let projection =
            crate::export::knowledge::KnowledgeProjectionWrite::new(root.to_path_buf(), db);
        if let Err(error) = projection
            .archive_page_if_still_archived(db, &page.id, version)
            .await
        {
            log::warn!(
                "[shared_space] archived deleted {} but its file stays: {error}",
                page.id
            );
        }
    }
    Ok(true)
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
            body_digest: version::body_digest(
                &self.shared_body(&page.content, &local_citations(page)),
            ),
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
    /// What a member of this share may see, for a team share.
    fn member_scope(&self) -> ReadScope {
        match self.scope {
            Scope::Page(_) => ReadScope::Uncategorized,
            _ => ReadScope::Space(self.space.clone()),
        }
    }

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
        let hosted_versions;
        let versions = if self.settings.hosted && !rulesync_team::is_master_page(&page.id) {
            let Some(stay) = self.db.shared_page_stay_token(page, &self.space).await? else {
                counts.skipped += 1;
                return Ok(None);
            };
            let anchored = held
                .filter(|held| held.version.is_some())
                .map(|held| anchored_member_hashes(versions, &held.sha, &stay, &self.space_pages))
                .unwrap_or_default();
            hosted_versions = versions
                .iter()
                .filter(|version| anchored.contains(&version.sha))
                .cloned()
                .collect::<Vec<_>>();
            if hosted_versions.is_empty() && !versions.is_empty() {
                // Legacy files and files from a prior stay have no edge to
                // this stay. Share the live page as a new root.
                let Some(sha) = self.export_page(page, &[], None, None, None).await? else {
                    counts.skipped += 1;
                    return Ok(None);
                };
                counts.exported += 1;
                return Ok(Some(Held {
                    sha,
                    version: Some(page.version),
                    space: self.held_space_of(&page.id).await?,
                }));
            }
            &hosted_versions
        } else {
            versions
        };
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
        if let Some(head) = head.as_ref().filter(|head| {
            let mut seen = self.version_state(head.winner);
            if self.settings.hosted && !rulesync_team::is_master_page(&page.id) {
                seen.body_digest = version::body_digest(
                    &self.shared_body(&head.winner.page.body, &head.winner.page.citations),
                );
            }
            seen == state
        }) {
            // Already the head (an edit made here and elsewhere alike, or a
            // version taken earlier).
            return Ok(Some(at(&head.winner.sha)));
        }
        let team_rules = rulesync_team::is_master_page(&page.id);
        if team_rules && changed {
            if let Some(head) = &head {
                if head.winner.meta.body_digest != state.body_digest {
                    let author = self.author_of(page).await?;
                    let revision = base.map_or(head.winner.sha.as_str(), |base| base.sha.as_str());
                    self.propose_team_rules(
                        &page.id,
                        &page.content,
                        revision,
                        author,
                        edit_ts(&page.last_modified),
                    )
                    .await?;
                }
            }
        }
        // A member who may only read writes no version: the team's version
        // is taken over the edit, which stays in this page's history.
        let changed = changed && !self.settings.read_only && !team_rules;
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
        let held = if rulesync_team::is_master_page(id) {
            None
        } else {
            held
        };
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
        // On a team host a member's version never brings back a page
        // deleted here (one edited since, or restored elsewhere since): it
        // stays deleted, and the pass counts it as skipped.
        let team_host = self.settings.hosted && !self.library;
        let restored_since = matches!(
            held,
            Some(Held { sha, version: None, .. }) if sha != &winner.sha && !winner.meta.archived
        );
        if team_host && (edited_since || restored_since) {
            log::warn!(
                "[shared_space] {id} was deleted here and a member edited it since; it stays deleted"
            );
            counts.skipped += 1;
            return Ok(Some(deleted(&winner.sha)));
        }
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
                    // On a team host it is still deleted here: forgetting
                    // that would make it again from the folder next pass.
                    None if team_host => {
                        counts.skipped += 1;
                        Ok(Some(deleted(&winner.sha)))
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
                    if self.settings.hosted && !rulesync_team::is_master_page(id) {
                        let current = self.reload(id).await?;
                        let Some(sha) = self
                            .export_page(
                                &current,
                                versions,
                                Some(&head),
                                Some(winner),
                                Some(&winner.sha),
                            )
                            .await?
                        else {
                            counts.skipped += 1;
                            return Ok(None);
                        };
                        counts.exported += 1;
                        return Ok(Some(Held {
                            sha,
                            version: Some(current.version),
                            space: self.held_space_of(id).await?,
                        }));
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
        // A team share compares only titles its members may see: a rename
        // around any other title would tell them that title exists here. A
        // page shared alone shows its members no other page.
        if !self.library && matches!(self.scope, Scope::Page(_)) {
            return Ok(HashSet::new());
        }
        let member_scope = self.member_scope();
        let mut taken = HashSet::new();
        for page in self
            .db
            .list_pages_by_space("active", Some(&space), 1_000_000, 0)
            .await?
        {
            let in_space = if self.library {
                page.space.as_deref() == named_space(Some(&space))
            } else {
                page.space.as_deref() == Some(self.space.as_str())
                    && self
                        .db
                        .member_can_access_page(
                            &page.id,
                            &member_scope,
                            crate::db::MemberPageTarget::Published,
                        )
                        .await?
            };
            if page.id != id && in_space {
                taken.insert(MemoryDB::page_title_key(&page.title));
            }
        }
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
                file.citations
                    .retain(|citation| !rewrite::is_private_placeholder(&citation.locator));
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
            member_write: false,
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
        let current_shared_body = self.shared_body(&current.content, &local_citations(&current));
        if current.status == "active"
            && version::body_digest(&current_shared_body) != head.meta.body_digest
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
    /// sources private.
    async fn apply_sources(&self, id: &str, head: &PageVersion) -> Result<(), WenlanError> {
        if !self.library {
            return Ok(());
        }
        let mut file = head.page.clone();
        file.citations
            .retain(|citation| !rewrite::is_private_placeholder(&citation.locator));
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
            // A team share imports a member's page: it gets the member-write
            // gate, including no library-wide link sweep.
            member_write: !self.library,
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
        let published = if self.library {
            self.db.publish_page_draft(id, draft.version).await
        } else {
            self.db
                .publish_shared_page_draft(id, draft.version, &self.member_scope())
                .await
        };
        let page = match published {
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
        let (mut parent, mut merged) = match parent_sha {
            Some(_) => (None, Vec::new()),
            None => base_of(&history, versions, head, base),
        };
        let hosted_page = self.settings.hosted && !rulesync_team::is_master_page(&page.id);
        let stay = if hosted_page {
            let Some(stay) = db.shared_page_stay_token(page, &self.space).await? else {
                return Ok(None);
            };
            Some(stay)
        } else {
            base.or_else(|| head.map(|head| head.winner))
                .and_then(|parent| parent.meta.stay.clone())
        };
        if hosted_page
            && stay.as_ref().is_some_and(|stay| {
                parent
                    .as_ref()
                    .is_some_and(|parent| parent.meta.stay.as_ref() != Some(stay))
            })
        {
            // A new continuous stay starts a new history root. In particular,
            // do not make a re-entered page's snapshot a descendant of a
            // version from its previous stay.
            parent = None;
            merged.clear();
        }
        let parent_sha = parent_sha.filter(|sha| {
            !hosted_page || base.is_some_and(|base| base.sha == *sha && base.meta.stay == stay)
        });
        let (mut citations, mut basis) = local_sources(db, page).await?;
        // A source another member gave a sentence that is still on the page
        // stays: this Wenlan has no record of it, the version it came in does.
        if let Some(parent) = parent.or(base.filter(|_| parent_sha.is_some())) {
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

        let shared_body = self.shared_body(&page.content, &citations);
        let mut file = PageFile::new(&page.title, &shared_body);
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
        (file.citations, file.basis) = self.shared_sources(&citations, &basis);
        let meta = VersionMeta {
            parent: parent
                .map(|parent| parent.sha.clone())
                .or(parent_sha.map(str::to_string)),
            merged,
            body_digest: version::body_digest(&shared_body),
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
            stay,
        };
        let member = meta.author.member.clone();
        let text = if self.settings.hosted || self.settings.host.is_some() {
            crate::member_view::MemberSnapshot::new(&file, &meta, &self.space_pages).render()
        } else {
            version::render(&file, &meta)
        };
        let name = version::version_file_name(&version::sha256_hex(text.as_bytes()));
        match version::parse(&name, text.as_bytes()) {
            Ok(read) if read.page.body == shared_body && read.page.title == page.title => {}
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
        let stay = if self.settings.hosted {
            let pages = self.db.shared_folder_pages(&self.space).await?;
            let Some(stay) = pages.stays.get(&id).filter(|_| {
                pages
                    .authorized
                    .get(&id)
                    .is_some_and(|hashes| hashes.contains(&winner.sha))
            }) else {
                return Ok(None);
            };
            Some(stay.clone())
        } else {
            winner.meta.stay.clone()
        };
        let mut page = winner.page.clone();
        page.body = self.shared_body(&winner.page.body, &winner.page.citations);
        (page.citations, page.basis) = self.shared_sources(&page.citations, &page.basis);
        let meta = VersionMeta {
            parent: Some(winner.sha.clone()),
            merged: head
                .leaves
                .iter()
                .filter(|leaf| leaf.sha != winner.sha)
                .map(|leaf| leaf.sha.clone())
                .collect(),
            body_digest: version::body_digest(&page.body),
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
            stay,
        };
        let text = if self.settings.hosted || self.settings.host.is_some() {
            crate::member_view::MemberSnapshot::new(&page, &meta, &self.space_pages).render()
        } else {
            version::render(&page, &meta)
        };
        self.write(&id, text).await.map(Some)
    }

    fn shared_sources(
        &self,
        citations: &[Citation],
        basis: &[Basis],
    ) -> (Vec<Citation>, Vec<Basis>) {
        rewrite::rewrite_sources(
            citations,
            basis,
            &rewrite::RewriteContext {
                space_pages: &self.space_pages,
                keep_all: self.library,
            },
        )
    }

    fn shared_body(&self, body: &str, citations: &[Citation]) -> String {
        rewrite::rewrite_body(
            body,
            citations,
            &rewrite::RewriteContext {
                space_pages: &self.space_pages,
                keep_all: self.library,
            },
        )
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

/// One line per stored marker, using its first occurrence.
fn local_citations(page: &Page) -> Vec<Citation> {
    let mut stored = page.citations.clone();
    stored.sort_by_key(|citation| citation.occurrence);
    let mut seen = HashSet::new();
    stored
        .iter()
        .filter(|citation| seen.insert(citation.marker))
        .map(|citation| Citation {
            marker: citation.marker,
            label: String::new(),
            locator: crate::store::export::pages::locator_of(citation),
        })
        .collect()
}

/// The stored sources block before the shared-folder rewrite.
async fn local_sources(
    db: &MemoryDB,
    page: &Page,
) -> Result<(Vec<Citation>, Vec<Basis>), WenlanError> {
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
    Ok((local_citations(page), basis))
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

#[cfg(test)]
#[path = "rulesync_team_test.rs"]
mod rulesync_team_tests;
