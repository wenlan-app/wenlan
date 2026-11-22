// SPDX-License-Identifier: Apache-2.0
//! A host-owned library outside the personal DB and its sync/enrichment paths.
//! One atomic manifest is the record, history, receipt and tombstone store.
//! Every mutation serializes on the same file lock as team grant changes.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wenlan_types::team::{TeamAccess, TeamInfo, TeamRole};
use wenlan_types::team_library::{
    TeamGroup, TeamItem, TeamItemKind, TeamLibraryInfo, TeamPages, TeamPublicationReceipt,
};

use crate::error::WenlanError;
use crate::shared_space::version;
use crate::team::{self, Member, TeamFile};

pub const MEMORIES_ENV: &str = "WENLAN_TEAM_MEMORIES";

pub fn memories_enabled() -> bool {
    std::env::var(MEMORIES_ENV).is_ok_and(|value| value.trim() == "1")
}

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("Team Write is required")]
    Denied,
    #[error("Team item not found")]
    Absent,
    #[error("{0}")]
    Conflict(&'static str),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("Team memory publication is disabled on this host")]
    MemoriesDisabled,
    #[error(transparent)]
    Storage(#[from] WenlanError),
}

impl From<io::Error> for LibraryError {
    fn from(error: io::Error) -> Self {
        Self::Storage(WenlanError::Io(error))
    }
}

type Result<T> = std::result::Result<T, LibraryError>;

#[derive(Clone)]
pub struct Library {
    team_path: PathBuf,
    memories: bool,
}

#[derive(Deserialize, Serialize)]
struct Manifest {
    format: u32,
    groups: BTreeMap<String, TeamGroup>,
    migrated: BTreeSet<String>,
    records: BTreeMap<String, Vec<Record>>,
    retired: BTreeMap<String, Tombstone>,
    receipts: BTreeMap<String, Receipt>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            format: 1,
            groups: BTreeMap::new(),
            migrated: BTreeSet::new(),
            records: BTreeMap::new(),
            retired: BTreeMap::new(),
            receipts: BTreeMap::new(),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Record {
    item: TeamItem,
    brief: bool,
    private_source: bool,
    citations: Vec<CitationRef>,
    #[serde(default)]
    migrated_page: bool,
    legacy_file: Option<LegacyFile>,
}

#[derive(Clone, Deserialize, Serialize)]
struct CitationRef {
    id: String,
    revision: i64,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct LegacyFile {
    pub path: String,
    pub bytes: String,
}

pub struct LegacyHead {
    pub item: TeamItem,
    pub private_source: bool,
    pub file: Option<LegacyFile>,
}

#[derive(Deserialize, Serialize)]
struct Receipt {
    actor: String,
    receipt: TeamPublicationReceipt,
}

#[derive(Deserialize, Serialize)]
struct Tombstone {
    space_id: Option<String>,
    last_revision: i64,
    was_legacy_page: bool,
    publisher_id: Option<String>,
}

struct Data {
    file: TeamFile,
    info: TeamInfo,
    path: PathBuf,
    manifest: Manifest,
}

impl Data {
    fn member(&self, id: &str) -> Result<&Member> {
        self.file
            .members
            .iter()
            .find(|member| member.id == id && member.is_active())
            .ok_or(LibraryError::Denied)
    }

    fn heads(&self) -> impl Iterator<Item = &Record> {
        self.manifest
            .records
            .iter()
            .filter(|(id, _)| !self.manifest.retired.contains_key(*id))
            .filter_map(|(_, revisions)| revisions.last())
    }

    fn revisions(&self, id: &str) -> Option<&[Record]> {
        if self.manifest.retired.contains_key(id) {
            return None;
        }
        self.manifest.records.get(id).map(Vec::as_slice)
    }

    fn group(&self, space_id: Option<&str>) -> Result<Option<&TeamGroup>> {
        space_id
            .map(|id| self.manifest.groups.get(id).ok_or(LibraryError::Absent))
            .transpose()
    }

    fn access(&self, member: &Member, space_id: Option<&str>) -> Option<TeamAccess> {
        match self.group(space_id).ok()? {
            Some(group) if group.restricted => member.access(&group.name),
            Some(group) if member.space_only => member.spaces.get(&group.name).copied(),
            _ => member.library_access(),
        }
    }

    fn save(&self) -> Result<()> {
        let dir = self
            .path
            .parent()
            .ok_or(LibraryError::Invalid("Invalid team library path"))?;
        std::fs::create_dir_all(dir)?;
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(file.as_file_mut(), &self.manifest).map_err(WenlanError::from)?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(&self.path).map_err(|error| error.error)?;
        crate::store::export::history::sync_parent(&self.path)?;
        Ok(())
    }
}

fn fresh_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

impl Library {
    pub fn new(team_path: PathBuf, memories: bool) -> Self {
        Self {
            team_path,
            memories,
        }
    }

    fn open(&self, file: TeamFile) -> Result<Data> {
        let info = file
            .info()
            .filter(|info| version::is_safe_id(&info.id))
            .ok_or(LibraryError::Absent)?;
        let parent = self.team_path.parent().ok_or(LibraryError::Absent)?;
        let path = parent.join("teams").join(&info.id).join("library.json");
        let manifest: Manifest = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(WenlanError::from)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Manifest::default(),
            Err(e) => return Err(e.into()),
        };
        if manifest.format != 1 {
            return Err(LibraryError::Invalid("Unsupported team library format."));
        }
        Ok(Data {
            file,
            info,
            path,
            manifest,
        })
    }

    fn read(&self) -> Result<Data> {
        // Both on-disk documents are atomically replaced. The shared grant
        // lock also establishes a single point at which this read was allowed.
        let _lock = team::lock_team(&self.team_path)?;
        self.open(team::read_team(&self.team_path)?)
    }

    pub fn owner(&self) -> Result<String> {
        let file = team::read_team(&self.team_path)?;
        file.members
            .iter()
            .find(|member| member.is_active() && member.role == TeamRole::Owner)
            .map(|member| member.id.clone())
            .ok_or(LibraryError::Denied)
    }

    pub fn reader(&self, actor: &str, space_id: Option<&str>) -> Result<Reader> {
        let data = self.read()?;
        data.member(actor)?;
        let reader = Reader {
            data,
            actor: actor.into(),
            pin: space_id.map(str::to_owned),
            memories: self.memories,
        };
        if let Some(pin) = &reader.pin {
            if !reader.can_read(Some(pin)) {
                return Err(LibraryError::Absent);
            }
        }
        Ok(reader)
    }

    pub fn create_group(&self, actor: &str, name: &str, restricted: bool) -> Result<TeamGroup> {
        let name = name.trim();
        if name.is_empty() || name.len() > 128 || name.contains(['\r', '\n', '\0', '/']) {
            return Err(LibraryError::Invalid("Invalid Space name."));
        }
        let _lock = team::lock_team(&self.team_path)?;
        let mut data = self.open(team::read_team(&self.team_path)?)?;
        if data.member(actor)?.role != TeamRole::Owner {
            return Err(LibraryError::Denied);
        }
        if data
            .manifest
            .groups
            .values()
            .any(|group| group.name.eq_ignore_ascii_case(name))
            || data.file.everyone.contains_key(name)
            || data
                .file
                .members
                .iter()
                .any(|member| member.spaces.contains_key(name))
        {
            return Err(LibraryError::Conflict(
                "A Space or grant with this name already exists.",
            ));
        }
        let group = TeamGroup {
            id: fresh_id(),
            name: name.into(),
            restricted,
        };
        data.manifest.groups.insert(group.id.clone(), group.clone());
        data.save()?;
        Ok(group)
    }
}

pub struct Reader {
    data: Data,
    actor: String,
    pin: Option<String>,
    memories: bool,
}

impl Reader {
    fn can_read(&self, space_id: Option<&str>) -> bool {
        (self.pin.is_none() || self.pin.as_deref() == space_id)
            && self
                .data
                .member(&self.actor)
                .is_ok_and(|member| self.data.access(member, space_id).is_some())
    }

    fn can_read_item(&self, item: &TeamItem, space_id: Option<&str>) -> bool {
        (space_id.is_none() || space_id == item.space_id.as_deref())
            && self.can_read(item.space_id.as_deref())
            && (item.kind != TeamItemKind::Memory || self.memories)
    }

    pub fn info(&self) -> TeamLibraryInfo {
        TeamLibraryInfo {
            team: self.data.info.clone(),
            spaces: self
                .data
                .manifest
                .groups
                .values()
                .filter(|group| self.can_read(Some(&group.id)))
                .cloned()
                .collect(),
            team_memories: self.memories,
        }
    }

    pub fn space(&self, name: &str) -> Option<String> {
        self.data
            .manifest
            .groups
            .values()
            .find(|group| group.name == name && self.can_read(Some(&group.id)))
            .map(|group| group.id.clone())
    }

    pub fn migrated_space(&self, id: &str) -> Option<&str> {
        if self.can_read(Some(id)) && self.data.manifest.migrated.contains(id) {
            return self
                .data
                .manifest
                .groups
                .get(id)
                .map(|group| group.name.as_str());
        }
        None
    }

    fn page_record(&self, id: &str) -> Result<&Record> {
        self.data
            .revisions(id)
            .and_then(|revisions| revisions.last())
            .filter(|record| {
                record.item.kind == TeamItemKind::Page && self.can_read_item(&record.item, None)
            })
            .ok_or(LibraryError::Absent)
    }

    pub fn page(&self, id: &str) -> Result<TeamItem> {
        Ok(self.page_record(id)?.item.clone())
    }

    pub fn pages(&self, space: Option<&str>, limit: usize, offset: usize) -> TeamPages {
        let pages = self
            .data
            .heads()
            .filter(|record| {
                record.item.kind == TeamItemKind::Page && self.can_read_item(&record.item, space)
            })
            .skip(offset)
            .take(limit.min(200))
            .map(|record| record.item.clone())
            .collect();
        TeamPages { pages }
    }
}
