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
//! - A page moved to another Space in Wenlan, a renamed Space, or a Space
//!   that joined a team has its file moved to the new Space's folder. The
//!   folders the person made inside the old Space come along.
//! - A file the person dragged (in Finder or Obsidian) into another personal
//!   Space's folder moves its page to that Space. A personal Space's folder
//!   the person renamed renames the Space.
//! - Team Spaces are not changed from the folder: a file dragged into or out
//!   of a team's folder stays where it is and keeps its Space, so nothing is
//!   shared with a team, or taken off it, without being asked in Wenlan. A
//!   file dragged out of every Space's folder is left there too (it does not
//!   lose its Space).
//!
//! Which is which is told by where Wenlan last put each page
//! ([`Record::placed`]): only a file still under that folder is moved.
//!
//! Off until the person agrees: the record file in `<pages>/.wenlan/` is
//! both the switch and the record. The first pass moves every existing page
//! into its Space's folder, keeping the folders the person made. Notes the
//! person put in the folder themselves (adopted notes, such as an Obsidian
//! vault's) and linked vault folders are never moved.

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
    if off_path(root).is_file() {
        std::fs::rename(off_path(root), record_path(root))?;
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

fn off_path(root: &Path) -> PathBuf {
    root.join(".wenlan").join("space-folders.off.json")
}

/// Turns it off: pages stay where they are, nothing more is moved. The
/// record is kept aside, so turning it on again knows where pages were put.
pub fn turn_off(root: &Path) -> Result<(), WenlanError> {
    match std::fs::rename(record_path(root), off_path(root)) {
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
    if out.is_empty()
        || windows_reserved
        || RESERVED.iter().any(|r| out.eq_ignore_ascii_case(r))
        || crate::export::knowledge::validate_knowledge_folder_path(&out).is_err()
    {
        out = format!("{out} (Space)").trim().to_string();
    }
    out
}

fn key(name: &str) -> String {
    crate::export::knowledge::collision_key(name)
}

/// `name`, or `name 2`, `name 3`… whichever is not in `taken`.
fn unique(name: String, taken: &BTreeSet<String>) -> String {
    if !taken.contains(&key(&name)) {
        return name;
    }
    (2..)
        .map(|n| format!("{name} {n}"))
        .find(|candidate| !taken.contains(&key(candidate)))
        .unwrap_or(name)
}

/// What decides each Space's folder.
#[derive(Debug, Default, Clone)]
pub struct LayoutInput {
    /// Every Space here, by name.
    pub spaces: Vec<String>,
    /// Team Spaces: the team's name, or `None` while the team has no name
    /// yet (its pages are left where they are until it has one).
    pub teams: BTreeMap<String, Option<String>>,
    /// The folders at the top of the pages folder now, as spelled on disk.
    pub top_folders: Vec<String>,
    /// Folders never used for a Space (linked vaults).
    pub skip: Vec<String>,
}

/// Every Space's folder, by Space name. No two Spaces share a folder (in
/// any letter case or Unicode form); a folder already on disk keeps its
/// spelling; a team's folder never takes a personal Space's or a linked
/// vault's folder.
pub fn layout(input: &LayoutInput) -> BTreeMap<String, String> {
    let on_disk: BTreeMap<String, String> = input
        .top_folders
        .iter()
        .map(|folder| (key(folder), folder.clone()))
        .collect();
    let spelled = |name: String| on_disk.get(&key(&name)).cloned().unwrap_or(name);
    let mut taken: BTreeSet<String> = input.skip.iter().map(|folder| key(folder)).collect();
    let mut spaces = input.spaces.clone();
    spaces.sort();
    let mut out = BTreeMap::new();
    // Personal Spaces first, so a team never takes their folders.
    for space in spaces
        .iter()
        .filter(|space| !input.teams.contains_key(*space))
    {
        let folder = spelled(unique(folder_name(space), &taken));
        taken.insert(key(&folder));
        out.insert(space.clone(), folder);
    }
    let mut team_folders: BTreeMap<String, String> = BTreeMap::new();
    let mut inside: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for space in &spaces {
        let Some(Some(team)) = input.teams.get(space) else {
            continue;
        };
        let team_folder = match team_folders.get(&key(team)) {
            Some(folder) => folder.clone(),
            None => {
                let folder = spelled(unique(folder_name(team), &taken));
                taken.insert(key(&folder));
                team_folders.insert(key(team), folder.clone());
                folder
            }
        };
        let used = inside.entry(team_folder.clone()).or_default();
        let name = unique(folder_name(space), used);
        used.insert(key(&name));
        out.insert(space.clone(), format!("{team_folder}/{name}"));
    }
    out
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
    /// The person dragged the file into this personal Space's folder: the
    /// page moves to that Space.
    SetSpace { id: String, space: String },
    /// Already where it belongs; remember it there.
    Keep { id: String, folder: String },
}

fn dir_of(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The rest of `dir` inside `folder`, comparing names as the file system
/// does (any letter case, any Unicode form); `None` when not inside.
fn under<'a>(dir: &'a str, folder: &str) -> Option<&'a str> {
    if folder.is_empty() {
        return Some(dir);
    }
    let depth = folder.split('/').count();
    let parts: Vec<&str> = dir.split('/').collect();
    if dir.is_empty() || parts.len() < depth {
        return None;
    }
    if parts[..depth]
        .iter()
        .zip(folder.split('/'))
        .any(|(have, want)| key(have) != key(want))
    {
        return None;
    }
    let skip: usize = parts[..depth].iter().map(|part| part.len() + 1).sum();
    Some(dir.get(skip.min(dir.len())..).unwrap_or_default())
}

/// The rest of `dir` once the Space folder holding it (the deepest one) is
/// taken off; all of `dir` when no Space folder holds it.
fn outside_spaces<'a>(dir: &'a str, layout: &BTreeMap<String, String>) -> &'a str {
    layout
        .values()
        .filter_map(|folder| under(dir, folder).map(|rest| (folder.len(), rest)))
        .max_by_key(|(len, _)| *len)
        .map_or(dir, |(_, rest)| rest)
}

/// The Space whose folder holds `dir` (the deepest one).
fn owner(dir: &str, layout: &BTreeMap<String, String>) -> Option<String> {
    layout
        .iter()
        .filter(|(_, folder)| under(dir, folder).is_some())
        .max_by_key(|(_, folder)| folder.len())
        .map(|(space, _)| space.clone())
}

fn join(folder: &str, rest: &str) -> String {
    match (folder.is_empty(), rest.is_empty()) {
        (true, _) => rest.to_string(),
        (_, true) => folder.to_string(),
        _ => format!("{folder}/{rest}"),
    }
}

/// What a pass does, page by page. Pages under a `skip` folder (linked
/// vaults) or in Wenlan's own folders are left alone. `teams` names the
/// team Spaces, which the folder never changes.
pub fn plan(
    pages: &[Placement],
    layout: &BTreeMap<String, String>,
    teams: &BTreeSet<String>,
    record: &Record,
    skip: &[String],
) -> Vec<Step> {
    let personal = |space: &Option<String>| space.as_ref().is_none_or(|s| !teams.contains(s));
    let mut steps = Vec::new();
    for page in pages {
        let dir = dir_of(&page.file);
        let first = dir.split('/').next().unwrap_or_default();
        if RESERVED.iter().any(|r| key(first) == key(r))
            || first.starts_with('.')
            || skip
                .iter()
                .any(|folder| !folder.is_empty() && under(dir, folder).is_some())
        {
            continue;
        }
        let want = match &page.space {
            None => String::new(),
            Some(space) => match layout.get(space) {
                Some(folder) => folder.clone(),
                // A Space this pass cannot place yet: leave the file alone.
                None => continue,
            },
        };
        let keep = || Step::Keep {
            id: page.id.clone(),
            folder: want.clone(),
        };
        let step = match record.placed.get(&page.id) {
            // Still under the folder Wenlan put it in: the Space is what changed.
            Some(placed) if !placed.is_empty() => match under(dir, placed) {
                Some(_) if key(placed) == key(&want) => keep(),
                Some(rest) => Step::Move {
                    id: page.id.clone(),
                    from: page.file.clone(),
                    folder: join(&want, rest),
                },
                // The person moved it.
                None => match owner(dir, layout) {
                    now if now == page.space => keep(),
                    // Into another personal Space's folder: that Space.
                    Some(now) if personal(&Some(now.clone())) && personal(&page.space) => {
                        Step::SetSpace {
                            id: page.id.clone(),
                            space: now,
                        }
                    }
                    // Out of every Space's folder, or to or from a team's:
                    // left where they put it, in its Space.
                    _ => continue,
                },
            },
            // Put at the top (no Space then), or never put anywhere: into
            // its Space's folder, keeping the person's own folders. A file
            // in some other Space's folder takes only the folders inside it.
            placed => {
                let at_top = placed.is_some();
                let rest = outside_spaces(dir, layout);
                let inside_want = !want.is_empty() && under(dir, &want).is_some();
                if want.is_empty() {
                    match owner(dir, layout) {
                        None => keep(),
                        // Put at the top, then dragged into a personal
                        // Space's folder: that Space.
                        Some(now) if at_top && personal(&Some(now.clone())) => Step::SetSpace {
                            id: page.id.clone(),
                            space: now,
                        },
                        // In some Space's folder from before: left there.
                        Some(_) => continue,
                    }
                } else if inside_want {
                    keep()
                } else {
                    Step::Move {
                        id: page.id.clone(),
                        from: page.file.clone(),
                        folder: join(&want, rest),
                    }
                }
            }
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

/// Linked vault folders, never used for a Space and never moved.
fn vault_folders() -> Vec<String> {
    crate::config::load_config()
        .linked_vaults
        .into_iter()
        .map(|vault| vault.folder)
        .collect()
}

/// Everything [`layout`] needs, read now. An unreadable team settings file
/// is an error: guessing would move team Spaces out and back.
async fn layout_input(
    db: &MemoryDB,
    root: &Path,
    settings_path: &Path,
    skip: &[String],
) -> Result<LayoutInput, WenlanError> {
    let spaces = db.space_names().await?;
    let teams = crate::shared_space::team_names_by_space(settings_path)?;
    let root_owned = root.to_path_buf();
    let top_folders = tokio::task::spawn_blocking(move || {
        crate::export::knowledge::list_knowledge_folders(&root_owned)
    })
    .await
    .map_err(|e| WenlanError::Io(std::io::Error::other(e.to_string())))??
    .into_iter()
    .filter(|folder| folder.parent_path.is_empty())
    .map(|folder| folder.name)
    .collect();
    Ok(LayoutInput {
        spaces,
        teams,
        top_folders,
        skip: skip.to_vec(),
    })
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
    let input = layout_input(
        db,
        root,
        &crate::shared_space::settings_path(),
        &vault_folders(),
    )
    .await
    .ok()?;
    let folder = layout(&input).get(space)?.clone();
    make_folders(root, &folder)
        .map_err(|error| log::warn!("[space-folders] could not make {folder}: {error}"))
        .ok()?;
    Some(folder)
}

/// Most moves one pass makes: each move reads the folder's notes once, and
/// the page watcher waits for the pass. The rest go in the next passes.
pub const MOVES_PER_PASS: usize = 40;

/// How long a page whose move failed is left before it is tried again.
const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Pages whose move failed, and when, so a pass does not retry (and warn)
/// every 30 seconds.
static FAILED: std::sync::Mutex<BTreeMap<String, std::time::Instant>> =
    std::sync::Mutex::new(BTreeMap::new());

fn failed_recently(id: &str) -> bool {
    FAILED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(id)
        .is_some_and(|at| at.elapsed() < RETRY_AFTER)
}

/// The folder a page's file is in now.
fn page_dir(pages: &[Placement], id: &str) -> String {
    pages
        .iter()
        .find(|page| page.id == id)
        .map(|page| dir_of(&page.file).to_string())
        .unwrap_or_default()
}

/// Personal Spaces whose folder the person renamed: the folder is gone,
/// and every page Wenlan put in it is now in one other top folder that is
/// no Space's, no linked vault's and not Wenlan's own.
pub fn folder_renames(
    pages: &[Placement],
    layout: &BTreeMap<String, String>,
    teams: &BTreeSet<String>,
    input: &LayoutInput,
    record: &Record,
) -> Vec<(String, String)> {
    let on_disk: BTreeSet<String> = input.top_folders.iter().map(|f| key(f)).collect();
    let taken: BTreeSet<String> = layout
        .values()
        .map(|folder| key(folder.split('/').next().unwrap_or_default()))
        .chain(input.skip.iter().map(|folder| key(folder)))
        .collect();
    let mut out = Vec::new();
    for (space, folder) in layout {
        if teams.contains(space) || folder.contains('/') || on_disk.contains(&key(folder)) {
            continue;
        }
        let tops: BTreeSet<&str> = pages
            .iter()
            .filter(|page| page.space.as_deref() == Some(space.as_str()))
            .filter(|page| {
                record
                    .placed
                    .get(&page.id)
                    .is_some_and(|p| key(p) == key(folder))
            })
            .map(|page| dir_of(&page.file).split('/').next().unwrap_or_default())
            .collect();
        let [top] = tops.into_iter().collect::<Vec<_>>()[..] else {
            continue;
        };
        if top.is_empty()
            || taken.contains(&key(top))
            || RESERVED.iter().any(|r| key(r) == key(top))
            || !on_disk.contains(&key(top))
        {
            continue;
        }
        out.push((space.clone(), top.to_string()));
    }
    out
}

/// What one pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PassStats {
    pub moved: usize,
    /// Pages moved to the Space whose folder the person dragged them into.
    pub respaced: usize,
    /// Spaces renamed because the person renamed their folder.
    pub renamed: usize,
    pub failed: usize,
    /// Moves left for the next passes.
    pub waiting: usize,
}

/// One pass: moves the files whose page's Space changed (or that were
/// never put in their Space's folder). Does nothing while it is off.
pub async fn arrange(
    db: &MemoryDB,
    root: &Path,
    settings_path: &Path,
    skip: &[String],
) -> Result<PassStats, WenlanError> {
    let Some(record) = read_record(root)? else {
        return Ok(PassStats::default());
    };
    let mut next = record.clone();
    let input = layout_input(db, root, settings_path, skip).await?;
    let layout = layout(&input);
    let teams: BTreeSet<String> = input.teams.keys().cloned().collect();
    let spaces = db.active_page_spaces().await?;
    let root_owned = root.to_path_buf();
    let (files, person) = tokio::task::spawn_blocking(move || {
        Ok::<_, WenlanError>((
            crate::export::knowledge::knowledge_edit_targets(&root_owned)?,
            crate::export::knowledge::person_page_files(&root_owned)?,
        ))
    })
    .await
    .map_err(|e| WenlanError::Io(std::io::Error::other(e.to_string())))??;
    let pages: Vec<Placement> = files
        .into_iter()
        // A note the person put in the folder is theirs: never moved (its
        // images and links would break).
        .filter(|(id, _)| !person.contains_key(id))
        .filter_map(|(id, file)| {
            let space = spaces.get(&id)?.clone();
            Some(Placement {
                id,
                file,
                space: space.filter(|s| s != UNFILED_SPACE_ID && s != "uncategorized"),
            })
        })
        .collect();
    let mut stats = PassStats::default();
    let mut moves = Vec::new();
    // A personal Space's folder renamed in Finder renames the Space; the
    // next pass then finds every page where it belongs.
    let renames = folder_renames(&pages, &layout, &teams, &input, &record);
    if !renames.is_empty() {
        let descriptions: BTreeMap<String, Option<String>> = db
            .list_spaces()
            .await?
            .into_iter()
            .map(|space| (space.name, space.description))
            .collect();
        for (space, folder) in renames {
            let description = descriptions.get(&space).cloned().flatten();
            match db
                .update_space(&space, &folder, description.as_deref())
                .await
            {
                Ok(_) => {
                    stats.renamed += 1;
                    log::info!(
                        "[space-folders] Space {space} renamed to {folder}, as its folder was"
                    );
                }
                Err(error) => {
                    stats.failed += 1;
                    log::warn!("[space-folders] folder of {space} is now {folder}, but the Space was not renamed: {error}");
                }
            }
        }
        return Ok(stats);
    }
    for step in plan(&pages, &layout, &teams, &record, skip) {
        match step {
            Step::Keep { id, folder } => {
                next.placed.insert(id, folder);
            }
            Step::SetSpace { id, space } => {
                let Some(page) = db.get_page(&id).await? else {
                    continue;
                };
                match db
                    .set_page_space(
                        &id,
                        Some(&space),
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
                                log::warn!("[space-folders] {id} moved to {space} but its file was not rewritten: {error}");
                            }
                        }
                        next.placed
                            .insert(id, layout.get(&space).cloned().unwrap_or_default());
                    }
                    Err(error) => {
                        stats.failed += 1;
                        log::warn!(
                            "[space-folders] {id} was dragged into {space}'s folder, but could not move there: {error:?}"
                        );
                        // It keeps its Space: remember it where it is, so
                        // the next pass puts it back in its Space's folder
                        // instead of trying again.
                        let dir = page_dir(&pages, &id);
                        next.placed.insert(id, dir);
                    }
                }
            }
            Step::Move { .. } if moves.len() >= MOVES_PER_PASS => stats.waiting += 1,
            Step::Move { id, .. } if failed_recently(&id) => {}
            Step::Move { id, from, folder } => moves.push((id, from, folder)),
        }
    }
    if !moves.is_empty() {
        let _projection_write = db.begin_page_projection_write();
        let root_owned = root.to_path_buf();
        let done: Vec<(String, String, Result<String, WenlanError>)> =
            tokio::task::spawn_blocking(move || {
                moves
                    .into_iter()
                    .map(|(id, from, folder)| {
                        let result = make_folders(&root_owned, &folder).and_then(|()| {
                            crate::export::knowledge::move_projected_page(
                                &root_owned,
                                &id,
                                &from,
                                &folder,
                                &format!("space-folder-{id}"),
                            )
                        });
                        if result.is_ok() {
                            remove_emptied(&root_owned, dir_of(&from));
                        }
                        (id, folder, result)
                    })
                    .collect()
            })
            .await
            .map_err(|e| WenlanError::Io(std::io::Error::other(e.to_string())))?;
        for (id, folder, result) in done {
            match result {
                Ok(_) => {
                    stats.moved += 1;
                    // Moved because its Space changed (not a first move):
                    // its file's `space:` line says the new Space too.
                    if record.placed.contains_key(&id) {
                        if let Ok(Some(page)) = db.get_page(&id).await {
                            let projection =
                                crate::export::knowledge::KnowledgeProjectionWrite::new(
                                    root.to_path_buf(),
                                    db,
                                );
                            if let Err(error) = projection.write_page_gated(db, &page).await {
                                log::warn!("[space-folders] moved {id} but its file was not rewritten: {error}");
                            }
                        }
                    }
                    let space_folder = layout
                        .values()
                        .filter(|f| under(&folder, f).is_some())
                        .max_by_key(|f| f.len())
                        .cloned()
                        .unwrap_or_default();
                    next.placed.insert(id, space_folder);
                }
                Err(error) => {
                    stats.failed += 1;
                    log::warn!(
                        "[space-folders] could not move {id} into {folder} (tried again in 15 min): {error}"
                    );
                    FAILED
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(id, std::time::Instant::now());
                }
            }
        }
    }
    // Forget only pages that are gone, not ones missing from one scan.
    next.placed.retain(|id, _| spaces.contains_key(id));
    if next != record {
        save_record(root, &next)?;
    }
    Ok(stats)
}

#[cfg(test)]
#[path = "space_folders_test.rs"]
mod tests;
