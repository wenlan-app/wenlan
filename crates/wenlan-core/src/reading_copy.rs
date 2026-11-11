//! One-way "reading copy" of the person's pages as plain Markdown files.
//!
//! The folder (usually inside a cloud drive) lets a phone AI tool read pages
//! without a computer being on. Wenlan writes it and never reads it back as
//! edits: a changed copy is overwritten on the next pass. Layout:
//! `<dir>/<Space>/<title>.md`, plus an instructions file at the top.
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
}

/// What Wenlan wrote last time, so it only ever removes its own files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyState {
    /// Relative path (always `/`-separated) to the file Wenlan wrote.
    #[serde(default)]
    pub files: BTreeMap<String, CopyFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyFile {
    /// Hex SHA-256 of the bytes Wenlan wrote.
    pub sha256: String,
    pub page_id: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyResult {
    pub written: u32,
    pub removed: u32,
    pub unchanged: u32,
    pub removal_held_back: bool,
}

/// Active pages for the copy: every Space including Unfiled, never archived,
/// minus pages whose Space is in `skip_spaces`.
pub async fn pages_for_copy(
    db: &MemoryDB,
    skip_spaces: &HashSet<String>,
) -> Result<Vec<CopyPage>, WenlanError> {
    let pages = db.list_pages_by_space("active", None, 1_000_000, 0).await?;
    Ok(pages
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
        })
        .collect())
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

    for (rel, page) in &desired {
        let bytes = render(page, space_label_unfiled).into_bytes();
        let sha = sha_hex(&bytes);
        let target = dir.join(rel_to_path(rel));
        // What Wenlan wrote last time, still there at the same size: not read
        // again, so a large library is not re-read on every pass.
        let recorded_same = state.files.get(rel).is_some_and(|file| file.sha256 == sha)
            && fs::metadata(&target).is_ok_and(|meta| meta.len() == bytes.len() as u64);
        let on_disk_same = recorded_same || fs::read(&target).map(|b| b == bytes).unwrap_or(false);
        if on_disk_same {
            result.unchanged += 1;
        } else {
            write_atomic(&target, &bytes)?;
            result.written += 1;
        }
        state.files.insert(
            rel.clone(),
            CopyFile {
                sha256: sha,
                page_id: page.id.clone(),
            },
        );
    }

    // Files Wenlan wrote earlier whose page no longer lands there.
    let stale: Vec<String> = state
        .files
        .keys()
        .filter(|rel| !desired.contains_key(*rel))
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
        let desired_lower: HashSet<String> = desired.keys().map(|k| k.to_lowercase()).collect();
        for rel in stale {
            let path = dir.join(rel_to_path(&rel));
            if !desired_lower.contains(&rel.to_lowercase()) || exact_name_present(&path) {
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
                if let Some(parent) = path.parent() {
                    if parent != dir {
                        // Fails (and is ignored) while the folder is not empty.
                        let _ = fs::remove_dir(parent);
                    }
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
         \n\
         # Note for AI tools\n\
         \n\
         The files in this folder are read-only copies of the person's Wenlan pages, updated {now}.\n\
         \n\
         - Read them freely.\n\
         - Do not edit them: your edits are overwritten the next time Wenlan updates the copy.\n\
         - If you can reach Wenlan directly through its connector, use that instead.\n\
         - To save something new for Wenlan, create a new file in the folder `收件匣` next to this file.\n"
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

/// A JSON string is a valid double-quoted YAML scalar.
fn yaml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn sha_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
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
