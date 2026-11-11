// SPDX-License-Identifier: Apache-2.0
//! One page version in the shared folder, and how the folder is read.
//!
//! A version file is today's page file (`store::format::page`) with one more
//! frontmatter key, `shared`:
//!
//! ```text
//! ---
//! id: page_…
//! title: Seating plan
//! type: page
//! tags: []
//! created: …
//! modified: …
//! shared:
//!   parent: <sha256 of the version this one was written over>
//!   merged: [<sha256 of other latest versions this one replaces>]
//!   body_digest: <sha256 of the body>
//!   author:
//!     member: <member id>
//!     kind: person | ai
//!     agent: <AI tool, when kind is ai and it is known>
//!   ts: <when it was written; for display, and to pick between two
//!        versions written at the same time>
//!   archived: true   (only on a version that archives the page)
//! ---
//! ```
//!
//! A version's content is its title (the page's own `title` key), its body
//! and whether it archives the page ([`State`]): a rename is a new version
//! even when the body is the same, and so is an archive (a tombstone that
//! keeps the last title and body, so a restore brings them back).
//!
//! `parent` is left out on a page's first version, `merged` when it is
//! empty and `archived` when it is false. The file is named by the sha256 of its bytes, so a file that is
//! still arriving, or was changed, does not match its name and is skipped
//! until it is whole.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::WenlanError;
use crate::store::format::page::{self, PageFile};

/// The frontmatter key that holds a version's sharing facts.
const SHARED_KEY: &str = "shared";

/// `members/<member_id>/pages/<page_id>/<sha256>.md`.
pub const MEMBERS_DIR: &str = "members";
pub const PAGES_DIR: &str = "pages";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorKind {
    Person,
    Ai,
}

impl AuthorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthorKind::Person => "person",
            AuthorKind::Ai => "ai",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "person" => Some(AuthorKind::Person),
            "ai" => Some(AuthorKind::Ai),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    pub member: String,
    pub kind: AuthorKind,
    pub agent: Option<String>,
}

/// The `shared` frontmatter of a version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionMeta {
    pub parent: Option<String>,
    pub merged: Vec<String>,
    pub body_digest: String,
    pub author: Author,
    pub ts: String,
    /// The version archives the page.
    pub archived: bool,
    /// The page is locked for review: an AI tool's change to it waits for a
    /// member to take it. Old versions without the key read as unlocked.
    pub locked: bool,
}

/// What two versions, or a version and the page here, are compared by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub title: String,
    pub body_digest: String,
    pub archived: bool,
    pub locked: bool,
}

/// A version read from the folder.
#[derive(Debug, Clone, PartialEq)]
pub struct PageVersion {
    /// The file's name without `.md`: the sha256 of its bytes.
    pub sha: String,
    pub meta: VersionMeta,
    /// The page as written, without the `shared` key.
    pub page: PageFile,
}

impl PageVersion {
    pub fn state(&self) -> State {
        State {
            title: self.page.title.clone(),
            body_digest: self.meta.body_digest.clone(),
            archived: self.meta.archived,
            locked: self.meta.locked,
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The digest two versions are compared by: the page's text.
pub fn body_digest(body: &str) -> String {
    sha256_hex(body.as_bytes())
}

/// The extension version files are written with. Not `.md`, so an AI tool
/// browsing a shared folder (a Drive connector) does not take the history
/// of a page for notes and answer from an old version.
pub const VERSION_EXT: &str = "wlv";
/// The extension older Wenlans wrote; still read.
const OLD_VERSION_EXT: &str = "md";

/// A version file's name, `<sha256>.wlv` (or `<sha256>.md` as older
/// Wenlans wrote), when `name` is one.
pub fn version_sha(name: &str) -> Option<&str> {
    let (sha, ext) = name.rsplit_once('.')?;
    if ext != VERSION_EXT && ext != OLD_VERSION_EXT {
        return None;
    }
    (sha.len() == 64
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then_some(sha)
}

/// The file name a version with `sha` is written under.
pub fn version_file_name(sha: &str) -> String {
    format!("{sha}.{VERSION_EXT}")
}

/// Whether the version file at `path`, under either extension, is there.
pub fn version_file_exists(path: &Path) -> bool {
    path.exists()
        || [VERSION_EXT, OLD_VERSION_EXT]
            .iter()
            .any(|ext| path.with_extension(ext).exists())
}

/// A version file's path with no extension, so the same version under the
/// old and new name compares equal.
pub fn version_key(path: &str) -> &str {
    path.strip_suffix(&format!(".{VERSION_EXT}"))
        .or_else(|| path.strip_suffix(&format!(".{OLD_VERSION_EXT}")))
        .unwrap_or(path)
}

/// A name that is safe as one folder name on every platform: a page or
/// member id.
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn text(value: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(value.to_string())
}

fn key(name: &str) -> serde_yaml::Value {
    text(name)
}

/// Writes a version file.
pub fn render(page: &PageFile, meta: &VersionMeta) -> String {
    let mut author = serde_yaml::Mapping::new();
    author.insert(key("member"), text(&meta.author.member));
    author.insert(key("kind"), text(meta.author.kind.as_str()));
    if let Some(agent) = &meta.author.agent {
        author.insert(key("agent"), text(agent));
    }
    let mut shared = serde_yaml::Mapping::new();
    if let Some(parent) = &meta.parent {
        shared.insert(key("parent"), text(parent));
    }
    if !meta.merged.is_empty() {
        shared.insert(
            key("merged"),
            serde_yaml::Value::Sequence(meta.merged.iter().map(|sha| text(sha)).collect()),
        );
    }
    shared.insert(key("body_digest"), text(&meta.body_digest));
    shared.insert(key("author"), serde_yaml::Value::Mapping(author));
    shared.insert(key("ts"), text(&meta.ts));
    if meta.archived {
        shared.insert(key("archived"), serde_yaml::Value::Bool(true));
    }
    if meta.locked {
        shared.insert(key("locked"), serde_yaml::Value::Bool(true));
    }

    let mut page = page.clone();
    page.other.shift_remove(SHARED_KEY);
    page.other
        .insert(key(SHARED_KEY), serde_yaml::Value::Mapping(shared));
    page::render(&page)
}

fn invalid(why: impl Into<String>) -> WenlanError {
    WenlanError::Validation(why.into())
}

fn scalar(map: &serde_yaml::Mapping, name: &str) -> Option<String> {
    match map.get(name)? {
        serde_yaml::Value::String(value) => Some(value.clone()),
        _ => None,
    }
}

/// Reads the version file `name` (`<sha256>.md`) from its bytes. A file
/// whose bytes do not match its name, or whose body does not match its
/// `body_digest`, is not a whole version.
pub fn parse(name: &str, bytes: &[u8]) -> Result<PageVersion, WenlanError> {
    let sha = version_sha(name).ok_or_else(|| invalid(format!("{name} is not a version")))?;
    if sha256_hex(bytes) != sha {
        return Err(invalid(format!("{name} does not match its name")));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid(format!("{name} is not text")))?;
    let mut page = page::parse(text, "")?;
    if page.id.as_deref().is_none_or(|id| id.is_empty()) {
        return Err(invalid(format!("{name} names no page")));
    }
    let Some(serde_yaml::Value::Mapping(shared)) = page.other.shift_remove(SHARED_KEY) else {
        return Err(invalid(format!("{name} has no shared key")));
    };
    let Some(serde_yaml::Value::Mapping(author)) = shared.get("author") else {
        return Err(invalid(format!("{name} names no author")));
    };
    let author = Author {
        member: scalar(author, "member").ok_or_else(|| invalid(format!("{name}: no member")))?,
        kind: scalar(author, "kind")
            .as_deref()
            .and_then(AuthorKind::parse)
            .ok_or_else(|| invalid(format!("{name}: author kind is not person or ai")))?,
        agent: scalar(author, "agent"),
    };
    let merged = match shared.get("merged") {
        None => Vec::new(),
        Some(serde_yaml::Value::Sequence(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| invalid(format!("{name}: merged is not a list of versions")))?,
        Some(_) => return Err(invalid(format!("{name}: merged is not a list"))),
    };
    let meta = VersionMeta {
        parent: scalar(&shared, "parent"),
        merged,
        body_digest: scalar(&shared, "body_digest")
            .ok_or_else(|| invalid(format!("{name}: no body_digest")))?,
        author,
        ts: scalar(&shared, "ts").unwrap_or_default(),
        archived: match shared.get("archived") {
            None => false,
            Some(serde_yaml::Value::Bool(archived)) => *archived,
            Some(_) => return Err(invalid(format!("{name}: archived is not true or false"))),
        },
        locked: match shared.get("locked") {
            None => false,
            Some(serde_yaml::Value::Bool(locked)) => *locked,
            Some(_) => return Err(invalid(format!("{name}: locked is not true or false"))),
        },
    };
    if meta.body_digest != body_digest(&page.body) {
        return Err(invalid(format!("{name}: body does not match body_digest")));
    }
    Ok(PageVersion {
        sha: sha.to_string(),
        meta,
        page,
    })
}

/// The latest version of a page: the leaves of the parent graph (versions no
/// other version was written over), and the one every member picks among
/// them.
#[derive(Debug)]
pub struct Head<'a> {
    pub winner: &'a PageVersion,
    pub leaves: Vec<&'a PageVersion>,
}

fn ts_key(ts: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|at| at.with_timezone(&chrono::Utc))
}

/// The head of `versions` (one page's versions from every member). Two or
/// more leaves are edits made at the same time: the later `ts` wins, and the
/// larger sha when they tie, so every member picks the same one.
pub fn head(versions: &[PageVersion]) -> Option<Head<'_>> {
    let replaced: HashSet<&str> = versions
        .iter()
        .flat_map(|version| {
            version
                .meta
                .parent
                .iter()
                .chain(version.meta.merged.iter())
                .map(String::as_str)
        })
        .collect();
    let mut leaves: Vec<&PageVersion> = versions
        .iter()
        .filter(|version| !replaced.contains(version.sha.as_str()))
        .collect();
    if leaves.is_empty() {
        // Only a forged cycle has no leaf; still pick one the same way.
        leaves = versions.iter().collect();
    }
    leaves.sort_by(|a, b| {
        (ts_key(&a.meta.ts), &a.sha)
            .cmp(&(ts_key(&b.meta.ts), &b.sha))
            .reverse()
    });
    Some(Head {
        winner: leaves.first().copied()?,
        leaves,
    })
}

/// Every whole version in the folder, by page id.
#[derive(Debug, Default)]
pub struct FolderRead {
    pub pages: HashMap<String, Vec<PageVersion>>,
    /// Files that are not whole versions yet: a cloud placeholder
    /// (`.<name>.icloud`), a file still arriving, or a damaged one.
    pub skipped: u32,
}

/// `<name>` when `name` is `<name> (N)`: the second folder Google Drive
/// keeps when two computers made the same new folder before seeing each
/// other's. Its versions are read as the folder's own (their names are
/// their sha, so the two never hold different files under one name).
fn drive_copy_of(name: &str) -> Option<&str> {
    let (base, rest) = name.rsplit_once(" (")?;
    let number = rest.strip_suffix(')')?;
    (!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())).then_some(base)
}

fn sub_dirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
        .map(|(name, path)| (drive_copy_of(&name).unwrap_or(&name).to_string(), path))
        .filter(|(name, _)| is_safe_id(name))
        .collect();
    out.sort();
    out
}

/// Reads every member's versions. A file that cannot be read is skipped and
/// counted; reading goes on.
pub fn read_folder(folder: &Path) -> FolderRead {
    let mut read = FolderRead::default();
    let mut seen = HashSet::new();
    for (member, member_dir) in sub_dirs(&folder.join(MEMBERS_DIR)) {
        for (page_id, page_dir) in sub_dirs(&member_dir.join(PAGES_DIR)) {
            let Ok(entries) = std::fs::read_dir(&page_dir) else {
                read.skipped += 1;
                continue;
            };
            for entry in entries.flatten() {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if name.ends_with(".icloud") {
                    read.skipped += 1;
                    continue;
                }
                if version_sha(&name).is_none() {
                    // A temp file being written, or something not ours.
                    continue;
                }
                let version = std::fs::read(entry.path())
                    .map_err(WenlanError::from)
                    .and_then(|bytes| parse(&name, &bytes));
                match version {
                    Ok(version)
                        if version.page.id.as_deref() == Some(page_id.as_str())
                            && version.meta.author.member == member =>
                    {
                        if seen.insert(version.sha.clone()) {
                            read.pages.entry(page_id.clone()).or_default().push(version);
                        }
                    }
                    Ok(_) => {
                        log::warn!("[shared_space] {name} is filed under another page or member");
                        read.skipped += 1;
                    }
                    Err(error) => {
                        log::debug!("[shared_space] skipping {name}: {error}");
                        read.skipped += 1;
                    }
                }
            }
        }
    }
    read
}

/// Writes one version under `members/<member>/pages/<page_id>/`. Written to
/// a temp file in the same folder, then renamed, and never over a file that
/// is already there (a file with this name has these bytes). Returns the
/// version's sha and whether a file was written.
pub fn write_version(
    folder: &Path,
    member: &str,
    page_id: &str,
    text: &str,
) -> Result<(String, bool), WenlanError> {
    use std::io::Write;
    if !is_safe_id(member) || !is_safe_id(page_id) {
        return Err(invalid(format!(
            "{member}/{page_id} is not a folder name a version can go in"
        )));
    }
    let sha = sha256_hex(text.as_bytes());
    let dir = folder
        .join(MEMBERS_DIR)
        .join(member)
        .join(PAGES_DIR)
        .join(page_id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(version_file_name(&sha));
    if version_file_exists(&path) {
        return Ok((sha, false));
    }
    let mut tmp = tempfile::Builder::new()
        .prefix(".writing-")
        .tempfile_in(&dir)?;
    tmp.write_all(text.as_bytes())?;
    tmp.flush()?;
    match tmp.persist_noclobber(&path) {
        Ok(_) => Ok((sha, true)),
        Err(_) if path.exists() => Ok((sha, false)),
        Err(error) => Err(WenlanError::Io(error.error)),
    }
}
