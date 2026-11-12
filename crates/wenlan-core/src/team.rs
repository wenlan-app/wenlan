// SPDX-License-Identifier: Apache-2.0
//! A team server's member list: who may connect, with which key, to which
//! Spaces (see `wenlan_types::team`).
//!
//! A team server is an ordinary Wenlan that hosts some of its Spaces: each
//! one is shared (`shared_space`) through a folder of the server's own,
//! `<data root>/team-spaces/<id>/`, which the server alone writes to disk.
//! Members' desktops keep their own copy of that folder and sync it over
//! the team listener; members' AI tools reach the server's pages over its
//! `/mcp`. Both present the member's key.
//!
//! The list is `team.json` under the data root, outside the database (like
//! `shared-spaces.json`), so a rebuilt database keeps its members:
//!
//! ```text
//! {"format":1,"members":[{"id":…,"name":…,"key_sha256":…,"spaces":{"<Space>":"write"},"added_at":…}]}
//! ```
//!
//! Only a key's sha256 is kept: a key is shown once, when it is made. A
//! removed member stays listed with `removed_at`, so the Space's history
//! keeps their name; their key no longer matches.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wenlan_types::team::{TeamAccess, TeamInfo, TeamMe, TeamMember, TeamRole, TeamSpaceAccess};

use crate::error::WenlanError;
use crate::shared_space::{self, version, Scope, SpaceMember};

/// The file format this code reads and writes.
pub const FORMAT: u32 = 1;
/// The member list, under the data root.
pub const TEAM_FILE: &str = "team.json";
/// Hosted Spaces' folders, under the data root.
pub const SPACES_DIR: &str = "team-spaces";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub id: String,
    pub name: String,
    pub key_sha256: String,
    #[serde(default)]
    pub spaces: BTreeMap<String, TeamAccess>,
    pub added_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<String>,
    /// What they are on the team. Members added by the owner are members;
    /// the owner is the person at the server's own computer.
    #[serde(default, skip_serializing_if = "TeamRole::is_member")]
    pub role: TeamRole,
    /// The Spaces everyone on the team has ([`TeamFile::everyone`]), filled
    /// in when the list is read. Their own entry for a Space wins.
    #[serde(skip)]
    pub everyone: BTreeMap<String, TeamAccess>,
    /// The team ([`TeamFile::info`]), filled in when the list is read.
    #[serde(skip)]
    pub team: Option<TeamInfo>,
}

impl Member {
    pub fn is_active(&self) -> bool {
        self.removed_at.is_none()
    }

    /// What this member may do in `space`; `None` when they are removed or
    /// it is not one of theirs.
    pub fn access(&self, space: &str) -> Option<TeamAccess> {
        if !self.is_active() {
            return None;
        }
        // The owner can edit every Space the server hosts. (Whether `space`
        // is hosted at all is the caller's check, as for anyone.)
        if self.role == TeamRole::Owner {
            return Some(TeamAccess::Write);
        }
        self.spaces
            .get(space)
            .or_else(|| self.everyone.get(space))
            .copied()
    }

    pub fn to_wire(&self) -> TeamMember {
        TeamMember {
            id: self.id.clone(),
            name: self.name.clone(),
            spaces: self.space_list(),
            added_at: self.added_at.clone(),
            removed_at: self.removed_at.clone(),
            role: self.role,
        }
    }

    /// Who they are and every Space they can use, their own and everyone's
    /// (the owner: every Space the server hosts, `hosted`).
    pub fn me(&self, hosted: &[String]) -> TeamMe {
        let mut all = self.everyone.clone();
        all.extend(self.spaces.clone());
        if self.role == TeamRole::Owner {
            for space in hosted {
                all.insert(space.clone(), TeamAccess::Write);
            }
        }
        TeamMe {
            member_id: self.id.clone(),
            name: self.name.clone(),
            spaces: all
                .into_iter()
                .map(|(space, access)| TeamSpaceAccess { space, access })
                .collect(),
            team: self.team.clone(),
            role: self.role,
        }
    }

    fn space_list(&self) -> Vec<TeamSpaceAccess> {
        self.spaces
            .iter()
            .map(|(space, access)| TeamSpaceAccess {
                space: space.clone(),
                access: *access,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamFile {
    #[serde(default = "format_one")]
    pub format: u32,
    /// The team's id, made the first time the list is written; it never
    /// changes, so members' computers know the team wherever it moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The name the owner gave the team; every member sees it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub members: Vec<Member>,
    /// Spaces every member has, now and when added later (a team's
    /// handbook or wiki), by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub everyone: BTreeMap<String, TeamAccess>,
    /// Where the team listener listens, set when the owner starts the team
    /// from the app (`WENLAN_TEAM_ADDR` wins over it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<String>,
    /// The address put in invite links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

fn format_one() -> u32 {
    FORMAT
}

impl Default for TeamFile {
    fn default() -> Self {
        Self {
            format: FORMAT,
            id: None,
            name: None,
            members: Vec::new(),
            everyone: BTreeMap::new(),
            listen: None,
            url: None,
        }
    }
}

impl TeamFile {
    /// The team's id and name; `None` before the list was first written.
    pub fn info(&self) -> Option<TeamInfo> {
        self.id.as_ref().map(|id| TeamInfo {
            id: id.clone(),
            name: self.name.clone(),
        })
    }
}

/// `team.json` under the data root.
pub fn team_path() -> PathBuf {
    crate::config::data_root().join(TEAM_FILE)
}

pub fn read_team(path: &Path) -> Result<TeamFile, WenlanError> {
    let mut file: TeamFile = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            WenlanError::Validation(format!("{} cannot be read: {error}", path.display()))
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => TeamFile::default(),
        Err(error) => return Err(error.into()),
    };
    if file.format != FORMAT {
        return Err(WenlanError::Validation(format!(
            "{} is format {}; this Wenlan reads format {FORMAT}",
            path.display(),
            file.format
        )));
    }
    let info = file.info();
    for member in &mut file.members {
        member.everyone = file.everyone.clone();
        member.team = info.clone();
    }
    Ok(file)
}

/// Changes `team.json` under its lock and writes it back (0600 on Unix: it
/// holds key hashes).
fn change_team<T>(
    path: &Path,
    change: impl FnOnce(&mut TeamFile) -> Result<T, WenlanError>,
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
    let mut file = read_team(path)?;
    if file.id.is_none() {
        file.id = Some(uuid::Uuid::new_v4().simple().to_string());
    }
    let out = change(&mut file)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(serde_json::to_string_pretty(&file)?.as_bytes())?;
    tmp.flush()?;
    // A temp file is made readable by its owner only (0600 on Unix), and
    // keeps that when it replaces team.json.
    tmp.persist(path)
        .map_err(|error| WenlanError::Io(error.error))?;
    Ok(out)
}

/// A new member key: `wlt_` and 64 random hex digits (two v4 uuids).
pub fn new_key() -> String {
    format!(
        "wlt_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn key_hash(key: &str) -> String {
    version::sha256_hex(key.trim().as_bytes())
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The team's id and name, making the id when the team has none yet.
pub fn team_info(path: &Path) -> Result<TeamInfo, WenlanError> {
    if let Some(info) = read_team(path)?.info() {
        return Ok(info);
    }
    change_team(path, |file| Ok(file.info()))?
        .ok_or_else(|| WenlanError::Generic("the team has no id".into()))
}

/// Names the team. Every member's Wenlan shows the new name on its next
/// sync pass.
pub fn rename_team(path: &Path, name: &str) -> Result<TeamInfo, WenlanError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(WenlanError::Validation(
            "a team needs a name of 1 to 60 characters".into(),
        ));
    }
    change_team(path, |file| {
        file.name = Some(name.to_string());
        Ok(file.info())
    })?
    .ok_or_else(|| WenlanError::Generic("the team has no id".into()))
}

/// Sets where the team listener listens (`None`: off) and, when given, the
/// address put in invite links.
pub fn set_listen(path: &Path, listen: Option<&str>, url: Option<&str>) -> Result<(), WenlanError> {
    let listen = listen.map(str::trim).filter(|addr| !addr.is_empty());
    if let Some(addr) = listen {
        addr.parse::<std::net::SocketAddr>().map_err(|_| {
            WenlanError::Validation(format!("{addr:?} is not an address to listen on"))
        })?;
    }
    change_team(path, |file| {
        file.listen = listen.map(str::to_string);
        if let Some(url) = url.map(str::trim).filter(|url| !url.is_empty()) {
            file.url = Some(url.trim_end_matches('/').to_string());
        }
        Ok(())
    })
}

/// Sets the address put in invite links (`None`: none kept).
pub fn set_url(path: &Path, url: Option<&str>) -> Result<(), WenlanError> {
    let url = url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(|url| url.trim_end_matches('/').to_string());
    change_team(path, |file| {
        file.url = url;
        Ok(())
    })
}

/// The owner's member record and a new key for it, made when the team has
/// no owner yet: for a server nobody sits at (Docker), whose owner manages
/// the team from their own computer. Any earlier owner key stops working.
pub fn owner_key(path: &Path, name: &str) -> Result<(Member, String), WenlanError> {
    let name = clean_name(name)?;
    let key = new_key();
    let member = change_team(path, |file| {
        let info = file.info();
        let everyone = file.everyone.clone();
        let member = match file
            .members
            .iter_mut()
            .find(|member| member.role == TeamRole::Owner && member.is_active())
        {
            Some(owner) => {
                owner.key_sha256 = key_hash(&key);
                owner.clone()
            }
            None => {
                let owner = Member {
                    id: uuid::Uuid::new_v4().simple().to_string(),
                    name,
                    key_sha256: key_hash(&key),
                    spaces: BTreeMap::new(),
                    added_at: now(),
                    removed_at: None,
                    role: TeamRole::Owner,
                    everyone: BTreeMap::new(),
                    team: None,
                };
                file.members.push(owner.clone());
                owner
            }
        };
        Ok(Member {
            everyone,
            team: info,
            ..member
        })
    })?;
    Ok((member, key))
}

/// Whether the team has an owner who manages it from another computer.
pub fn has_owner(path: &Path) -> Result<bool, WenlanError> {
    Ok(read_team(path)?
        .members
        .iter()
        .any(|member| member.role == TeamRole::Owner && member.is_active()))
}

fn clean_name(name: &str) -> Result<String, WenlanError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(WenlanError::Validation(
            "a member needs a name of 1 to 80 characters".into(),
        ));
    }
    Ok(name.to_string())
}

fn access_map(spaces: &[TeamSpaceAccess]) -> BTreeMap<String, TeamAccess> {
    spaces
        .iter()
        .filter(|entry| !entry.space.trim().is_empty())
        .map(|entry| (entry.space.trim().to_string(), entry.access))
        .collect()
}

/// Adds a member. Returns them and their key, which is not kept.
pub fn add_member(
    path: &Path,
    name: &str,
    spaces: &[TeamSpaceAccess],
) -> Result<(Member, String), WenlanError> {
    let name = clean_name(name)?;
    let key = new_key();
    let mut member = Member {
        id: uuid::Uuid::new_v4().simple().to_string(),
        name,
        key_sha256: key_hash(&key),
        spaces: access_map(spaces),
        added_at: now(),
        removed_at: None,
        role: TeamRole::Member,
        everyone: BTreeMap::new(),
        team: None,
    };
    change_team(path, |file| {
        member.everyone = file.everyone.clone();
        member.team = file.info();
        file.members.push(member.clone());
        Ok(())
    })?;
    Ok((member, key))
}

fn find_mut<'a>(file: &'a mut TeamFile, id: &str) -> Result<&'a mut Member, WenlanError> {
    file.members
        .iter_mut()
        .find(|member| member.id == id)
        .ok_or_else(|| WenlanError::NotFound(format!("member {id}")))
}

/// Renames a member or changes their Spaces.
pub fn update_member(
    path: &Path,
    id: &str,
    name: Option<&str>,
    spaces: Option<&[TeamSpaceAccess]>,
) -> Result<Member, WenlanError> {
    let name = name.map(clean_name).transpose()?;
    change_team(path, |file| {
        let member = find_mut(file, id)?;
        if !member.is_active() {
            return Err(WenlanError::Validation(format!("member {id} was removed")));
        }
        if let Some(name) = name {
            member.name = name;
        }
        if let Some(spaces) = spaces {
            member.spaces = access_map(spaces);
        }
        Ok(member.clone())
    })
}

/// Gives every member `access` to `space`, now and when added later, or
/// (`None`) stops doing so. Members' own entries for it are kept.
pub fn set_everyone(
    path: &Path,
    space: &str,
    access: Option<TeamAccess>,
) -> Result<(), WenlanError> {
    let space = space.trim();
    if space.is_empty() {
        return Err(WenlanError::Validation("no Space named".into()));
    }
    change_team(path, |file| {
        match access {
            Some(access) => file.everyone.insert(space.to_string(), access),
            None => file.everyone.remove(space),
        };
        Ok(())
    })
}

/// Gives a member a new key; the old one stops working. For a key that was
/// lost or shown to the wrong person.
pub fn new_member_key(path: &Path, id: &str) -> Result<(Member, String), WenlanError> {
    let key = new_key();
    let member = change_team(path, |file| {
        let member = find_mut(file, id)?;
        if !member.is_active() {
            return Err(WenlanError::Validation(format!("member {id} was removed")));
        }
        member.key_sha256 = key_hash(&key);
        Ok(member.clone())
    })?;
    Ok((member, key))
}

/// Removes a member: their key stops working at once. What their computer
/// already synced stays there; nothing can take it back.
pub fn remove_member(path: &Path, id: &str) -> Result<Member, WenlanError> {
    change_team(path, |file| {
        let member = find_mut(file, id)?;
        if member.role == TeamRole::Owner {
            return Err(WenlanError::Validation(
                "the team's owner cannot be removed".into(),
            ));
        }
        if member.removed_at.is_none() {
            member.removed_at = Some(now());
        }
        Ok(member.clone())
    })
}

/// The active member whose key this is.
pub fn authenticate(path: &Path, key: &str) -> Result<Option<Member>, WenlanError> {
    let key = key.trim();
    if !wenlan_types::team::is_team_key(key) {
        return Ok(None);
    }
    let hash = key_hash(key);
    let file = read_team(path)?;
    // Compared in full whatever matches, so the time taken does not say how
    // much of a hash matched.
    let mut found = None;
    for member in file.members {
        let same = member
            .key_sha256
            .bytes()
            .zip(hash.bytes())
            .fold(member.key_sha256.len() ^ hash.len(), |acc, (a, b)| {
                acc | usize::from(a ^ b)
            })
            == 0;
        if same && member.is_active() {
            found = Some(member);
        }
    }
    Ok(found)
}

// ===== Hosted Spaces =====

/// The Spaces this Wenlan hosts for the team, with their folders.
pub fn hosted_spaces(settings_path: &Path) -> Result<Vec<(String, PathBuf)>, WenlanError> {
    Ok(shared_space::shares_at(settings_path)?
        .into_iter()
        .filter_map(|(scope, settings)| match scope {
            Scope::Space(name) if settings.hosted => Some((name, PathBuf::from(settings.folder))),
            _ => None,
        })
        .collect())
}

/// The folder of the hosted Space `space`.
pub fn hosted_folder(settings_path: &Path, space: &str) -> Result<Option<PathBuf>, WenlanError> {
    Ok(hosted_spaces(settings_path)?
        .into_iter()
        .find(|(name, _)| name == space)
        .map(|(_, folder)| folder))
}

/// A new folder for a Space this Wenlan starts hosting, beside the
/// settings file.
pub fn new_hosted_folder(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(SPACES_DIR)
        .join(uuid::Uuid::new_v4().simple().to_string())
}

/// Lists in the hosted Space's `space.json` every member who can use it,
/// with their current name, so every member's Wenlan shows their name.
/// Members already listed stay (the history keeps their name).
pub fn list_members_in_space_file(
    folder: &Path,
    space: &str,
    members: &[Member],
) -> Result<(), WenlanError> {
    let wanted: Vec<SpaceMember> = members
        .iter()
        .filter(|member| member.access(space).is_some())
        .map(|member| SpaceMember {
            id: member.id.clone(),
            name: member.name.clone(),
        })
        .collect();
    shared_space::add_space_members(folder, &wanted)
}

/// Everything the team listener needs to check a version a member sends:
/// it is whole, it is theirs, and it is filed under its page.
/// Returns the folder kind the file goes in (a version, a proposal or a
/// decision; see [`version::KIND_DIRS`]).
pub fn check_member_version(
    member: &str,
    page_id: &str,
    name: &str,
    bytes: &[u8],
) -> Result<&'static str, WenlanError> {
    if !version::is_safe_id(page_id) {
        return Err(WenlanError::Validation(format!(
            "{page_id} is not a page id"
        )));
    }
    let read = version::parse(name, bytes)?;
    if read.page.id.as_deref() != Some(page_id) {
        return Err(WenlanError::Validation(format!(
            "{name} is a version of another page"
        )));
    }
    if read.meta.author.member != member {
        return Err(WenlanError::Validation(format!(
            "{name} was written by another member"
        )));
    }
    Ok(version::file_kind(&read.meta))
}

/// Paths of every file in a hosted Space's folder, as the team listener
/// lists them: `space.json`, then each version file.
pub fn list_folder_files(folder: &Path) -> Vec<String> {
    let mut files = Vec::new();
    if folder.join(shared_space::SPACE_FILE).is_file() {
        files.push(shared_space::SPACE_FILE.to_string());
    }
    let dirs = |dir: &Path| -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut out: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| version::is_safe_id(name))
            .collect();
        out.sort();
        out
    };
    let members_dir = folder.join(version::MEMBERS_DIR);
    for (member, kind) in dirs(&members_dir)
        .into_iter()
        .flat_map(|member| version::KIND_DIRS.map(|kind| (member.clone(), kind)))
    {
        let pages_dir = members_dir.join(&member).join(kind);
        for page in dirs(&pages_dir) {
            let Ok(entries) = std::fs::read_dir(pages_dir.join(&page)) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| version::version_sha(name).is_some())
                .collect();
            names.sort();
            for name in names {
                files.push(format!(
                    "{}/{member}/{kind}/{page}/{name}",
                    version::MEMBERS_DIR
                ));
            }
        }
    }
    files
}

/// A path the team listener may serve from a hosted folder: `space.json` or
/// a version file. Anything else (`..`, other files) is refused.
pub fn served_path(folder: &Path, path: &str) -> Option<PathBuf> {
    if path == shared_space::SPACE_FILE {
        return Some(folder.join(path));
    }
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        [members, member, pages, page, name]
            if *members == version::MEMBERS_DIR
                && version::KIND_DIRS.contains(pages)
                && version::is_safe_id(member)
                && version::is_safe_id(page)
                && version::version_sha(name).is_some() =>
        {
            Some(
                folder
                    .join(members)
                    .join(member)
                    .join(pages)
                    .join(page)
                    .join(name),
            )
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "team_test.rs"]
mod tests;
