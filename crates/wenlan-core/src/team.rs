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
use wenlan_types::team::{TeamAccess, TeamMe, TeamMember, TeamSpaceAccess};

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
        self.spaces.get(space).copied()
    }

    pub fn to_wire(&self) -> TeamMember {
        TeamMember {
            id: self.id.clone(),
            name: self.name.clone(),
            spaces: self.space_list(),
            added_at: self.added_at.clone(),
            removed_at: self.removed_at.clone(),
        }
    }

    pub fn me(&self) -> TeamMe {
        TeamMe {
            member_id: self.id.clone(),
            name: self.name.clone(),
            spaces: self.space_list(),
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
    #[serde(default)]
    pub members: Vec<Member>,
}

fn format_one() -> u32 {
    FORMAT
}

impl Default for TeamFile {
    fn default() -> Self {
        Self {
            format: FORMAT,
            members: Vec::new(),
        }
    }
}

/// `team.json` under the data root.
pub fn team_path() -> PathBuf {
    crate::config::data_root().join(TEAM_FILE)
}

pub fn read_team(path: &Path) -> Result<TeamFile, WenlanError> {
    let file: TeamFile = match std::fs::read(path) {
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
    let member = Member {
        id: uuid::Uuid::new_v4().simple().to_string(),
        name,
        key_sha256: key_hash(&key),
        spaces: access_map(spaces),
        added_at: now(),
        removed_at: None,
    };
    change_team(path, |file| {
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
pub fn check_member_version(
    member: &str,
    page_id: &str,
    name: &str,
    bytes: &[u8],
) -> Result<(), WenlanError> {
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
    Ok(())
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
    for member in dirs(&members_dir) {
        let pages_dir = members_dir.join(&member).join(version::PAGES_DIR);
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
                    "{}/{member}/{}/{page}/{name}",
                    version::MEMBERS_DIR,
                    version::PAGES_DIR
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
                && *pages == version::PAGES_DIR
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
