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
//! A sync pass keeps no state of its own, so a rebuilt database cannot apply
//! a version twice. For each page of the Space here and in the folder:
//!
//! 1. Find the folder's head of the page.
//! 2. Export: when this page's text is not the text of any version in the
//!    folder, write it as a new version over the version this page last
//!    held here (the head when it held none). An edit made over an older
//!    version than the head is a second leaf: an edit made at the same time.
//! 3. Apply: when this page's text is an older (or losing) version and the
//!    head's text is different, or the page is not here yet, write the head
//!    here through the normal page write, as `shared_person:<member>` or
//!    `shared_ai:<member>` (who wrote it is kept on each history row).
//!
//! An edit made here is in no version yet, so step 2 writes it over the
//! head, and it is the head next pass.
//!
//! This Wenlan's member id, name and folder for each shared Space are kept in
//! `shared-spaces.json` under the data root, outside the database, so a
//! rebuilt database stays the same member.

pub mod rewrite;
pub mod version;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wenlan_types::shared_space::SharedSyncCounts;

use crate::db::{MemoryDB, UNFILED_SPACE_ID};
use crate::error::WenlanError;
use crate::pages::Page;
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

/// One sync pass of `target.space` with its folder (see the module docs).
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
    let folder = target.folder.to_path_buf();
    let settings = join_at(target.settings_path, space, &folder, member_name)?;
    let me = SpaceMember {
        id: settings.member_id.clone(),
        name: settings.member_name.clone(),
    };
    {
        let folder = folder.clone();
        let space = space.to_string();
        tokio::task::spawn_blocking(move || ensure_space_file(&folder, &space, &me))
            .await
            .map_err(|error| WenlanError::Generic(format!("shared folder task: {error}")))??;
    }
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

    let local: HashMap<String, Page> = db
        .list_pages_by_space("active", Some(space), 1_000_000, 0)
        .await?
        .into_iter()
        .filter(|page| page.space.as_deref() == Some(space))
        .map(|page| (page.id.clone(), page))
        .collect();
    let space_pages: HashMap<String, String> = local
        .values()
        .map(|page| (page.id.clone(), page.title.clone()))
        .collect();

    let ids: BTreeSet<&str> = local
        .keys()
        .map(String::as_str)
        .chain(read.pages.keys().map(String::as_str))
        .collect();
    let empty = Vec::new();
    for id in ids {
        let versions = read.pages.get(id).unwrap_or(&empty);
        let head = version::head(versions);
        match local.get(id) {
            Some(page) => {
                let digest = version::body_digest(&page.content);
                let in_folder = versions.iter().any(|v| v.meta.body_digest == digest);
                if !in_folder {
                    let exported = export_page(
                        db,
                        page,
                        versions,
                        head.as_ref(),
                        &space_pages,
                        &settings,
                        &folder,
                    )
                    .await?;
                    if exported {
                        counts.exported += 1;
                    } else {
                        counts.skipped += 1;
                    }
                    continue;
                }
                let Some(head) = head else { continue };
                if head.winner.meta.body_digest == digest {
                    continue;
                }
                // This page's text is an earlier version, or an edit made at
                // the same time as the head that lost to it: take the head.
                // The text it had stays in this page's history.
                let lost = head
                    .leaves
                    .iter()
                    .any(|leaf| leaf.sha != head.winner.sha && leaf.meta.body_digest == digest);
                if apply_update(db, page, head.winner, target.knowledge_path).await? {
                    counts.applied += 1;
                    if lost {
                        counts.conflicts += 1;
                    }
                } else {
                    counts.skipped += 1;
                }
            }
            None => {
                let Some(head) = head else { continue };
                // A page with this id that is here but not in this Space (or
                // archived) is never written over, and never shared.
                if db.get_page(id).await?.is_some() {
                    log::warn!(
                        "[shared_space] {id} is in the folder but here it is not an active page of {space}; left as it is"
                    );
                    counts.skipped += 1;
                    continue;
                }
                if apply_create(db, space, head.winner, target.knowledge_path).await? {
                    counts.applied += 1;
                } else {
                    counts.skipped += 1;
                }
            }
        }
    }
    Ok(counts)
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

/// The version an edit made here was written over: the newest version this
/// page held here (an export or an apply), found by its text in the page's
/// history. An edit made over an older version than the head is then a
/// second leaf, an edit made at the same time, not a silent overwrite. With
/// no such version (the page was never shared from here) it is the head.
/// The other leaves this page held here are replaced by the new version.
fn base_of<'a>(
    history: &[String],
    versions: &'a [PageVersion],
    head: Option<&version::Head<'a>>,
) -> (Option<&'a PageVersion>, Vec<String>) {
    let newest = |digest: &String| {
        versions
            .iter()
            .filter(|version| &version.meta.body_digest == digest)
            .max_by(|a, b| (&a.meta.ts, &a.sha).cmp(&(&b.meta.ts, &b.sha)))
    };
    let parent = history
        .iter()
        .find_map(newest)
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

/// Writes `page` as a new version (see [`base_of`] for what it is written
/// over). `false` when it could not be written as a whole version (an id
/// that is no folder name, a body the page file cannot carry unchanged).
async fn export_page(
    db: &MemoryDB,
    page: &Page,
    versions: &[PageVersion],
    head: Option<&version::Head<'_>>,
    space_pages: &HashMap<String, String>,
    settings: &ShareSettings,
    folder: &Path,
) -> Result<bool, WenlanError> {
    if !version::is_safe_id(&page.id) {
        log::warn!(
            "[shared_space] page id {} cannot name a folder; not shared",
            page.id
        );
        return Ok(false);
    }
    let history: Vec<String> = db
        .list_page_history(&page.id, HISTORY_SCAN)
        .await?
        .iter()
        .map(|row| version::body_digest(&row.content))
        .collect();
    let (parent, merged) = base_of(&history, versions, head);
    let (citations, basis) = local_sources(db, page).await?;
    let memory_ids: Vec<String> = citations
        .iter()
        .filter_map(|citation| match &citation.locator {
            crate::store::format::page::Locator::Wenlan { id, .. } => Some(id.clone()),
            crate::store::format::page::Locator::Other(text) => {
                text.strip_prefix("wenlan:").map(str::to_string)
            }
        })
        .filter(|id| !space_pages.contains_key(id))
        .collect();
    let cited_text = db.export_memory_texts(&memory_ids).await?;
    let ctx = rewrite::RewriteContext {
        space_pages,
        cited_text: &cited_text,
        member_name: &settings.member_name,
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
        author: local_author(db, page, &settings.member_id).await?,
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    };
    let text = version::render(&file, &meta);
    let name = format!("{}.md", version::sha256_hex(text.as_bytes()));
    match version::parse(&name, text.as_bytes()) {
        Ok(read) if read.page.body == page.content => {}
        _ => {
            log::warn!(
                "[shared_space] {} does not read back as written; not shared",
                page.id
            );
            return Ok(false);
        }
    }
    let folder = folder.to_path_buf();
    let member = settings.member_id.clone();
    let page_id = page.id.clone();
    tokio::task::spawn_blocking(move || version::write_version(&folder, &member, &page_id, &text))
        .await
        .map_err(|error| WenlanError::Generic(format!("shared folder task: {error}")))??;
    Ok(true)
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

async fn apply_create(
    db: &MemoryDB,
    space: &str,
    head: &PageVersion,
    knowledge_path: Option<&Path>,
) -> Result<bool, WenlanError> {
    let Some(id) = head.page.id.as_deref() else {
        return Ok(false);
    };
    let agent = shared_edited_by(&head.meta.author);
    let distillation = crate::tuning::DistillationConfig::default();
    let write = crate::post_write::PageWrite::Create {
        page_id: Some(id),
        req: wenlan_types::requests::CreateConceptRequest {
            title: head.page.title.clone(),
            content: head.page.body.clone(),
            summary: None,
            entity_id: None,
            space: Some(space.to_string()).into(),
            source_memory_ids: Vec::new(),
            creation_kind: Some("authored".to_string()),
            workspace: None,
        },
        agent: &agent,
        knowledge_path,
        page_min_cluster_size: distillation.page_min_cluster_size,
        page_match_threshold: distillation.page_match_threshold,
        citations_json: None,
        basis_marks: Vec::new(),
        basis_space_guard: None,
        skip_protected_title_match: false,
        research_write: None,
    };
    match crate::post_write::page_write(db, write).await {
        Ok(result) => Ok(result.wrote),
        Err(error) => skipped_write(id, error),
    }
}

#[cfg(test)]
#[path = "shared_space_test.rs"]
mod tests;

#[cfg(test)]
#[path = "sync_test.rs"]
mod sync_tests;
