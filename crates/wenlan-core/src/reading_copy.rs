//! One-way "reading copy" of the person's pages as plain Markdown files.
//!
//! The folder (usually inside a cloud drive) lets a phone AI tool read pages
//! without a computer being on. Wenlan writes it and never reads it back as
//! edits: a changed copy is overwritten on the next pass. Layout:
//! `<dir>/<Space>/<title>.md`, plus an instructions file at the top. Earlier
//! versions of a page go to `<dir>/歷史/<Space>/<title>.md`, newest first, so
//! an AI tool can see how a page came to be.
//!
//! [`write_reading_copy`] is a pure, synchronous function over plain data, so
//! it is tested without a database. [`pages_for_copy`] is the only part that
//! touches `MemoryDB`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::{MemoryDB, UNFILED_SPACE_ID};
use crate::error::WenlanError;

/// File name of the instructions file written next to the Space folders.
pub const INSTRUCTIONS_FILE: &str = "給 AI 的說明.md";

/// Folder, next to the Space folders, that holds each page's earlier versions.
pub const HISTORY_DIR: &str = "歷史";
/// Earlier versions kept in a page's history file.
pub const HISTORY_KEEP: usize = 10;

/// A removal pass that would delete more than this many files...
const MASS_REMOVAL_MIN: usize = 20;
/// Longest file stem or folder name, in characters.
const NAME_MAX_CHARS: usize = 120;
/// Longest file stem or folder name, in bytes (leaves room for ` (NN).md`).
const NAME_MAX_BYTES: usize = 180;

/// A page as the reading copy needs it.
#[derive(Debug, Clone)]
pub struct CopyPage {
    pub id: String,
    pub title: String,
    /// `None` means Unfiled.
    pub space: Option<String>,
    pub body: String,
    /// RFC 3339 last-modified time.
    pub modified: String,
    /// Earlier versions, newest first. `None` when they were not read this
    /// pass because the page did not change: the history file written last
    /// time stays (moved along if the page moved).
    pub history: Option<Vec<CopyVersion>>,
}

/// One earlier version of a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyVersion {
    /// RFC 3339 time the version was saved.
    pub at: String,
    /// The person wrote it (on any of their computers); otherwise an AI.
    pub by_person: bool,
    pub body: String,
}

/// What Wenlan wrote last time, so it only ever removes its own files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyState {
    /// Relative path (always `/`-separated) to the file Wenlan wrote.
    #[serde(default)]
    pub files: BTreeMap<String, CopyFile>,
    /// Page id to the `modified` time its history was last read at, so an
    /// unchanged page's history is not read again.
    #[serde(default)]
    pub history_seen: BTreeMap<String, String>,
}

impl CopyState {
    /// Whether this page's earlier versions must be read for the next pass.
    pub fn needs_history(&self, page_id: &str, modified: &str) -> bool {
        self.history_seen.get(page_id).map(String::as_str) != Some(modified)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyFile {
    /// Hex SHA-256 of the bytes Wenlan wrote.
    pub sha256: String,
    pub page_id: String,
    /// The page's history file rather than the page itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub history: bool,
    /// The file's size and modification time right after Wenlan wrote or
    /// last read it. A file that kept both is taken as unchanged without
    /// reading it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<FileStamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub len: u64,
    /// Nanoseconds since the Unix epoch.
    pub modified: u128,
}

fn stamp_of(path: &Path) -> Option<FileStamp> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(FileStamp {
        len: meta.len(),
        modified,
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyResult {
    pub written: u32,
    pub removed: u32,
    pub unchanged: u32,
    pub removal_held_back: bool,
}

/// Active pages for the copy: every Space including Unfiled, never archived,
/// minus pages whose Space is in `skip_spaces`. Earlier versions are read
/// only for pages that changed since `state` last saw them.
pub async fn pages_for_copy(
    db: &MemoryDB,
    skip_spaces: &HashSet<String>,
    state: &CopyState,
) -> Result<Vec<CopyPage>, WenlanError> {
    let pages = db.list_pages_by_space("active", None, 1_000_000, 0).await?;
    let mut out: Vec<CopyPage> = pages
        .into_iter()
        .filter(|p| {
            p.space
                .as_deref()
                .map(|s| !skip_spaces.contains(s))
                .unwrap_or(true)
        })
        .map(|p| CopyPage {
            space: p
                .space
                .filter(|s| !s.is_empty() && s != UNFILED_SPACE_ID && s != "uncategorized"),
            id: p.id,
            title: p.title,
            body: p.content,
            modified: p.last_modified,
            history: None,
        })
        .collect();
    for page in &mut out {
        if state.needs_history(&page.id, &page.modified) {
            // Saves are folded, so read more rows than are kept.
            let rows = db
                .list_page_history(&page.id, (HISTORY_KEEP * 6) as i64)
                .await?;
            page.history = Some(earlier_versions(&rows, &page.body));
        }
    }
    Ok(out)
}

/// Earlier versions from the page's history rows (newest first): the current
/// text left out, a run of the person's saves listed once (its last save, as
/// the page's own history lists it), at most [`HISTORY_KEEP`].
pub fn earlier_versions(rows: &[crate::db::PageHistoryEntry], current: &str) -> Vec<CopyVersion> {
    let mut out: Vec<CopyVersion> = Vec::new();
    // The open run of the person's saves: its writer and its newest save.
    let mut run: Option<(&str, i64)> = None;
    let mut last_kept_body: &str = current;
    for row in rows {
        let folded = run.is_some_and(|(writer, newest)| {
            writer == row.edited_by && newest - row.created_at <= crate::page_history::SAVE_RUN_SECS
        });
        if !folded {
            run = crate::page_history::folds(Some(row.edited_by.as_str()))
                .then_some((row.edited_by.as_str(), row.created_at));
        }
        if folded || row.content == last_kept_body {
            continue;
        }
        last_kept_body = &row.content;
        out.push(CopyVersion {
            at: chrono::DateTime::from_timestamp(row.created_at, 0)
                .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
                .unwrap_or_default(),
            by_person: crate::page_history::kind_of_edit(&row.edited_by)
                == wenlan_types::responses::PageHistoryKind::PersonEdit,
            body: row.content.clone(),
        });
        if out.len() >= HISTORY_KEEP {
            break;
        }
    }
    out
}

/// Make `dir` match `pages`. See the module docs for the rules.
pub fn write_reading_copy(
    dir: &Path,
    pages: &[CopyPage],
    state: &mut CopyState,
    now: &str,
    space_label_unfiled: &str,
) -> Result<CopyResult, WenlanError> {
    let mut result = CopyResult::default();
    let desired = assign_paths(pages, space_label_unfiled);
    let state_len_before = state.files.len();

    let mut keep: HashSet<String> = desired.keys().cloned().collect();
    for (rel, page) in &desired {
        let bytes = render(page, space_label_unfiled).into_bytes();
        put(dir, rel, bytes, &page.id, false, state, &mut result)?;
    }

    // Each page's earlier versions, under the history folder at the same path.
    for (rel, page) in &desired {
        let history_rel = format!("{HISTORY_DIR}/{rel}");
        match &page.history {
            Some(versions) => {
                state
                    .history_seen
                    .insert(page.id.clone(), page.modified.clone());
                if versions.is_empty() {
                    continue;
                }
                let bytes = render_history(page, versions).into_bytes();
                put(dir, &history_rel, bytes, &page.id, true, state, &mut result)?;
                keep.insert(history_rel);
            }
            None => {
                let written = state
                    .files
                    .iter()
                    .find(|(_, file)| file.history && file.page_id == page.id)
                    .map(|(old, file)| (old.clone(), file.clone()));
                let Some((old_rel, file)) = written else {
                    continue;
                };
                if old_rel != history_rel {
                    let from = dir.join(rel_to_path(&old_rel));
                    let to = dir.join(rel_to_path(&history_rel));
                    if !plainly_inside(dir, &from) || !plainly_inside(dir, &to) {
                        state.history_seen.remove(&page.id);
                        continue;
                    }
                    let moved = to
                        .parent()
                        .map_or(Ok(()), fs::create_dir_all)
                        .and_then(|()| fs::rename(&from, &to));
                    if moved.is_err() {
                        // Read again next pass; the old file goes as stale.
                        state.history_seen.remove(&page.id);
                        continue;
                    }
                    if let Some(parent) = from.parent() {
                        if parent != dir {
                            let _ = fs::remove_dir(parent);
                        }
                    }
                    state.files.remove(&old_rel);
                    state.files.insert(history_rel.clone(), file);
                }
                keep.insert(history_rel);
            }
        }
    }
    let ids: HashSet<&str> = pages.iter().map(|page| page.id.as_str()).collect();
    state.history_seen.retain(|id, _| ids.contains(id.as_str()));

    // Files Wenlan wrote earlier whose page no longer lands there.
    let stale: Vec<String> = state
        .files
        .keys()
        .filter(|rel| !keep.contains(*rel))
        .cloned()
        .collect();
    if stale.len() > MASS_REMOVAL_MIN && stale.len() * 2 > state_len_before {
        log::warn!(
            "[reading_copy] holding back removal of {} of {} files",
            stale.len(),
            state_len_before
        );
        result.removal_held_back = true;
    } else {
        // On a case-insensitive disk a case-only rename writes over the old
        // file, so deleting the old name would delete the new page.
        let desired_lower: HashSet<String> = keep.iter().map(|k| k.to_lowercase()).collect();
        for rel in stale {
            let path = dir.join(rel_to_path(&rel));
            if !plainly_inside(dir, &path) {
                // Never follow a link out of the reading copy; forget the file.
                state.files.remove(&rel);
                continue;
            }
            if !desired_lower.contains(&rel.to_lowercase()) || exact_name_present(&path) {
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
                // Fails (and is ignored) while a folder is not empty.
                let mut parent = path.parent();
                while let Some(folder) = parent.filter(|folder| *folder != dir) {
                    if fs::remove_dir(folder).is_err() {
                        break;
                    }
                    parent = folder.parent();
                }
            }
            state.files.remove(&rel);
            result.removed += 1;
        }
    }

    let instructions = dir.join(INSTRUCTIONS_FILE);
    if result.written > 0 || result.removed > 0 || !instructions.exists() {
        write_atomic(&instructions, instructions_text(now).as_bytes())?;
    }
    Ok(result)
}

/// Write `bytes` at `rel` unless the same bytes are already there, and record
/// it as Wenlan's file.
fn put(
    dir: &Path,
    rel: &str,
    bytes: Vec<u8>,
    page_id: &str,
    history: bool,
    state: &mut CopyState,
    result: &mut CopyResult,
) -> Result<(), WenlanError> {
    let sha = sha_hex(&bytes);
    let target = dir.join(rel_to_path(rel));
    if !plainly_inside(dir, &target) {
        log::warn!("[reading_copy] a folder on the way to {rel} links elsewhere; not written");
        return Ok(());
    }
    // What Wenlan wrote last time, still there with the same size and
    // modification time: not read again, so a large library is not re-read
    // on every pass. Anything else (no stamp, a file someone replaced, a
    // cloud placeholder) is read and compared.
    let now = stamp_of(&target);
    let recorded_same = now.is_some()
        && state
            .files
            .get(rel)
            .is_some_and(|file| file.sha256 == sha && file.stamp == now);
    let on_disk_same = recorded_same || fs::read(&target).map(|b| b == bytes).unwrap_or(false);
    let stamp = if on_disk_same {
        result.unchanged += 1;
        now
    } else {
        write_atomic(&target, &bytes)?;
        result.written += 1;
        stamp_of(&target)
    };
    state.files.insert(
        rel.to_string(),
        CopyFile {
            sha256: sha,
            page_id: page_id.to_string(),
            history,
            stamp,
        },
    );
    Ok(())
}

/// Whether the folder lists a file with exactly this name. After a
/// case-only rename a case-insensitive disk lists only the new name (the old
/// one is the same file), while a case-sensitive disk still lists the old one.
fn exact_name_present(path: &Path) -> bool {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    fs::read_dir(parent).is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .any(|entry| entry.file_name() == name)
    })
}

fn instructions_text(now: &str) -> String {
    format!(
        "# 給 AI 的說明\n\
         \n\
         這個資料夾裡的檔案，是使用者 Wenlan 頁面的唯讀副本，最後更新於 {now}。\n\
         \n\
         - 請放心閱讀這些檔案。\n\
         - 請不要編輯它們：Wenlan 下次更新時會覆蓋你的修改。\n\
         - 如果你能直接透過連接器連到 Wenlan，請改用連接器。\n\
         - 若要儲存新的內容給 Wenlan，請在這個檔案旁邊的「收件匣」資料夾裡建立新檔案。\n\
         - 「歷史」資料夾裡同名的檔案是一頁以前的版本，新的在上，標著日期和誰改的。用它看這頁怎麼變過來；引用內容時以「歷史」外面的最新版為準。\n\
         \n\
         # Note for AI tools\n\
         \n\
         The files in this folder are read-only copies of the person's Wenlan pages, updated {now}.\n\
         \n\
         - Read them freely.\n\
         - Do not edit them: your edits are overwritten the next time Wenlan updates the copy.\n\
         - If you can reach Wenlan directly through its connector, use that instead.\n\
         - To save something new for Wenlan, create a new file in the folder `收件匣` next to this file.\n\
         - The file with the same path under `歷史` holds a page's earlier versions, newest first, each with its date and who changed it. Use it to see how the page came to be; quote the current version outside `歷史`.\n"
    )
}

fn render(page: &CopyPage, space_label_unfiled: &str) -> String {
    let space = page.space.as_deref().unwrap_or(space_label_unfiled);
    format!(
        "---\ntitle: {}\nspace: {}\nupdated: {}\n---\n\n{}",
        yaml_str(&page.title),
        yaml_str(space),
        yaml_str(&page.modified),
        page.body
    )
}

/// A page's earlier versions, newest first.
fn render_history(page: &CopyPage, versions: &[CopyVersion]) -> String {
    let mut out = format!(
        "---\ntitle: {}\nearlier_versions: true\n---\n\n\
         這是「{}」以前的版本，新的在上。現在的內容以「歷史」外面同名的檔案為準。\n\
         Earlier versions of this page, newest first. The current page is the file with the same path outside `歷史`.\n",
        yaml_str(&page.title),
        page.title
    );
    for version in versions {
        let who = if version.by_person {
            "使用者改的 · by the person"
        } else {
            "AI 改的 · by AI"
        };
        out.push_str(&format!(
            "\n---\n\n## {} · {}\n\n{}\n",
            version.at,
            who,
            version.body.trim_end()
        ));
    }
    out
}

/// A JSON string is a valid double-quoted YAML scalar.
fn yaml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn sha_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Whether every folder between `dir` and `path` is a real folder, not a
/// link to somewhere else (a symlink, or a junction on Windows). Wenlan
/// writes and removes files only there, so a link inside the reading copy
/// can never send a write or a removal outside it. Folders not made yet
/// count as inside: Wenlan makes them as plain folders.
fn plainly_inside(dir: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(dir) else {
        return false;
    };
    let mut at = dir.to_path_buf();
    let folders: Vec<_> = rel.components().collect();
    for part in folders.iter().take(folders.len().saturating_sub(1)) {
        at.push(part);
        match fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => return false,
            Ok(_) => {}
            Err(_) => return true,
        }
    }
    true
}

fn rel_to_path(rel: &str) -> PathBuf {
    rel.split('/').collect()
}

/// Deterministic relative path (`Space/title.md`) for every page, in id order.
/// Paths that differ only by case collide too (macOS and Windows).
fn assign_paths<'a>(
    pages: &'a [CopyPage],
    space_label_unfiled: &str,
) -> BTreeMap<String, &'a CopyPage> {
    let mut sorted: Vec<&CopyPage> = pages.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));

    let mut out = BTreeMap::new();
    let mut taken: HashMap<String, ()> = HashMap::new();
    for page in sorted {
        let short = short_id(&page.id);
        let space_raw = page.space.as_deref().unwrap_or(space_label_unfiled);
        let folder = sanitize_name(space_raw).unwrap_or_else(|| short.clone());
        let stem = sanitize_name(&page.title).unwrap_or_else(|| short.clone());
        let mut n = 1u32;
        loop {
            let file = if n == 1 {
                format!("{stem}.md")
            } else {
                format!("{stem} ({n}).md")
            };
            let rel = format!("{folder}/{file}");
            if taken.insert(rel.to_lowercase(), ()).is_none() {
                out.insert(rel, page);
                break;
            }
            n += 1;
        }
    }
    out
}

fn short_id(id: &str) -> String {
    let s: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    if s.is_empty() {
        "page".to_string()
    } else {
        s
    }
}

/// A name safe on Windows, macOS and Linux, or `None` when nothing is left.
fn sanitize_name(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .collect();
    let mut name = cleaned.trim_matches([' ', '.']).to_string();
    if name.is_empty() {
        return None;
    }
    // Length caps, on a char boundary.
    let mut capped = String::new();
    for c in name.chars() {
        if capped.chars().count() >= NAME_MAX_CHARS || capped.len() + c.len_utf8() > NAME_MAX_BYTES
        {
            break;
        }
        capped.push(c);
    }
    name = capped.trim_end_matches([' ', '.']).to_string();
    if name.is_empty() {
        return None;
    }
    let stem = name.split('.').next().unwrap_or("").trim_end();
    if is_windows_reserved(stem) {
        name.push('_');
    }
    Some(name)
}

fn is_windows_reserved(stem: &str) -> bool {
    let up = stem.to_ascii_uppercase();
    matches!(up.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((up.starts_with("COM") || up.starts_with("LPT"))
            && up.len() == 4
            && matches!(up.as_bytes()[3], b'1'..=b'9'))
}

/// Write via a dot-prefixed temp file in the same folder, then rename.
fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), WenlanError> {
    let parent = target
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no parent folder"))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".wenlan-tmp-{}",
        &sha_hex(target.to_string_lossy().as_bytes())[..12]
    ));
    let res = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, target)
    })();
    if let Err(e) = res {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "reading_copy_test.rs"]
mod tests;
