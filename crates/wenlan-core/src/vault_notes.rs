// SPDX-License-Identifier: Apache-2.0
//! The person's own notes in a vault: whether AI may change each one, and the
//! changes an AI proposed that wait for the person to accept them.
//!
//! Nothing here writes into the person's notes except [`accept`], and only for
//! a proposal the person accepted, against the exact file it was made for.
//! The settings live in `config.json` and the proposals beside it,
//! never in the notes themselves.

use crate::config::config_root;
use crate::error::WenlanError;
use crate::sources::Source;
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt as _};
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use wenlan_types::sources::{AiChanges, AiChangesFrom, NoteAiChanges, NoteProposal};

/// A proposed file larger than this is refused.
pub const MAX_PROPOSAL_BYTES: usize = 2 * 1024 * 1024;

const PROPOSALS_DIR: &str = "note-proposals";

/// Checks a settings key: `""` for the vault, `dir/` for a folder,
/// `dir/note.md` for a file, always vault-relative with `/` separators.
pub fn normalize_key(key: &str) -> Result<String, WenlanError> {
    if key.is_empty() {
        return Ok(String::new());
    }
    let bad = || WenlanError::Validation("note_path_invalid".into());
    if key.starts_with('/') || key.contains('\\') || key.contains('\0') {
        return Err(bad());
    }
    let trimmed = key.strip_suffix('/').unwrap_or(key);
    if trimmed
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(bad());
    }
    Ok(key.to_string())
}

/// `file` as a path relative to `vault`, with `/` separators.
pub fn vault_relative(vault: &Path, file: &Path) -> Result<String, WenlanError> {
    let outside = || WenlanError::Validation("note_outside_vault".into());
    if !file.is_absolute()
        || file
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(outside());
    }
    let rest = file.strip_prefix(vault).map_err(|_| outside())?;
    let parts: Vec<String> = rest
        .components()
        .map(|c| match c {
            Component::Normal(part) => Ok(part.to_string_lossy().into_owned()),
            _ => Err(outside()),
        })
        .collect::<Result<_, _>>()?;
    if parts.is_empty() {
        return Err(outside());
    }
    Ok(parts.join("/"))
}

fn is_pdf(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".pdf")
}

/// The setting that applies to `path` (vault-relative; `""` is the vault, a
/// folder ends with `/`): the file's own, else the nearest folder's, else the
/// vault's, else [`AiChanges::Never`]. A PDF is always `Never`.
pub fn effective(settings: Option<&BTreeMap<String, AiChanges>>, path: &str) -> NoteAiChanges {
    let found = |value, from| NoteAiChanges {
        path: path.to_string(),
        value,
        from,
    };
    if is_pdf(path) {
        return found(AiChanges::Never, AiChangesFrom::Pdf);
    }
    let Some(settings) = settings else {
        return found(AiChanges::Never, AiChangesFrom::Default);
    };
    if !path.is_empty() && !path.ends_with('/') {
        if let Some(value) = settings.get(path) {
            return found(*value, AiChangesFrom::File);
        }
    }
    let folder = settings
        .iter()
        .filter(|(key, _)| key.ends_with('/') && path.starts_with(key.as_str()))
        .max_by_key(|(key, _)| key.len());
    if let Some((_, value)) = folder {
        return found(*value, AiChangesFrom::Folder);
    }
    match settings.get("") {
        Some(value) => found(*value, AiChangesFrom::Vault),
        None => found(AiChanges::Never, AiChangesFrom::Default),
    }
}

/// The file a proposal for `path` would change, after checking it is one of
/// the person's own Markdown notes: inside the vault without passing through
/// a link, not hidden (`.obsidian`, `.trash`, `.git`), and not one of
/// Wenlan's own pages.
pub fn note_file(source: &Source, pages_folder: &Path, path: &str) -> Result<PathBuf, WenlanError> {
    let key = normalize_key(path)?;
    if key.is_empty() || key.ends_with('/') {
        return Err(WenlanError::Validation("note_path_invalid".into()));
    }
    let lower = key.to_ascii_lowercase();
    if !(lower.ends_with(".md") || lower.ends_with(".markdown")) {
        return Err(WenlanError::Validation("note_not_markdown".into()));
    }
    if key.split('/').any(|part| part.starts_with('.')) {
        return Err(WenlanError::Validation("note_path_invalid".into()));
    }
    let file = source.path.join(&key);
    if file.starts_with(pages_folder) {
        return Err(WenlanError::Validation("note_is_wenlan_page".into()));
    }
    let meta = std::fs::symlink_metadata(&file).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => WenlanError::NotFound(format!("note {key}")),
        _ => WenlanError::Io(e),
    })?;
    if !meta.file_type().is_file() {
        return Err(WenlanError::Validation("note_path_invalid".into()));
    }
    let vault = source.path.canonicalize()?;
    let parent = file
        .parent()
        .ok_or_else(|| WenlanError::Validation("note_path_invalid".into()))?
        .canonicalize()?;
    if !parent.starts_with(&vault) {
        return Err(WenlanError::Validation("note_outside_vault".into()));
    }
    Ok(file)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn read_nofollow(directory: &Dir, name: &str) -> std::io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let mut file = directory.open_with(name, &options)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "note_path_invalid",
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn open_parent(file: &Path) -> Result<(Dir, String), WenlanError> {
    let parent = file
        .parent()
        .ok_or_else(|| WenlanError::Validation("note_path_invalid".into()))?;
    let name = file
        .file_name()
        .ok_or_else(|| WenlanError::Validation("note_path_invalid".into()))?
        .to_string_lossy()
        .into_owned();
    let directory = Dir::open_ambient_dir(parent, cap_std::ambient_authority())?;
    Ok((directory, name))
}

/// The one open proposal per note has a fixed id, so a newer proposal for the
/// same note replaces the older one.
pub fn proposal_id(source_id: &str, path: &str) -> String {
    let digest = sha256_hex(format!("{source_id}\0{path}").as_bytes());
    format!("np_{}", &digest[..16])
}

fn valid_id(id: &str) -> bool {
    id.len() == 19 && id.starts_with("np_") && id[3..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn proposals_dir() -> PathBuf {
    config_root().join(PROPOSALS_DIR)
}

fn proposal_file(id: &str) -> Result<PathBuf, WenlanError> {
    if !valid_id(id) {
        return Err(WenlanError::NotFound(format!("proposal {id}")));
    }
    Ok(proposals_dir().join(format!("{id}.json")))
}

fn save_proposal(proposal: &NoteProposal) -> Result<(), WenlanError> {
    let dir = proposals_dir();
    std::fs::create_dir_all(&dir)?;
    let path = proposal_file(&proposal.id)?;
    let temp = dir.join(format!(".{}.tmp", proposal.id));
    let bytes = serde_json::to_vec_pretty(proposal)?;
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, &path)?;
    Ok(())
}

/// Every open proposal, oldest first, optionally for one source.
pub fn list_proposals(source_id: Option<&str>) -> Vec<NoteProposal> {
    let Ok(entries) = std::fs::read_dir(proposals_dir()) else {
        return Vec::new();
    };
    let mut found: Vec<NoteProposal> = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .is_some_and(valid_id)
        })
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<NoteProposal>(&bytes).ok())
        .filter(|p| source_id.is_none_or(|id| p.source_id == id))
        .collect();
    found.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    found
}

pub fn load_proposal(id: &str) -> Result<NoteProposal, WenlanError> {
    let path = proposal_file(id)?;
    let bytes = std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => WenlanError::NotFound(format!("proposal {id}")),
        _ => WenlanError::Io(e),
    })?;
    serde_json::from_slice(&bytes).map_err(WenlanError::Json)
}

/// Removes a proposal. `false` when there was none.
pub fn remove_proposal(id: &str) -> Result<bool, WenlanError> {
    match std::fs::remove_file(proposal_file(id)?) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(WenlanError::Io(e)),
    }
}

/// Removes every proposal of `source_id` whose note no longer allows one,
/// after the person changed a setting. Returns how many were removed.
pub fn drop_disallowed(
    source_id: &str,
    settings: Option<&BTreeMap<String, AiChanges>>,
) -> Result<usize, WenlanError> {
    let mut removed = 0;
    for proposal in list_proposals(Some(source_id)) {
        if effective(settings, &proposal.path).value != AiChanges::AskFirst
            && remove_proposal(&proposal.id)?
        {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Records an AI's proposed change to the note at `path`. Refused unless the
/// note is set to [`AiChanges::AskFirst`]. Writes nothing to the note.
pub fn propose(
    source: &Source,
    settings: Option<&BTreeMap<String, AiChanges>>,
    pages_folder: &Path,
    path: &str,
    content: String,
    reason: String,
    agent: &str,
) -> Result<NoteProposal, WenlanError> {
    let file = note_file(source, pages_folder, path)?;
    if effective(settings, path).value != AiChanges::AskFirst {
        return Err(WenlanError::Validation("note_ai_changes_never".into()));
    }
    if content.len() > MAX_PROPOSAL_BYTES {
        return Err(WenlanError::Validation("note_proposal_too_large".into()));
    }
    let (directory, name) = open_parent(&file)?;
    let current = read_nofollow(&directory, &name)?;
    if current == content.as_bytes() {
        return Err(WenlanError::Validation("note_proposal_unchanged".into()));
    }
    let proposal = NoteProposal {
        id: proposal_id(&source.id, path),
        source_id: source.id.clone(),
        path: path.to_string(),
        content,
        reason: reason.trim().to_string(),
        agent: agent.to_string(),
        created_at: chrono::Utc::now().timestamp(),
        base_hash: sha256_hex(&current),
    };
    save_proposal(&proposal)?;
    Ok(proposal)
}

/// The note's current text, for showing a proposal next to what it changes.
pub fn current_text(
    source: &Source,
    pages_folder: &Path,
    path: &str,
) -> Result<String, WenlanError> {
    let file = note_file(source, pages_folder, path)?;
    let (directory, name) = open_parent(&file)?;
    Ok(String::from_utf8_lossy(&read_nofollow(&directory, &name)?).into_owned())
}

/// Writes an accepted proposal into the note and removes the proposal.
///
/// Refuses, and drops the proposal, when the note changed after the proposal
/// was made: the person's newer text is never overwritten. The write goes to
/// a private file beside the note first and then replaces it in one step, so
/// a crash leaves either the old note or the new one, never half of each.
pub fn accept(source: &Source, pages_folder: &Path, id: &str) -> Result<PathBuf, WenlanError> {
    let proposal = load_proposal(id)?;
    if proposal.source_id != source.id {
        return Err(WenlanError::NotFound(format!("proposal {id}")));
    }
    let file = note_file(source, pages_folder, &proposal.path)?;
    let (directory, name) = open_parent(&file)?;
    let current = read_nofollow(&directory, &name)?;
    if sha256_hex(&current) != proposal.base_hash {
        remove_proposal(id)?;
        return Err(WenlanError::Conflict("note_changed_since_proposal".into()));
    }
    let temporary = format!(".{name}.wenlan-{id}.tmp");
    match directory.remove_file(&temporary) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(WenlanError::Io(e)),
    }
    crate::export::knowledge::write_page_atomically_nofollow(
        &directory,
        &name,
        &temporary,
        proposal.content.as_bytes(),
    )?;
    remove_proposal(id)?;
    Ok(file)
}

#[cfg(test)]
#[path = "vault_notes_test.rs"]
mod tests;
