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
//! A member may also carry `identities`, tool `keys` and an `invite`; the
//! first write of any of those (or of a member with no `key_sha256`) makes
//! the file format 2, after copying the format 1 file to `team.json.v1.bak`.
//!
//! Only a key's sha256 is kept: a key is shown once, when it is made. A
//! removed member stays listed with `removed_at`, so the Space's history
//! keeps their name; their key no longer matches.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wenlan_types::team::{
    TeamAccess, TeamInfo, TeamMe, TeamMember, TeamPerson, TeamRole, TeamSpaceAccess,
};

use crate::error::WenlanError;
use crate::shared_space::{self, version, Scope, SpaceMember};

/// The format of a file nobody has used a newer feature in: every member
/// has a legacy key and none of the newer fields. Today's code reads only
/// this one.
pub const FORMAT: u32 = 1;
/// The format written once any member has no legacy key or carries
/// identities, tool keys or an invite. The current file is kept first as
/// [`V1_BACKUP_FILE`]. This code reads [`FORMAT`] and this; anything newer
/// is refused.
pub const FORMAT_V2: u32 = 2;
/// The copy of a format 1 file, made once before the first format 2 write.
pub const V1_BACKUP_FILE: &str = "team.json.v1.bak";
/// The member list, under the data root.
pub const TEAM_FILE: &str = "team.json";
/// Hosted Spaces' folders, under the data root.
pub const SPACES_DIR: &str = "team-spaces";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub id: String,
    pub name: String,
    /// The legacy member key's SHA-256. Absent only for a member who truly
    /// has no legacy key (which makes the file format 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_sha256: Option<String>,
    #[serde(default)]
    pub spaces: BTreeMap<String, TeamAccess>,
    pub added_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<String>,
    /// What they are on the team. Members added by the owner are members;
    /// the owner is the person at the server's own computer.
    #[serde(default, skip_serializing_if = "TeamRole::is_member")]
    pub role: TeamRole,
    /// Accounts at an identity provider linked to this member.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identities: Vec<Identity>,
    /// Keys for AI tools: they reach `/mcp` and nothing else.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<ToolKey>,
    /// An invite not yet redeemed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<Invite>,
    /// The Spaces everyone on the team has ([`TeamFile::everyone`]), filled
    /// in when the list is read. Their own entry for a Space wins.
    #[serde(skip)]
    pub everyone: BTreeMap<String, TeamAccess>,
    /// The team ([`TeamFile::info`]), filled in when the list is read.
    #[serde(skip)]
    pub team: Option<TeamInfo>,
}

/// A provider account linked to a member. Who it is is `(issuer, subject)`;
/// the email is for display only and never a key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub issuer: String,
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub linked_at: String,
}

/// A member's key for AI tools. Only its SHA-256 is kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolKey {
    pub id: String,
    pub name: String,
    pub sha256: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
}

/// A one-time invite token (its SHA-256), optionally for one email.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub token_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub expires_at: String,
    /// The account that used the invite but waits for the owner's approval
    /// ([`approve_pending`]). While it is set the token is spent: it cannot
    /// be used again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Identity>,
}

/// What kind of credential a request presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    LegacyKey,
    ToolKey,
    /// A browser's session cookie. The session itself lives in the server
    /// process; core only names the kind.
    BrowserSession,
}

/// Which credential matched: the member's legacy key, or one of their tool
/// keys (by its id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    LegacyKey,
    ToolKey { id: String },
    BrowserSession,
}

impl Credential {
    pub fn kind(&self) -> CredentialKind {
        match self {
            Credential::LegacyKey => CredentialKind::LegacyKey,
            Credential::ToolKey { .. } => CredentialKind::ToolKey,
            Credential::BrowserSession => CredentialKind::BrowserSession,
        }
    }
}

/// An authenticated request: the active member and what they presented.
/// Dereferences to the member.
#[derive(Debug, Clone)]
pub struct Authenticated {
    pub member: Member,
    pub credential: Credential,
}

impl std::ops::Deref for Authenticated {
    type Target = Member;
    fn deref(&self) -> &Member {
        &self.member
    }
}

impl Member {
    /// Whether this member needs the format 2 file.
    fn needs_v2(&self) -> bool {
        self.key_sha256.is_none()
            || !self.identities.is_empty()
            || !self.keys.is_empty()
            || self.invite.is_some()
    }

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
    /// (the owner: every Space the server hosts, `hosted`, by name and
    /// folder). A hosted Space carries its id, which a rename keeps.
    pub fn me(&self, hosted: &[(String, PathBuf)]) -> TeamMe {
        let mut all = self.everyone.clone();
        all.extend(self.spaces.clone());
        if self.role == TeamRole::Owner {
            for (space, _) in hosted {
                all.insert(space.clone(), TeamAccess::Write);
            }
        }
        let id_of = |space: &str| {
            hosted
                .iter()
                .find(|(name, _)| name == space)
                .and_then(|(_, folder)| hosted_space_id(folder))
        };
        TeamMe {
            member_id: self.id.clone(),
            name: self.name.clone(),
            spaces: all
                .into_iter()
                .map(|(space, access)| TeamSpaceAccess {
                    id: id_of(&space),
                    space,
                    access,
                })
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
                id: None,
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
    if file.format != FORMAT && file.format != FORMAT_V2 {
        return Err(WenlanError::Validation(format!(
            "{} is format {}; this Wenlan reads formats {FORMAT} and {FORMAT_V2}",
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

/// The people `member` shares a hosted Space with, `member` included, by id
/// and name only: who may have changed a page they can open. A removed
/// member stays listed, so their past changes keep their name; nobody who
/// shares no hosted Space with `member` is listed.
pub fn people_sharing_with(
    file: &TeamFile,
    member: &Member,
    hosted: &[(String, PathBuf)],
) -> Vec<TeamPerson> {
    let hosted_space = |space: &str| hosted.iter().any(|(name, _)| name == space);
    let mine: std::collections::BTreeSet<String> = member
        .me(hosted)
        .spaces
        .into_iter()
        .map(|entry| entry.space)
        .filter(|space| hosted_space(space))
        .collect();
    file.members
        .iter()
        .filter(|other| {
            other
                .me(hosted)
                .spaces
                .iter()
                .any(|entry| mine.contains(&entry.space))
        })
        .map(|other| TeamPerson {
            id: other.id.clone(),
            name: other.name.clone(),
            owner: other.role == TeamRole::Owner,
        })
        .collect()
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
    let read_format = file.format;
    if file.id.is_none() {
        file.id = Some(uuid::Uuid::new_v4().simple().to_string());
    }
    let out = change(&mut file)?;
    // A file stays format 1 until something only format 2 can hold is in it;
    // today's code cannot read a member without a legacy key, so the format 1
    // file is kept once, before the first such write.
    if file.members.iter().any(Member::needs_v2) {
        file.format = FORMAT_V2;
    }
    if file.format == FORMAT_V2 && read_format == FORMAT && path.is_file() {
        let mut name = path.as_os_str().to_owned();
        name.push(".v1.bak");
        let backup = PathBuf::from(name);
        if !backup.exists() {
            std::fs::copy(path, &backup)?;
        }
    }
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

/// Whether two key hashes are equal, compared in constant time.
fn same_hash(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.as_bytes().ct_eq(b.as_bytes()).into()
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
    let key = new_key();
    let member = set_owner_hash(path, name, &key_hash(&key))?;
    Ok((member, key))
}

/// The SHA-256 the server keeps of a member key (lowercase hex).
pub fn key_sha256(key: &str) -> String {
    key_hash(key)
}

/// Whether `hash` looks like a key's SHA-256: 64 lowercase hex digits.
pub fn is_key_sha256(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The owner's member record for a key the owner made themselves, given by
/// its SHA-256 only: their Wenlan keeps the key and puts just its hash in
/// the server's start command, so the key is never on that machine's
/// command line and the owner joins without reading the server's output.
/// Any earlier owner key stops working.
pub fn set_owner_hash(path: &Path, name: &str, hash: &str) -> Result<Member, WenlanError> {
    let name = clean_name(name)?;
    let hash = hash.trim();
    if !is_key_sha256(hash) {
        return Err(WenlanError::Validation(
            "an owner key's SHA-256 is 64 lowercase hex digits".into(),
        ));
    }
    change_team(path, |file| {
        let info = file.info();
        let everyone = file.everyone.clone();
        let member = match file
            .members
            .iter_mut()
            .find(|member| member.role == TeamRole::Owner && member.is_active())
        {
            Some(owner) => {
                owner.key_sha256 = Some(hash.to_string());
                owner.clone()
            }
            None => {
                let owner = Member {
                    id: uuid::Uuid::new_v4().simple().to_string(),
                    name,
                    key_sha256: Some(hash.to_string()),
                    spaces: BTreeMap::new(),
                    added_at: now(),
                    removed_at: None,
                    role: TeamRole::Owner,
                    identities: Vec::new(),
                    keys: Vec::new(),
                    invite: None,
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
    })
}

/// Whether `hash` is the SHA-256 of the team owner's key.
pub fn is_owner_hash(path: &Path, hash: &str) -> Result<bool, WenlanError> {
    let hash = hash.trim();
    Ok(read_team(path)?.members.iter().any(|member| {
        member.role == TeamRole::Owner
            && member.is_active()
            && member
                .key_sha256
                .as_deref()
                .is_some_and(|own| same_hash(own, hash))
    }))
}

/// Drops every member's access to `space`, and the team-wide access to it:
/// for a Space that stops being hosted, so a later Space of the same name
/// starts open to nobody but the owner.
pub fn forget_space_access(path: &Path, space: &str) -> Result<(), WenlanError> {
    let space = space.trim();
    if space.is_empty() || !path.exists() {
        return Ok(());
    }
    change_team(path, |file| {
        file.everyone.remove(space);
        for member in &mut file.members {
            member.spaces.remove(space);
        }
        Ok(())
    })
}

/// Moves every access to the Space `from` (everyone's and each member's)
/// to `to`: the Space was renamed. Does nothing without a team.
pub fn rename_space_access(path: &Path, from: &str, to: &str) -> Result<(), WenlanError> {
    let (from, to) = (from.trim(), to.trim());
    if from.is_empty() || to.is_empty() || from == to || !path.exists() {
        return Ok(());
    }
    change_team(path, |file| {
        if let Some(access) = file.everyone.remove(from) {
            file.everyone.insert(to.to_string(), access);
        }
        for member in &mut file.members {
            if let Some(access) = member.spaces.remove(from) {
                member.spaces.insert(to.to_string(), access);
            }
        }
        Ok(())
    })
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
        key_sha256: Some(key_hash(&key)),
        spaces: access_map(spaces),
        added_at: now(),
        removed_at: None,
        role: TeamRole::Member,
        identities: Vec::new(),
        keys: Vec::new(),
        invite: None,
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
        member.key_sha256 = Some(key_hash(&key));
        Ok(member.clone())
    })?;
    Ok((member, key))
}

/// A new tool key: `wlt_` and 64 hex digits from 32 random bytes.
fn new_tool_key() -> Result<String, WenlanError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| WenlanError::Generic(format!("no random bytes for a key: {error}")))?;
    Ok(format!("wlt_{}", hex::encode(bytes)))
}

fn find_active<'a>(file: &'a mut TeamFile, id: &str) -> Result<&'a mut Member, WenlanError> {
    let member = find_mut(file, id)?;
    if !member.is_active() {
        return Err(WenlanError::Validation(format!("member {id} was removed")));
    }
    Ok(member)
}

/// Gives a member a key for their AI tools; it reaches `/mcp` only. Returns
/// the key's id and the key, which is not kept.
pub fn issue_tool_key(
    path: &Path,
    member_id: &str,
    name: &str,
) -> Result<(String, String), WenlanError> {
    let name = clean_name(name)?;
    let key = new_tool_key()?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    change_team(path, |file| {
        let member = find_active(file, member_id)?;
        member.keys.push(ToolKey {
            id: id.clone(),
            name,
            sha256: key_hash(&key),
            created_at: now(),
            revoked_at: None,
        });
        Ok(())
    })?;
    Ok((id, key))
}

/// Revokes one of a member's tool keys; it stops working at the next request.
pub fn revoke_tool_key(path: &Path, member_id: &str, key_id: &str) -> Result<(), WenlanError> {
    change_team(path, |file| {
        let member = find_mut(file, member_id)?;
        let key = member
            .keys
            .iter_mut()
            .find(|key| key.id == key_id)
            .ok_or_else(|| WenlanError::NotFound(format!("key {key_id}")))?;
        if key.revoked_at.is_none() {
            key.revoked_at = Some(now());
        }
        Ok(())
    })
}

/// A member's account was stolen: every tool key is revoked, every linked
/// identity and the pending invite are dropped, and a fresh legacy key is
/// made (the old one stops working). The member's id, and so their history,
/// stays. Works on the owner too.
pub fn reset_compromised(path: &Path, member_id: &str) -> Result<(Member, String), WenlanError> {
    let key = new_key();
    let member = change_team(path, |file| {
        let member = find_active(file, member_id)?;
        let at = now();
        for tool_key in &mut member.keys {
            if tool_key.revoked_at.is_none() {
                tool_key.revoked_at = Some(at.clone());
            }
        }
        member.identities.clear();
        member.invite = None;
        member.key_sha256 = Some(key_hash(&key));
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

/// The member with this id as the file now says, removed or not; `None`
/// when there is no such member. A browser session names its member by id,
/// and the caller checks [`Member::is_active`] on every request.
pub fn member_by_id(path: &Path, id: &str) -> Result<Option<Member>, WenlanError> {
    Ok(read_team(path)?.members.into_iter().find(|m| m.id == id))
}

/// The active member with this provider account linked, if any. Who the
/// account is is `(issuer, subject)`; an email never finds a member. A
/// removed member is never found.
pub fn member_by_identity(
    path: &Path,
    issuer: &str,
    subject: &str,
) -> Result<Option<Member>, WenlanError> {
    Ok(read_team(path)?
        .members
        .into_iter()
        .find(|m| m.is_active() && has_identity(m, issuer, subject)))
}

fn has_identity(member: &Member, issuer: &str, subject: &str) -> bool {
    member
        .identities
        .iter()
        .any(|i| i.issuer == issuer && i.subject == subject)
}

/// Links a provider account to an active member, in one change under the
/// file's lock: refused with [`WenlanError::Conflict`] when another active
/// member already has the same `(issuer, subject)` (a removed member's
/// links no longer count: they cannot sign in, and the person may be
/// invited again as a new member). Linking what the member already has
/// changes only the email shown. `identity.linked_at` is set here.
pub fn link_identity(
    path: &Path,
    member_id: &str,
    mut identity: Identity,
) -> Result<Member, WenlanError> {
    if identity.issuer.trim().is_empty() || identity.subject.trim().is_empty() {
        return Err(WenlanError::Validation(
            "an identity needs an issuer and a subject".into(),
        ));
    }
    identity.linked_at = now();
    change_team(path, |file| {
        let taken = file.members.iter().any(|other| {
            other.id != member_id
                && other.is_active()
                && has_identity(other, &identity.issuer, &identity.subject)
        });
        if taken {
            return Err(WenlanError::Conflict(
                "that account is linked to another member".into(),
            ));
        }
        let member = find_active(file, member_id)?;
        match member
            .identities
            .iter_mut()
            .find(|i| i.issuer == identity.issuer && i.subject == identity.subject)
        {
            Some(known) => known.email = identity.email.clone(),
            None => member.identities.push(identity.clone()),
        }
        Ok(member.clone())
    })
}

// ===== Sign-in invites =====

/// How long an invite works.
pub const INVITE_DAYS: i64 = 7;

/// How much the signed-in email proves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmailTrust {
    /// The provider vouches the person owns the mailbox now (a Gmail
    /// address, an account of a company domain, or a GitHub verified
    /// email).
    Trusted,
    /// The provider calls the email verified but that does not show the
    /// person owns it now: a matching email waits for the owner's approval.
    Untrusted,
}

/// What redeeming an invite came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteOutcome {
    /// The account is linked to the member and the invite is gone.
    Linked(Member),
    /// The account is kept on the member as waiting; the owner approves it
    /// ([`approve_pending`]). The invite cannot be used again.
    PendingApproval(Member),
    /// Nothing changed.
    Refused(InviteRefusal),
}

/// Why an invite was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteRefusal {
    /// No live invite has this token, or its member is gone.
    Unknown,
    /// Older than [`INVITE_DAYS`]. The owner can send a new one.
    Expired,
    /// Already used (it may be waiting for the owner's approval).
    Used,
    /// The account belongs to another member.
    IdentityTaken,
    /// The invite is for another address; this one is the invite's, with
    /// all but the first letter of its name hidden.
    EmailMismatch { invited: String },
}

/// A new invite token: `inv_` and 64 hex digits from 32 random bytes.
fn new_invite_token() -> Result<String, WenlanError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| WenlanError::Generic(format!("no random bytes for an invite: {error}")))?;
    Ok(format!("inv_{}", hex::encode(bytes)))
}

/// The SHA-256 the server keeps of an invite token (lowercase hex).
pub fn invite_token_sha256(token: &str) -> String {
    key_hash(token)
}

/// `a***@example.com` for `amy@example.com`: enough to tell which address
/// was meant, not enough to learn it.
pub fn mask_email(email: &str) -> String {
    match email.trim().split_once('@') {
        Some((name, domain)) => {
            let first: String = name.chars().take(1).collect();
            format!("{first}***@{domain}")
        }
        None => "***".to_string(),
    }
}

fn clean_email(email: Option<&str>) -> Result<Option<String>, WenlanError> {
    let Some(email) = email.map(str::trim).filter(|email| !email.is_empty()) else {
        return Ok(None);
    };
    let plausible = email.len() <= 254
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && email
            .split_once('@')
            .is_some_and(|(name, domain)| !name.is_empty() && domain.contains('.'));
    if !plausible {
        return Err(WenlanError::Validation(
            "that is not an email address".into(),
        ));
    }
    Ok(Some(email.to_string()))
}

fn make_invite(email: Option<String>) -> Result<(Invite, String), WenlanError> {
    let token = new_invite_token()?;
    let expires_at = (chrono::Utc::now() + chrono::Duration::days(INVITE_DAYS))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let invite = Invite {
        token_sha256: key_hash(&token),
        email,
        expires_at,
        pending: None,
    };
    Ok((invite, token))
}

/// Adds a member who has no key yet and will sign in with their own account:
/// the member and an invite token, which is not kept (only its hash is).
/// With an `email`, only an account with that address may use the invite.
pub fn create_invited_member(
    path: &Path,
    name: &str,
    email: Option<&str>,
    spaces: &[TeamSpaceAccess],
) -> Result<(Member, String), WenlanError> {
    let name = clean_name(name)?;
    let email = clean_email(email)?;
    let (invite, token) = make_invite(email)?;
    let mut member = Member {
        id: uuid::Uuid::new_v4().simple().to_string(),
        name,
        key_sha256: None,
        spaces: access_map(spaces),
        added_at: now(),
        removed_at: None,
        role: TeamRole::Member,
        identities: Vec::new(),
        keys: Vec::new(),
        invite: Some(invite),
        everyone: BTreeMap::new(),
        team: None,
    };
    change_team(path, |file| {
        member.everyone = file.everyone.clone();
        member.team = file.info();
        file.members.push(member.clone());
        Ok(())
    })?;
    Ok((member, token))
}

/// A new invite for a member, in place of any they have: the old token (and
/// an account waiting for approval) is gone. Not for the owner.
pub fn reissue_invite(path: &Path, member_id: &str) -> Result<(Member, String), WenlanError> {
    change_team(path, |file| {
        let member = find_active(file, member_id)?;
        if member.role == TeamRole::Owner {
            return Err(WenlanError::Validation(
                "the team's owner is not invited".into(),
            ));
        }
        let email = member.invite.as_ref().and_then(|i| i.email.clone());
        let (invite, token) = make_invite(email)?;
        member.invite = Some(invite);
        Ok((member.clone(), token))
    })
}

/// The member holding the invite with this hash, compared in constant time
/// against every invite so the time says nothing about which one.
fn invite_holder<'a>(file: &'a mut TeamFile, token_hash: &str) -> Option<&'a mut Member> {
    let mut found = None;
    for (index, member) in file.members.iter().enumerate() {
        if let Some(invite) = &member.invite {
            if same_hash(&invite.token_sha256, token_hash) {
                found = Some(index);
            }
        }
    }
    found.map(|index| &mut file.members[index])
}

fn invite_expired(invite: &Invite) -> bool {
    chrono::DateTime::parse_from_rfc3339(&invite.expires_at)
        .map(|at| at <= chrono::Utc::now())
        .unwrap_or(true)
}

/// Whether the invite with this token hash can still be used right now:
/// there, not expired, not spent, its member active.
pub fn invite_usable(path: &Path, token_hash: &str) -> Result<bool, WenlanError> {
    let mut file = read_team(path)?;
    Ok(invite_holder(&mut file, token_hash).is_some_and(|member| {
        member.is_active()
            && member
                .invite
                .as_ref()
                .is_some_and(|i| i.pending.is_none() && !invite_expired(i))
    }))
}

/// Uses an invite token for the signed-in account `identity`.
/// `verified_emails` are all the addresses the provider vouches for as the
/// account's (one for Google, every verified one for GitHub); the invite
/// matches when any one of them is the invited address. `trust` says how far
/// to believe them (see [`EmailTrust`]).
/// See [`redeem_invite_hash`].
pub fn redeem_invite(
    path: &Path,
    token: &str,
    identity: Identity,
    verified_emails: &[String],
    trust: EmailTrust,
) -> Result<InviteOutcome, WenlanError> {
    redeem_invite_hash(
        path,
        &invite_token_sha256(token),
        identity,
        verified_emails,
        trust,
    )
}

/// [`redeem_invite`] for a token already hashed. Everything is checked and
/// changed in one change under the file's lock, so two people using one
/// invite at once cannot both succeed: the token's hash (in constant time),
/// that it has not expired nor been used, that its member is active, that
/// no other active member holds this `(issuer, subject)`, and, when the
/// invite names an email, that one of the verified addresses is it
/// (compared without regard to case). An invite without an email works for
/// whoever holds it. A matching email that is not trusted leaves the member
/// waiting for the owner ([`InviteOutcome::PendingApproval`]).
pub fn redeem_invite_hash(
    path: &Path,
    token_hash: &str,
    mut identity: Identity,
    verified_emails: &[String],
    trust: EmailTrust,
) -> Result<InviteOutcome, WenlanError> {
    use InviteOutcome::Refused;
    use InviteRefusal as No;
    if identity.issuer.trim().is_empty() || identity.subject.trim().is_empty() {
        return Err(WenlanError::Validation(
            "an identity needs an issuer and a subject".into(),
        ));
    }
    identity.linked_at = now();
    change_team(path, |file| {
        let (issuer, subject) = (identity.issuer.clone(), identity.subject.clone());
        let held_elsewhere = |file: &TeamFile, member_id: &str| {
            file.members.iter().any(|other| {
                other.id != member_id && other.is_active() && has_identity(other, &issuer, &subject)
            })
        };
        let Some(holder) = invite_holder(file, token_hash) else {
            return Ok(Refused(No::Unknown));
        };
        let member_id = holder.id.clone();
        let Some(invite) = holder.invite.clone().filter(|_| holder.is_active()) else {
            return Ok(Refused(No::Unknown));
        };
        if invite_expired(&invite) {
            return Ok(Refused(No::Expired));
        }
        if invite.pending.is_some() {
            return Ok(Refused(No::Used));
        }
        if held_elsewhere(file, &member_id) {
            return Ok(Refused(No::IdentityTaken));
        }
        if let Some(invited) = &invite.email {
            let matches = verified_emails
                .iter()
                .any(|email| email.trim().eq_ignore_ascii_case(invited));
            if !matches {
                return Ok(Refused(No::EmailMismatch {
                    invited: mask_email(invited),
                }));
            }
        }
        let waits = invite.email.is_some() && trust == EmailTrust::Untrusted;
        let member = find_active(file, &member_id)?;
        if waits {
            if let Some(invite) = member.invite.as_mut() {
                invite.pending = Some(identity.clone());
            }
            Ok(InviteOutcome::PendingApproval(member.clone()))
        } else {
            match member
                .identities
                .iter_mut()
                .find(|i| i.issuer == identity.issuer && i.subject == identity.subject)
            {
                Some(known) => known.email = identity.email.clone(),
                None => member.identities.push(identity.clone()),
            }
            member.invite = None;
            Ok(InviteOutcome::Linked(member.clone()))
        }
    })
}

/// The owner approves the account waiting on a member's invite: it is
/// linked and the invite is gone. Refused ([`WenlanError::Conflict`]) when
/// another active member has that account by now.
pub fn approve_pending(path: &Path, member_id: &str) -> Result<Member, WenlanError> {
    change_team(path, |file| {
        let pending = find_active(file, member_id)?
            .invite
            .as_ref()
            .and_then(|invite| invite.pending.clone())
            .ok_or_else(|| WenlanError::Validation("nobody is waiting for approval".into()))?;
        let taken = file.members.iter().any(|other| {
            other.id != member_id
                && other.is_active()
                && has_identity(other, &pending.issuer, &pending.subject)
        });
        if taken {
            return Err(WenlanError::Conflict(
                "that account is linked to another member".into(),
            ));
        }
        let member = find_active(file, member_id)?;
        if !has_identity(member, &pending.issuer, &pending.subject) {
            member.identities.push(Identity {
                linked_at: now(),
                ..pending
            });
        }
        member.invite = None;
        Ok(member.clone())
    })
}

/// The active member whose legacy key or non-revoked tool key this is, and
/// which of the two it was.
pub fn authenticate(path: &Path, key: &str) -> Result<Option<Authenticated>, WenlanError> {
    let key = key.trim();
    if !wenlan_types::team::is_team_key(key) {
        return Ok(None);
    }
    let hash = key_hash(key);
    let file = read_team(path)?;
    // Checked against every key of every member in constant time, so the
    // time taken does not say how much of a hash matched or which member or
    // key it was.
    let mut found = None;
    for member in file.members {
        let mut credential = None;
        if let Some(own) = &member.key_sha256 {
            if same_hash(own, &hash) {
                credential = Some(Credential::LegacyKey);
            }
        }
        for tool_key in &member.keys {
            if same_hash(&tool_key.sha256, &hash) && tool_key.revoked_at.is_none() {
                credential = Some(Credential::ToolKey {
                    id: tool_key.id.clone(),
                });
            }
        }
        if let Some(credential) = credential {
            if member.is_active() {
                found = Some(Authenticated { member, credential });
            }
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

/// A hosted Space's id: its folder's name, made once when it was first
/// hosted and kept when the Space is renamed.
pub fn hosted_space_id(folder: &Path) -> Option<String> {
    folder
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| version::is_safe_id(name))
        .map(str::to_string)
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
