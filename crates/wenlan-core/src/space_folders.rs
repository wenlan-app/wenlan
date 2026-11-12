// SPDX-License-Identifier: Apache-2.0
//! Each Space's pages in a folder of its own, inside the pages folder.
//!
//! ```text
//! <pages>/文瀾/定位.md             a page of the Space 文瀾
//! <pages>/文瀾/發布/清單.md        the person's own folder inside that Space
//! <pages>/Acme/產品/路線.md        a page of the team Acme's Space 產品
//! <pages>/沒分空間.md              a page in no Space stays at the top
//! ```
//!
//! The folder a page's file sits in and the page's Space say the same thing:
//!
//! - A page that Wenlan moved to another Space (or whose Space was renamed,
//!   or joined a team) has its file moved to the new Space's folder. The
//!   folders the person made inside the old Space come along.
//! - A file the person moved into another Space's folder (in Finder or
//!   Obsidian) moves its page to that Space; moved out of every Space's
//!   folder, the page is in no Space.
//!
//! The two are told apart by where Wenlan last put each page
//! ([`Record::placed`]): a file still under that folder was not moved by the
//! person, so a difference means the page's Space changed.
//!
//! This is off until the person agrees (or the pages folder starts empty):
//! the record file in `<pages>/.wenlan/` is both the switch and the record.
//! The first pass moves every existing page into its Space's folder,
//! keeping the folders the person made.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::db::{MemoryDB, UNFILED_SPACE_ID};
use crate::error::WenlanError;

/// The record, in the pages folder's own `.wenlan/` folder.
pub const RECORD_FILE: &str = "space-folders.json";

/// Longest folder name made from a Space or team name, in bytes.
const NAME_MAX_BYTES: usize = 120;

/// Folders of the pages folder that are never a Space's: Wenlan's own.
const RESERVED: &[&str] = &["archive", "_sources", "_attachments"];

/// The switch and the record of where each page was last put.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub format: u32,
    /// Page id -> the Space folder (`""` = the top) Wenlan last put it in.
    #[serde(default)]
    pub placed: BTreeMap<String, String>,
}

fn record_path(root: &Path) -> PathBuf {
    root.join(".wenlan").join(RECORD_FILE)
}

/// Whether pages are kept in their Spaces' folders here.
pub fn is_on(root: &Path) -> bool {
    record_path(root).is_file()
}

pub fn read_record(root: &Path) -> Result<Option<Record>, WenlanError> {
    match std::fs::read(record_path(root)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn save_record(root: &Path, record: &Record) -> Result<(), WenlanError> {
    let path = record_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(record)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Turns it on: the next pass moves every page into its Space's folder.
pub fn turn_on(root: &Path) -> Result<(), WenlanError> {
    if is_on(root) {
        return Ok(());
    }
    save_record(
        root,
        &Record {
            format: 1,
            placed: BTreeMap::new(),
        },
    )
}

/// Turns it off: pages stay where they are, nothing more is moved.
pub fn turn_off(root: &Path) -> Result<(), WenlanError> {
    match std::fs::remove_file(record_path(root)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

/// A folder name for a Space or team name: characters no file system
/// takes become `-`, and it never starts with a dot or names one of
/// Wenlan's own folders.
pub fn folder_name(name: &str) -> String {
    let mut out: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*') {
                '-'
            } else {
                c
            }
        })
        .collect();
    while out.len() > NAME_MAX_BYTES {
        out.pop();
    }
    let out = out.trim_end_matches(['.', ' ']).trim_start_matches('.');
    let mut out = out.to_string();
    let upper = out
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let windows_reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && upper.as_bytes()[3].is_ascii_digit());
    if out.is_empty() || windows_reserved || RESERVED.iter().any(|r| out.eq_ignore_ascii_case(r)) {
        out = format!("{out} (Space)").trim().to_string();
    }
    out
}

/// The folder a Space's pages are kept in: `<Space>` or `<team>/<Space>`;
/// `""` (the top) for no Space.
pub fn space_folder(space: Option<&str>, team: Option<&str>) -> String {
    match space {
        None => String::new(),
        Some(space) => match team {
            Some(team) => format!("{}/{}", folder_name(team), folder_name(space)),
            None => folder_name(space),
        },
    }
}

/// Every Space's folder, by Space name, with the team folders kept apart
/// from personal Space folders of the same name.
pub fn layout(spaces: &[String], teams: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let personal: BTreeSet<String> = spaces
        .iter()
        .filter(|space| !teams.contains_key(*space))
        .map(|space| folder_name(space).to_lowercase())
        .collect();
    spaces
        .iter()
        .map(|space| {
            let team = teams.get(space).map(|team| {
                if personal.contains(&folder_name(team).to_lowercase()) {
                    format!("{team} (team)")
                } else {
                    team.clone()
                }
            });
            (space.clone(), space_folder(Some(space), team.as_deref()))
        })
        .collect()
}

/// One page as a pass sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub id: String,
    /// Its file, relative to the pages folder.
    pub file: String,
    /// Its Space now, `None` for none.
    pub space: Option<String>,
}

/// What a pass does to one page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Move the file into `folder` (created first when missing).
    Move {
        id: String,
        from: String,
        folder: String,
    },
    /// The person moved the file: put the page in this Space (`None` = none).
    SetSpace { id: String, space: Option<String> },
    /// Already where it belongs; remember it there.
    Keep { id: String, folder: String },
}

fn dir_of(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn under<'a>(dir: &'a str, folder: &str) -> Option<&'a str> {
    if folder.is_empty() {
        return Some(dir);
    }
    if dir.eq_ignore_ascii_case(folder) {
        return Some("");
    }
    let lower = dir.to_ascii_lowercase();
    let prefix = format!("{}/", folder.to_ascii_lowercase());
    lower
        .starts_with(&prefix)
        .then(|| &dir[dir.len() - (lower.len() - prefix.len())..])
}

/// The Space whose folder holds `dir` (the deepest one), with the rest of
/// the path inside it.
fn owner<'a>(dir: &'a str, layout: &BTreeMap<String, String>) -> Option<(String, &'a str)> {
    layout
        .iter()
        .filter_map(|(space, folder)| under(dir, folder).map(|rest| (space, folder.len(), rest)))
        .max_by_key(|(_, len, _)| *len)
        .map(|(space, _, rest)| (space.clone(), rest))
}

fn join(folder: &str, rest: &str) -> String {
    match (folder.is_empty(), rest.is_empty()) {
        (true, _) => rest.to_string(),
        (_, true) => folder.to_string(),
        _ => format!("{folder}/{rest}"),
    }
}

/// What a pass does, page by page. `skip` folders (linked vaults, Wenlan's
/// own) are left alone.
pub fn plan(
    pages: &[Placement],
    layout: &BTreeMap<String, String>,
    record: &Record,
    skip: &[String],
) -> Vec<Step> {
    let mut steps = Vec::new();
    for page in pages {
        let dir = dir_of(&page.file);
        let first = dir.split('/').next().unwrap_or_default();
        if RESERVED.iter().any(|r| first.eq_ignore_ascii_case(r))
            || first.starts_with('.')
            || skip
                .iter()
                .any(|folder| under(dir, folder).is_some() && !folder.is_empty())
        {
            continue;
        }
        let want = page
            .space
            .as_ref()
            .and_then(|space| layout.get(space))
            .cloned()
            .unwrap_or_default();
        if page.space.is_some() && want.is_empty() {
            // A Space this pass does not know yet: leave the file alone.
            continue;
        }
        let step = match record.placed.get(&page.id) {
            // Still under the folder Wenlan put it in: the Space is what changed.
            Some(placed) if !placed.is_empty() && under(dir, placed).is_some() => {
                let rest = under(dir, placed).unwrap_or_default();
                if placed.eq_ignore_ascii_case(&want) {
                    Step::Keep {
                        id: page.id.clone(),
                        folder: want,
                    }
                } else {
                    Step::Move {
                        id: page.id.clone(),
                        from: page.file.clone(),
                        folder: join(&want, rest),
                    }
                }
            }
            Some(placed) if placed.is_empty() && owner(dir, layout).is_none() => {
                if want.is_empty() {
                    Step::Keep {
                        id: page.id.clone(),
                        folder: want,
                    }
                } else {
                    Step::Move {
                        id: page.id.clone(),
                        from: page.file.clone(),
                        folder: join(&want, dir),
                    }
                }
            }
            // The person moved it: the folder it is in says its Space.
            Some(_) => {
                let now = owner(dir, layout).map(|(space, _)| space);
                if now == page.space {
                    Step::Keep {
                        id: page.id.clone(),
                        folder: want,
                    }
                } else {
                    Step::SetSpace {
                        id: page.id.clone(),
                        space: now,
                    }
                }
            }
            // First pass: into its Space's folder, keeping the person's folders.
            None => match owner(dir, layout) {
                Some((space, _)) if Some(&space) == page.space.as_ref() => Step::Keep {
                    id: page.id.clone(),
                    folder: want,
                },
                _ if want.is_empty() => Step::Keep {
                    id: page.id.clone(),
                    folder: want,
                },
                _ => Step::Move {
                    id: page.id.clone(),
                    from: page.file.clone(),
                    folder: join(&want, dir),
                },
            },
        };
        steps.push(step);
    }
    steps
}

/// Makes `folder` and every folder above it, as the Wiki's New folder does.
fn make_folders(root: &Path, folder: &str) -> Result<(), WenlanError> {
    let mut parent = String::new();
    for name in folder.split('/').filter(|part| !part.is_empty()) {
        parent = crate::export::knowledge::create_knowledge_folder(root, &parent, name)?;
    }
    Ok(())
}

/// Removes the folders a move left empty, deepest first. A folder holding
/// anything at all (even a hidden file) stays.
fn remove_emptied(root: &Path, dir: &str) {
    let mut dir = dir.to_string();
    while !dir.is_empty() {
        if std::fs::remove_dir(root.join(&dir)).is_err() {
            return;
        }
        dir = dir_of(&dir).to_string();
    }
}

/// The team name each team Space here is synced with, by Space name.
pub fn team_spaces(settings_path: &Path) -> BTreeMap<String, String> {
    crate::shared_space::team_names_by_space(settings_path).unwrap_or_default()
}

/// Where a new page goes when nothing else says: its Space's folder, made
/// if missing. `None` while it is off, for no Space, or when the folder
/// cannot be made (the page then goes to the top, and a pass moves it).
/// A page that already has a file keeps it; a pass moves that one.
pub async fn new_page_folder(
    db: &MemoryDB,
    root: &Path,
    page: &crate::pages::Page,
) -> Option<String> {
    if !is_on(root) {
        return None;
    }
    let space = page
        .space
        .as_deref()
        .filter(|s| *s != UNFILED_SPACE_ID && *s != "uncategorized")?;
    let spaces: Vec<String> = db
        .list_spaces()
        .await
        .ok()?
        .into_iter()
        .map(|s| s.name)
        .collect();
    let folder = layout(&spaces, &team_spaces(&crate::shared_space::settings_path()))
        .get(space)?
        .clone();
    make_folders(root, &folder)
        .map_err(|error| log::warn!("[space-folders] could not make {folder}: {error}"))
        .ok()?;
    Some(folder)
}

/// Every active page with a file, its file and its Space.
async fn placements(db: &MemoryDB, root: &Path) -> Result<Vec<Placement>, WenlanError> {
    let files = {
        let root = root.to_path_buf();
        tokio::task::spawn_blocking(move || crate::export::knowledge::knowledge_edit_targets(&root))
            .await
            .map_err(|e| WenlanError::Io(std::io::Error::other(e.to_string())))??
    };
    let spaces = db.active_page_spaces().await?;
    Ok(files
        .into_iter()
        .filter_map(|(id, file)| {
            let space = spaces.get(&id)?.clone();
            Some(Placement {
                id,
                file,
                space: space.filter(|s| s != UNFILED_SPACE_ID && s != "uncategorized"),
            })
        })
        .collect())
}

/// What one pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PassStats {
    pub moved: usize,
    pub respaced: usize,
    pub failed: usize,
}

/// One pass: moves files whose Space changed and moves pages whose file the
/// person moved. Does nothing while it is off.
pub async fn arrange(
    db: &MemoryDB,
    root: &Path,
    settings_path: &Path,
    skip: &[String],
) -> Result<PassStats, WenlanError> {
    let Some(mut record) = read_record(root)? else {
        return Ok(PassStats::default());
    };
    let spaces: Vec<String> = db
        .list_spaces()
        .await?
        .into_iter()
        .map(|s| s.name)
        .collect();
    let layout = layout(&spaces, &team_spaces(settings_path));
    let pages = placements(db, root).await?;
    let steps = plan(&pages, &layout, &record, skip);
    let mut stats = PassStats::default();
    let mut moves = Vec::new();
    for step in steps {
        match step {
            Step::Keep { id, folder } => {
                record.placed.insert(id, folder);
            }
            Step::SetSpace { id, space } => {
                let Some(page) = db.get_page(&id).await? else {
                    continue;
                };
                match db
                    .set_page_space(
                        &id,
                        space.as_deref(),
                        page.version,
                        &crate::read_scope::ReadScope::Global,
                    )
                    .await
                {
                    Ok(_) => {
                        stats.respaced += 1;
                        // Its file's `space:` line says the new Space too.
                        if let Ok(Some(page)) = db.get_page(&id).await {
                            let projection =
                                crate::export::knowledge::KnowledgeProjectionWrite::new(
                                    root.to_path_buf(),
                                    db,
                                );
                            if let Err(error) = projection.write_page_gated(db, &page).await {
                                log::warn!("[space-folders] {id} moved Space but its file was not rewritten: {error}");
                            }
                        }
                        let folder = space
                            .as_ref()
                            .and_then(|space| layout.get(space))
                            .cloned()
                            .unwrap_or_default();
                        record.placed.insert(id, folder);
                    }
                    Err(error) => {
                        stats.failed += 1;
                        log::warn!(
                            "[space-folders] could not move {id} to its folder's Space: {error:?}"
                        );
                    }
                }
            }
            Step::Move { id, from, folder } => moves.push((id, from, folder)),
        }
    }
    if !moves.is_empty() {
        let _projection_write = db.begin_page_projection_write();
        let root_owned = root.to_path_buf();
        let placed: Vec<(String, String, Result<String, WenlanError>)> =
            tokio::task::spawn_blocking(move || {
                moves
                    .into_iter()
                    .map(|(id, from, folder)| {
                        let result = (|| {
                            make_folders(&root_owned, &folder)?;
                            crate::export::knowledge::move_projected_page(
                                &root_owned,
                                &id,
                                &from,
                                &folder,
                                &format!("space-folder-{id}"),
                            )
                        })();
                        if result.is_ok() {
                            remove_emptied(&root_owned, dir_of(&from));
                        }
                        (id, folder, result)
                    })
                    .collect()
            })
            .await
            .map_err(|e| WenlanError::Io(std::io::Error::other(e.to_string())))?;
        for (id, folder, result) in placed {
            match result {
                Ok(_) => {
                    stats.moved += 1;
                    let space_folder = layout
                        .values()
                        .filter(|f| under(&folder, f).is_some())
                        .max_by_key(|f| f.len())
                        .cloned()
                        .unwrap_or_default();
                    record.placed.insert(id, space_folder);
                }
                Err(error) => {
                    stats.failed += 1;
                    log::warn!("[space-folders] could not move {id} into {folder}: {error}");
                }
            }
        }
    }
    let live: BTreeSet<&String> = pages.iter().map(|p| &p.id).collect();
    record.placed.retain(|id, _| live.contains(id));
    save_record(root, &record)?;
    Ok(stats)
}

#[cfg(test)]
#[path = "space_folders_test.rs"]
mod tests;
