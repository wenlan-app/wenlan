// SPDX-License-Identifier: Apache-2.0
//! A host-owned library outside the personal DB and its sync/enrichment paths.
//! One atomic manifest is the record, history, receipt and tombstone store.
//! Every mutation serializes on the same file lock as team grant changes.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wenlan_types::team::{TeamAccess, TeamInfo, TeamPerson, TeamRole};
use wenlan_types::team_library::{
    TeamAudience, TeamAuthorship, TeamBrief, TeamCitation, TeamGroup, TeamItem, TeamItemKind,
    TeamLibraryInfo, TeamPageHistory, TeamPageSources, TeamPages, TeamPublication,
    TeamPublicationPreview, TeamPublicationReceipt, TeamRecall, TeamText,
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
    #[serde(default)]
    legacy_pages: BTreeMap<String, BTreeSet<String>>,
    records: BTreeMap<String, Vec<Record>>,
    retired: BTreeMap<String, Tombstone>,
    receipts: BTreeMap<String, Receipt>,
    /// Publication timestamps are wire metadata, kept outside persisted
    /// TeamItem records so older readers with deny_unknown_fields remain able
    /// to open this manifest.
    #[serde(default)]
    published_at: BTreeMap<String, String>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            format: 1,
            groups: BTreeMap::new(),
            migrated: BTreeSet::new(),
            legacy_pages: BTreeMap::new(),
            records: BTreeMap::new(),
            retired: BTreeMap::new(),
            receipts: BTreeMap::new(),
            published_at: BTreeMap::new(),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Record {
    #[serde(with = "persisted_team_item")]
    item: TeamItem,
    brief: bool,
    private_source: bool,
    citations: Vec<CitationRef>,
    #[serde(default)]
    migrated_page: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    legacy_origin: Option<team::LegacyHeadProof>,
    legacy_file: Option<LegacyFile>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedTeamItem {
    team_id: String,
    item_id: String,
    kind: TeamItemKind,
    revision: i64,
    space_id: Option<String>,
    title: String,
    body: String,
    contributor: Option<TeamPerson>,
    authorship: Option<TeamAuthorship>,
    publisher: Option<TeamPerson>,
    last_editor: Option<TeamPerson>,
}

impl From<&TeamItem> for PersistedTeamItem {
    fn from(item: &TeamItem) -> Self {
        Self {
            team_id: item.team_id.clone(),
            item_id: item.item_id.clone(),
            kind: item.kind,
            revision: item.revision,
            space_id: item.space_id.clone(),
            title: item.title.clone(),
            body: item.body.clone(),
            contributor: item.contributor.clone(),
            authorship: item.authorship,
            publisher: item.publisher.clone(),
            last_editor: item.last_editor.clone(),
        }
    }
}

impl From<PersistedTeamItem> for TeamItem {
    fn from(item: PersistedTeamItem) -> Self {
        Self {
            team_id: item.team_id,
            item_id: item.item_id,
            kind: item.kind,
            revision: item.revision,
            space_id: item.space_id,
            title: item.title,
            body: item.body,
            contributor: item.contributor,
            authorship: item.authorship,
            publisher: item.publisher,
            last_editor: item.last_editor,
            published_at: None,
        }
    }
}

mod persisted_team_item {
    use super::{PersistedTeamItem, TeamItem};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(item: &TeamItem, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        PersistedTeamItem::from(item).serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<TeamItem, D::Error>
    where
        D: Deserializer<'de>,
    {
        PersistedTeamItem::deserialize(deserializer).map(Into::into)
    }
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

pub struct LegacySpace {
    pub group: TeamGroup,
    pub folder: PathBuf,
    pub authorized: team::SharedFolderPages,
    pub heads: Vec<LegacyHead>,
    pub possible_copies: BTreeSet<String>,
}

impl LegacySpace {
    fn can_serve(&self, id: &str, file: &LegacyFile) -> bool {
        let Some(name) = file.path.rsplit('/').next() else {
            return false;
        };
        team::served_path(&self.folder, &file.path)
            .and_then(|path| std::fs::read(path).ok())
            .is_some_and(|bytes| {
                bytes == file.bytes.as_bytes()
                    && team::can_serve_version(id, name, &bytes, &self.authorized)
            })
    }
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

    fn retire(&mut self, id: &str) {
        let Some(record) = self
            .manifest
            .records
            .remove(id)
            .and_then(|mut revisions| revisions.pop())
        else {
            return;
        };
        self.manifest.retired.insert(
            id.to_owned(),
            Tombstone {
                space_id: record.item.space_id,
                last_revision: record.item.revision,
                was_legacy_page: record.migrated_page,
                publisher_id: record.item.publisher.map(|person| person.id),
            },
        );
        // The retired item cannot survive in history or as a source.
        for record in self.manifest.records.values_mut().flatten() {
            let before = record.citations.len();
            record.citations.retain(|citation| citation.id != id);
            record.private_source |= before != record.citations.len();
        }
    }

    fn import_legacy_space(&mut self, space: &mut LegacySpace) -> Result<bool> {
        let group = &space.group;
        let imported = self.manifest.migrated.contains(&group.id);
        if !imported && self.manifest.groups.contains_key(&group.id) {
            return Err(LibraryError::Conflict(
                "The hosted Space's ID is already in use.",
            ));
        }
        let changed = self.manifest.groups.get(&group.id) != Some(group);
        if changed {
            if self
                .manifest
                .groups
                .values()
                .any(|other| other.id != group.id && other.name == group.name)
            {
                return Err(LibraryError::Conflict(
                    "A team Space already uses the hosted Space's name.",
                ));
            }
            self.manifest.groups.insert(group.id.clone(), group.clone());
        }
        let need_inventory = !self.manifest.legacy_pages.contains_key(&group.id);
        if need_inventory {
            self.manifest
                .legacy_pages
                .insert(group.id.clone(), std::mem::take(&mut space.possible_copies));
        }
        if imported {
            return Ok(changed || need_inventory);
        }
        for head in std::mem::take(&mut space.heads) {
            let id = head.item.item_id.clone();
            if self.manifest.retired.contains_key(&id) {
                continue;
            }
            let Some(proof) = space.authorized.legacy_head(&id) else {
                continue;
            };
            if head.item.team_id != self.info.id
                || head.item.space_id.as_deref() != Some(&group.id)
                || head.item.kind != TeamItemKind::Page
                || head.item.revision != proof.revision
                || self.manifest.records.contains_key(&id)
            {
                return Err(LibraryError::Conflict(
                    "An imported page conflicts with an existing team item.",
                ));
            }
            self.manifest.records.insert(
                id,
                vec![Record {
                    item: head.item,
                    brief: false,
                    private_source: head.private_source,
                    citations: Vec::new(),
                    migrated_page: true,
                    legacy_origin: Some(proof.clone()),
                    legacy_file: head.file,
                }],
            );
        }
        self.manifest.migrated.insert(group.id.clone());
        Ok(true)
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

    fn save(&mut self) -> Result<()> {
        self.manifest.published_at = self
            .manifest
            .records
            .values()
            .flatten()
            .filter_map(|record| {
                record
                    .item
                    .published_at
                    .as_ref()
                    .map(|published_at| (record.item.item_id.clone(), published_at.clone()))
            })
            .collect();
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

    fn preview(
        &self,
        actor: &str,
        id: String,
        draft: TeamPublication,
        memories: bool,
    ) -> Result<TeamPublicationPreview> {
        let member = self.member(actor)?;
        let space = self.group(draft.space_id.as_deref())?;
        if self.access(member, draft.space_id.as_deref()) != Some(TeamAccess::Write) {
            return Err(LibraryError::Denied);
        }
        if !memories && (draft.kind == TeamItemKind::Memory || !draft.memories.is_empty()) {
            return Err(LibraryError::MemoriesDisabled);
        }
        draft.validate()?;
        if let Some(id) = &draft.item_id {
            let prior = self
                .revisions(id)
                .and_then(|revisions| revisions.last())
                .ok_or(LibraryError::Conflict(
                    "The item is unavailable. Start a new publication.",
                ))?;
            if prior.item.space_id != draft.space_id
                || self.access(member, prior.item.space_id.as_deref()) != Some(TeamAccess::Write)
            {
                return Err(LibraryError::Denied);
            }
            if Some(prior.item.revision) != draft.expected_revision || prior.item.kind != draft.kind
            {
                return Err(LibraryError::Conflict(
                    "The item changed. Review its current revision.",
                ));
            }
            if !draft.memories.is_empty() {
                return Err(LibraryError::Invalid(
                    "Publish a new page to approve new sources.",
                ));
            }
        }
        let now = self
            .file
            .members
            .iter()
            .filter(|member| member.is_active() && member.invite.is_none())
            .filter(|member| self.access(member, draft.space_id.as_deref()).is_some())
            .map(person)
            .collect();
        let later = match space {
            Some(group) if group.restricted => {
                if self.file.everyone.contains_key(&group.name) {
                    "Current and future team members with the Space grant, including members who inherit it. Space-only guests need an invitation for this Space."
                } else {
                    "Current and future team members given access to this restricted Space."
                }
            }
            Some(_) => {
                "Current and future members with team access. Space-only guests need an explicit grant to this Space."
            }
            _ => "Current and future members with team access. Space-only guests have no access.",
        };
        Ok(TeamPublicationPreview {
            id,
            team: self.info.clone(),
            space: space.cloned(),
            audience: TeamAudience {
                now,
                later: later.into(),
            },
            draft,
        })
    }
}

trait ValidatePublication {
    fn validate(&self) -> Result<()>;
}

impl ValidatePublication for TeamPublication {
    fn validate(&self) -> Result<()> {
        fn valid(title: &str, body: &str) -> bool {
            !title.trim().is_empty()
                && title.len() <= 512
                && !title.contains(['\r', '\n', '\0'])
                && !body.trim().is_empty()
                && body.len() <= 500_000
                && !body.contains('\0')
        }
        if !valid(&self.title, &self.body)
            || self.memories.len() > 32
            || self
                .memories
                .iter()
                .any(|source| !valid(&source.title, &source.body))
        {
            return Err(LibraryError::Invalid(
                "A title or body is empty, too long, or contains an invalid character.",
            ));
        }
        if self.kind == TeamItemKind::Memory
            && (!self.memories.is_empty() || self.private_source || self.brief)
        {
            return Err(LibraryError::Invalid(
                "Only a team page can be a Brief or carry sources.",
            ));
        }
        if self.item_id.is_some() != self.expected_revision.is_some() {
            return Err(LibraryError::Invalid(
                "An edit needs an item ID and expected revision.",
            ));
        }
        Ok(())
    }
}

fn person(member: &Member) -> TeamPerson {
    TeamPerson {
        id: member.id.clone(),
        name: member.name.clone(),
        owner: member.role == TeamRole::Owner,
    }
}

fn fresh_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// The host folder for one team's library records, beside `library.json`.
pub(crate) fn team_dir(team_path: &std::path::Path, team_id: &str) -> Option<PathBuf> {
    Some(team_path.parent()?.join("teams").join(team_id))
}

fn read_manifest(path: &std::path::Path) -> Result<Manifest> {
    let manifest: Manifest = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(WenlanError::from)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Manifest::default(),
        Err(e) => return Err(e.into()),
    };
    if manifest.format != 1 {
        return Err(LibraryError::Invalid("Unsupported team library format."));
    }
    Ok(manifest)
}

/// The library's Spaces, read without the team lock. The manifest is replaced
/// atomically, and callers that already hold the shared grant lock would
/// deadlock on `team::lock_team`.
pub(crate) fn groups_in(
    team_path: &std::path::Path,
    file: &TeamFile,
) -> std::result::Result<Vec<TeamGroup>, WenlanError> {
    let Some(info) = file.info().filter(|info| version::is_safe_id(&info.id)) else {
        return Ok(Vec::new());
    };
    let Some(dir) = team_dir(team_path, &info.id) else {
        return Ok(Vec::new());
    };
    match read_manifest(&dir.join("library.json")) {
        Ok(manifest) => Ok(manifest.groups.into_values().collect()),
        Err(LibraryError::Storage(error)) => Err(error),
        Err(error) => Err(WenlanError::Validation(error.to_string())),
    }
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
        let path = team_dir(&self.team_path, &info.id)
            .ok_or(LibraryError::Absent)?
            .join("library.json");
        let mut manifest = read_manifest(&path)?;
        let publication_times = manifest.published_at.clone();
        for record in manifest.records.values_mut().flatten() {
            record.item.published_at = publication_times.get(&record.item.item_id).cloned();
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

    pub fn preview(&self, actor: &str, draft: TeamPublication) -> Result<TeamPublicationPreview> {
        self.read()?
            .preview(actor, fresh_id(), draft, self.memories)
    }

    pub fn retry(&self, actor: &str, id: &str) -> Result<Option<TeamPublicationReceipt>> {
        let data = self.read()?;
        data.member(actor)?;
        Ok(data
            .manifest
            .receipts
            .get(id)
            .filter(|receipt| receipt.actor == actor)
            .map(|receipt| receipt.receipt.clone()))
    }

    pub fn publish(
        &self,
        actor: &str,
        approved: TeamPublicationPreview,
    ) -> Result<TeamPublicationReceipt> {
        let _lock = team::lock_team(&self.team_path)?;
        let mut data = self.open(team::read_team(&self.team_path)?)?;
        let member = data.member(actor)?;
        if let Some(earlier) = data.manifest.receipts.get(&approved.id) {
            return if earlier.actor == actor {
                Ok(earlier.receipt.clone())
            } else {
                Err(LibraryError::Absent)
            };
        }
        let review = data.preview(
            actor,
            approved.id.clone(),
            approved.draft.clone(),
            self.memories,
        )?;
        if review != approved {
            return Err(LibraryError::Conflict(
                "The destination or audience changed. Review a new preview.",
            ));
        }
        let actor_name = person(member);
        let draft = approved.draft;
        let space_id = draft.space_id;
        let item_id = draft.item_id.unwrap_or_else(fresh_id);
        let previous = data
            .revisions(&item_id)
            .and_then(|revisions| revisions.last());
        let revision = match previous {
            Some(record) => record
                .item
                .revision
                .checked_add(1)
                .ok_or(LibraryError::Conflict(
                    "The item has reached its revision limit.",
                ))?,
            None => 1,
        };
        let mut citations = Vec::new();
        let mut reviewed = Vec::new();
        for memory in draft.memories {
            let id = fresh_id();
            citations.push(CitationRef {
                id: id.clone(),
                revision: 1,
            });
            reviewed.push(Record {
                item: new_item(
                    &data.info,
                    &actor_name,
                    id,
                    space_id.clone(),
                    TeamItemKind::Memory,
                    memory,
                ),
                brief: false,
                private_source: false,
                citations: Vec::new(),
                migrated_page: false,
                legacy_origin: None,
                legacy_file: None,
            });
        }
        let mut record = Record {
            item: new_item(
                &data.info,
                &actor_name,
                item_id.clone(),
                space_id,
                draft.kind,
                TeamText {
                    title: draft.title,
                    body: draft.body,
                },
            ),
            brief: draft.brief,
            private_source: draft.private_source,
            citations,
            migrated_page: false,
            legacy_origin: None,
            legacy_file: None,
        };
        if let Some(authorship) = draft.authorship {
            record.item.authorship = Some(authorship);
        }
        if let Some(previous) = previous {
            record.item.revision = revision;
            record
                .item
                .contributor
                .clone_from(&previous.item.contributor);
            record.item.publisher.clone_from(&previous.item.publisher);
            record.citations.clone_from(&previous.citations);
            record
                .item
                .published_at
                .clone_from(&previous.item.published_at);
            record.private_source |= previous.private_source;
            record.migrated_page = previous.migrated_page;
            record.legacy_origin.clone_from(&previous.legacy_origin);
        }
        let receipt = TeamPublicationReceipt {
            team_id: data.info.id.clone(),
            item_id: item_id.clone(),
            revision,
            memory_ids: reviewed
                .iter()
                .map(|record| record.item.item_id.clone())
                .collect(),
        };
        // The Brief is a designated page, with at most one head per scope.
        if record.brief {
            let conflict = data.heads().any(|old| {
                old.brief
                    && old.item.item_id != item_id
                    && old.item.space_id == record.item.space_id
            });
            if conflict {
                return Err(LibraryError::Conflict(
                    "This destination has a Brief. Preview an edit to that page.",
                ));
            }
        }
        for memory in reviewed {
            data.manifest
                .records
                .insert(memory.item.item_id.clone(), vec![memory]);
        }
        data.manifest
            .records
            .entry(item_id)
            .or_default()
            .push(record);
        data.manifest.receipts.insert(
            approved.id,
            Receipt {
                actor: actor.into(),
                receipt: receipt.clone(),
            },
        );
        data.save()?;
        Ok(receipt)
    }

    pub fn withdraw(&self, actor: &str, id: &str) -> Result<()> {
        let _lock = team::lock_team(&self.team_path)?;
        let mut data = self.open(team::read_team(&self.team_path)?)?;
        let member = data.member(actor)?;
        let may_withdraw = |space, publisher| {
            member.role == TeamRole::Owner
                || (publisher == Some(actor)
                    && data.access(member, space) == Some(TeamAccess::Write))
        };
        if let Some(retired) = data.manifest.retired.get(id) {
            return if may_withdraw(retired.space_id.as_deref(), retired.publisher_id.as_deref()) {
                Ok(())
            } else {
                Err(LibraryError::Absent)
            };
        }
        let record = data
            .revisions(id)
            .and_then(|revisions| revisions.last())
            .ok_or(LibraryError::Absent)?;
        let publisher_id = record
            .item
            .publisher
            .as_ref()
            .map(|person| person.id.as_str());
        if !may_withdraw(record.item.space_id.as_deref(), publisher_id) {
            return Err(LibraryError::Absent);
        }
        data.retire(id);
        data.save()
    }

    pub fn create_group(&self, actor: &str, name: &str, restricted: bool) -> Result<TeamGroup> {
        let name = name.trim();
        if name.is_empty() || name.len() > 128 || name.contains(['\r', '\n', '\0', '/']) {
            return Err(LibraryError::Invalid("Invalid Space name."));
        }
        if name.eq_ignore_ascii_case(crate::rulesync_team::SPACE) {
            // The team-wide Rules page already uses this name.
            return Err(LibraryError::Invalid(
                "The name everyone is used by the team-wide Rules; choose another Space name.",
            ));
        }
        let _lock = team::lock_team(&self.team_path)?;
        let mut data = self.open(team::read_team(&self.team_path)?)?;
        if data.member(actor)?.role != TeamRole::Owner {
            return Err(LibraryError::Denied);
        }
        if let Some(group) = data
            .manifest
            .groups
            .values()
            .find(|group| group.name.eq_ignore_ascii_case(name))
        {
            return if group.restricted == restricted {
                Ok(group.clone())
            } else {
                Err(LibraryError::Conflict(
                    "A Space or grant with this name already exists.",
                ))
            };
        }
        if data.file.everyone.contains_key(name)
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

    pub fn needs_migration(&self, group: &TeamGroup) -> Result<bool> {
        Ok(!self.read()?.manifest.migrated.contains(&group.id))
    }

    pub fn reconcile_legacy_spaces(&self, mut spaces: Vec<LegacySpace>) -> Result<()> {
        let _lock = team::lock_team(&self.team_path)?;
        let mut data = self.open(team::read_team(&self.team_path)?)?;
        let mut changed = false;
        for space in &mut spaces {
            changed |= data.import_legacy_space(space)?;
        }
        let current: BTreeMap<_, _> = spaces
            .iter()
            .map(|space| (&space.group.id, space))
            .collect();
        let mut removed = Vec::new();
        for (id, revisions) in &mut data.manifest.records {
            let Some(record) = revisions.last_mut().filter(|record| record.migrated_page) else {
                continue;
            };
            let Some(space) = record.item.space_id.as_ref().and_then(|id| current.get(id)) else {
                removed.push(id.clone());
                continue;
            };
            // A new incarnation, a move, an edit or stale source evidence
            // invalidates the host snapshot we inherited. A reviewed edit
            // lives in the library and keeps that original host witness.
            let proof = record.legacy_origin.as_ref();
            if proof.is_none() || proof != space.authorized.legacy_head(id) {
                removed.push(id.clone());
                continue;
            }
            if record
                .legacy_file
                .as_ref()
                .is_some_and(|file| !space.can_serve(id, file))
            {
                record.legacy_file = None;
                changed = true;
            }
        }
        changed |= !removed.is_empty();
        for id in removed {
            data.retire(&id);
        }
        if changed {
            data.save()?;
        }
        Ok(())
    }
}

fn new_item(
    team: &TeamInfo,
    actor: &TeamPerson,
    id: String,
    space_id: Option<String>,
    kind: TeamItemKind,
    text: TeamText,
) -> TeamItem {
    TeamItem {
        team_id: team.id.clone(),
        item_id: id,
        kind,
        revision: 1,
        space_id,
        title: text.title,
        body: text.body,
        contributor: Some(actor.clone()),
        authorship: Some(TeamAuthorship::Human),
        publisher: Some(actor.clone()),
        last_editor: Some(actor.clone()),
        published_at: Some(chrono::Utc::now().to_rfc3339()),
    }
}

/// A personal page's citation metadata and quoted source blocks are not
/// part of the default review. A person can still paste exact prose.
pub fn draft_page_body(body: &str) -> String {
    let body = body
        .split(crate::store::format::page::SOURCES_START)
        .next()
        .unwrap_or_default();
    let mut text = String::new();
    let mut quote = String::new();
    let flush_quote = |text: &mut String, quote: &mut String| {
        if crate::citations::marker_spans(quote).is_empty() {
            text.push_str(quote);
        }
        quote.clear();
    };
    for line in body.split_inclusive('\n') {
        if line.trim_start().starts_with('>') {
            quote.push_str(line);
        } else {
            flush_quote(&mut text, &mut quote);
            text.push_str(line);
        }
    }
    flush_quote(&mut text, &mut quote);
    crate::citations::strip_member_citation_markers(&text)
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

    /// The team-only Spaces this member can use, with the access they can
    /// actually use: groups made in the library, not hosted Spaces it
    /// migrated (those reach members' computers as before). By name order.
    pub fn team_only_spaces(&self) -> Vec<wenlan_types::team::TeamSpaceAccess> {
        let Ok(member) = self.data.member(&self.actor) else {
            return Vec::new();
        };
        let mut spaces: Vec<_> = self
            .data
            .manifest
            .groups
            .values()
            .filter(|group| !self.data.manifest.migrated.contains(&group.id))
            .filter(|group| self.pin.is_none() || self.pin.as_deref() == Some(&group.id))
            .filter_map(|group| {
                let mut access = self.data.access(member, Some(&group.id))?;
                // Writing needs a person signed in with Google or GitHub
                // (every library write asks for a fresh browser session).
                // Someone who joined with only an invite link reads until
                // they link an account, so they are told Read.
                if member.role != TeamRole::Owner && member.identities.is_empty() {
                    access = TeamAccess::Read;
                }
                Some(wenlan_types::team::TeamSpaceAccess {
                    space: group.name.clone(),
                    access,
                    id: Some(group.id.clone()),
                })
            })
            .collect();
        spaces.sort_by(|a, b| a.space.cmp(&b.space));
        spaces
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

    pub fn memory(&self, id: &str) -> Result<TeamItem> {
        self.data
            .revisions(id)
            .and_then(|revisions| revisions.last())
            .filter(|record| {
                record.item.kind == TeamItemKind::Memory && self.can_read_item(&record.item, None)
            })
            .map(|record| record.item.clone())
            .ok_or(LibraryError::Absent)
    }

    pub fn history(&self, id: &str) -> Result<TeamPageHistory> {
        self.page_record(id)?;
        Ok(TeamPageHistory {
            revisions: self
                .data
                .revisions(id)
                .unwrap_or_default()
                .iter()
                .rev()
                .filter(|record| self.can_read_item(&record.item, None))
                .map(|record| record.item.clone())
                .collect(),
        })
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

    pub fn recall(&self, space: Option<&str>, topic: &str, limit: usize) -> TeamRecall {
        let lower = topic.to_lowercase();
        let words: Vec<_> = lower.split_whitespace().collect();
        if words.is_empty() {
            return TeamRecall::default();
        }
        let mut hits: Vec<_> = self
            .data
            .heads()
            .filter(|record| self.can_read_item(&record.item, space))
            .filter_map(|record| {
                let title = record.item.title.to_lowercase();
                let body = record.item.body.to_lowercase();
                let score: usize = words
                    .iter()
                    .map(|word| {
                        2 * usize::from(title.contains(word)) + usize::from(body.contains(word))
                    })
                    .sum();
                (score != 0).then_some((score, &record.item))
            })
            .collect();
        hits.sort_by(|(a, left), (b, right)| {
            b.cmp(a).then_with(|| left.item_id.cmp(&right.item_id))
        });
        let mut result = TeamRecall::default();
        for (_, item) in hits.into_iter().take(limit.min(100)) {
            match item.kind {
                TeamItemKind::Page => result.pages.push(item.clone()),
                TeamItemKind::Memory => result.memories.push(item.clone()),
            }
        }
        result
    }

    pub fn brief(&self, space: Option<&str>, topic: Option<&str>, limit: usize) -> TeamBrief {
        let group = space.or(self.pin.as_deref());
        let brief = self
            .data
            .heads()
            .find(|record| {
                record.brief
                    && record.item.space_id.as_deref() == group
                    && self.can_read_item(&record.item, group)
            })
            .map(|record| record.item.clone());
        TeamBrief {
            brief,
            related: topic
                .map(|topic| self.recall(space, topic, limit))
                .unwrap_or_default(),
        }
    }

    pub fn sources(&self, id: &str) -> Result<TeamPageSources> {
        let page = self.page_record(id)?;
        let mut hidden = page.private_source;
        let mut sources = Vec::new();
        for citation in &page.citations {
            let readable = self.data.revisions(&citation.id).and_then(|revisions| {
                revisions.iter().find(|record| {
                    record.item.kind == TeamItemKind::Memory
                        && record.item.revision == citation.revision
                        && self.can_read_item(&record.item, None)
                })
            });
            match readable {
                Some(record) => sources.push(TeamCitation::Memory {
                    memory: Box::new(record.item.clone()),
                }),
                None => hidden = true,
            }
        }
        if hidden {
            sources.push(TeamCitation::PrivateSource {
                label: "private source".into(),
            });
        }
        Ok(TeamPageSources {
            page_id: id.into(),
            sources,
        })
    }

    pub fn legacy_paths<'a>(&'a self, space_id: &'a str) -> impl Iterator<Item = &'a str> {
        self.data
            .heads()
            .filter(move |record| self.can_read_item(&record.item, Some(space_id)))
            .filter_map(|record| record.legacy_file.as_ref().map(|file| file.path.as_str()))
    }

    pub fn deleted_legacy_pages(&self, space_id: &str) -> Vec<String> {
        if !self.can_read(Some(space_id)) {
            return Vec::new();
        }
        let mut deleted = self
            .data
            .manifest
            .legacy_pages
            .get(space_id)
            .cloned()
            .unwrap_or_default();
        deleted.extend(
            self.data
                .manifest
                .retired
                .iter()
                .filter(|(_, entry)| {
                    entry.was_legacy_page && entry.space_id.as_deref() == Some(space_id)
                })
                .map(|(id, _)| id.clone()),
        );
        for record in self.data.heads().filter(|record| {
            record.migrated_page && record.item.space_id.as_deref() == Some(space_id)
        }) {
            if record.legacy_file.is_some() {
                deleted.remove(&record.item.item_id);
            } else {
                deleted.insert(record.item.item_id.clone());
            }
        }
        deleted.into_iter().collect()
    }

    pub fn legacy_file(&self, space_id: &str, path: &str) -> Option<String> {
        self.data
            .heads()
            .filter(|record| self.can_read_item(&record.item, Some(space_id)))
            .filter_map(|record| record.legacy_file.as_ref())
            .find(|file| file.path == path)
            .map(|file| file.bytes.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct BaseManifest {
        format: u32,
        groups: BTreeMap<String, serde_json::Value>,
        migrated: BTreeSet<String>,
        #[serde(default)]
        legacy_pages: BTreeMap<String, BTreeSet<String>>,
        records: BTreeMap<String, Vec<BaseRecord>>,
        retired: BTreeMap<String, serde_json::Value>,
        receipts: BTreeMap<String, serde_json::Value>,
    }

    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct BaseRecord {
        item: BaseTeamItem,
        brief: bool,
        private_source: bool,
        citations: Vec<BaseCitationRef>,
        #[serde(default)]
        migrated_page: bool,
        #[serde(default)]
        legacy_origin: Option<serde_json::Value>,
        legacy_file: Option<serde_json::Value>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct BaseTeamItem {
        team_id: String,
        item_id: String,
        kind: TeamItemKind,
        revision: i64,
        space_id: Option<String>,
        title: String,
        body: String,
        contributor: Option<TeamPerson>,
        authorship: Option<TeamAuthorship>,
        publisher: Option<TeamPerson>,
        last_editor: Option<TeamPerson>,
    }

    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct BaseCitationRef {
        id: String,
        revision: i64,
    }

    #[test]
    fn published_at_is_stored_outside_items_for_base_manifest_readers() {
        let item = TeamItem {
            team_id: "team".into(),
            item_id: "page".into(),
            kind: TeamItemKind::Page,
            revision: 1,
            space_id: None,
            title: "Launch".into(),
            body: "Friday".into(),
            contributor: None,
            authorship: Some(TeamAuthorship::Human),
            publisher: None,
            last_editor: None,
            published_at: Some("2026-11-26T12:00:00Z".into()),
        };
        let manifest = Manifest {
            format: 1,
            groups: BTreeMap::new(),
            migrated: BTreeSet::new(),
            legacy_pages: BTreeMap::new(),
            records: BTreeMap::from([(
                "page".into(),
                vec![Record {
                    item,
                    brief: false,
                    private_source: false,
                    citations: Vec::new(),
                    migrated_page: false,
                    legacy_origin: None,
                    legacy_file: None,
                }],
            )]),
            retired: BTreeMap::new(),
            receipts: BTreeMap::new(),
            published_at: BTreeMap::new(),
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.json");
        let mut data = Data {
            file: TeamFile::default(),
            info: TeamInfo {
                id: "team".into(),
                name: Some("Acme".into()),
            },
            path,
            manifest,
        };
        data.save().unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&data.path).unwrap()).unwrap();
        assert!(json["records"]["page"][0]["item"]
            .get("published_at")
            .is_none());
        assert_eq!(json["published_at"]["page"], "2026-11-26T12:00:00Z");
        let base: BaseManifest = serde_json::from_value(json).unwrap();
        assert_eq!(base.format, 1);
        assert!(base.groups.is_empty());
        assert!(base.migrated.is_empty());
        assert!(base.legacy_pages.is_empty());
        assert!(base.retired.is_empty());
        assert!(base.receipts.is_empty());
        assert_eq!(base.records["page"].len(), 1);
        assert_eq!(base.records["page"][0].item.item_id, "page");
    }

    #[test]
    fn create_group_reuses_the_same_library_space_but_rejects_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let team_path = dir.path().join(crate::team::TEAM_FILE);
        let owner = crate::team::ensure_owner(&team_path, "Owner").unwrap();
        let library = Library::new(team_path.clone(), false);

        let first = library.create_group(&owner.id, "Product", false).unwrap();
        let retry = library.create_group(&owner.id, " Product ", false).unwrap();
        assert_eq!(retry, first);
        assert!(matches!(
            library.create_group(&owner.id, "Product", true),
            Err(LibraryError::Conflict(_))
        ));

        crate::team::set_everyone(&team_path, "Legacy", Some(TeamAccess::Write)).unwrap();
        assert!(matches!(
            library.create_group(&owner.id, "Legacy", false),
            Err(LibraryError::Conflict(_))
        ));
    }
}
