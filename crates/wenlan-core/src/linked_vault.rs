// SPDX-License-Identifier: Apache-2.0
//! An Obsidian vault linked into the Wiki: every note in the vault is also a
//! page, kept in a folder of the pages folder, and the two stay the same.
//!
//! The vault is the person's. Wenlan adds nothing to it: no id line, no
//! state file, no index. The only changes it makes are the person's own
//! (an edit, a new page, a rename or a delete made in Wenlan), and a delete
//! moves the note to the vault's `.trash/`, where Obsidian puts it too.
//!
//! How it works: the folder `<pages folder>/<name>/` holds one file per
//! note at the same relative path (the mirror). The page watcher already
//! turns the files of the pages folder into pages and reads their edits
//! back, so this module only copies changes between the vault and the
//! mirror. Which side changed is told from the digests recorded after the
//! last copy, kept in Wenlan's data folder.
//!
//! ```text
//! vault/a.md  <-- copy -->  pages/<name>/a.md  <-- page watcher -->  page
//! ```
//!
//! - A note changed in the vault is copied to the mirror. The mirror keeps
//!   the `origin_id` line the page watcher wrote into it.
//! - A mirror file changed by Wenlan is copied to the vault without that line.
//! - Both changed since the last copy: the vault wins. The Wenlan edit stays
//!   in the page's history.
//! - A note renamed or moved on one side is renamed on the other, so the
//!   page keeps its history. It is found by its text, which a pure rename
//!   leaves unchanged.
//! - A note deleted in the vault deletes its mirror file, which archives the
//!   page (restorable). A page deleted in Wenlan moves the note to `.trash/`.
//! - Changes that differ only in blank lines between the frontmatter and the
//!   text, or at the end, are not copied to the vault.
//! - Many notes gone at once (a vault on a disconnected drive, a sync app
//!   halfway through) deletes nothing until most are back.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::error::WenlanError;

/// A vault note is copied only once it has not changed for this long, so a
/// note is never read halfway through an editor's save.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(2);

/// More notes than this gone from one side in one pass, and at least
/// [`MASS_GONE_SHARE`] of the recorded ones, deletes nothing that pass.
const MASS_GONE_FLOOR: usize = 20;
const MASS_GONE_SHARE: f64 = 0.25;

/// The most Markdown files a vault may hold. Past it the pass stops.
const FILE_LIMIT: usize = 50_000;

/// How deep folders are followed.
const DEPTH_LIMIT: usize = 32;

/// Obsidian's own trash folder, at the vault root.
const TRASH: &str = ".trash";

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// A vault linked into the Wiki, as `config.json` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedVault {
    /// The vault's folder.
    pub path: PathBuf,
    /// The folder of the pages folder its notes are kept in, one name.
    pub folder: String,
}

/// What one pass copied.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BridgeStats {
    pub to_wiki: usize,
    pub to_vault: usize,
    pub renamed: usize,
    pub removed_from_wiki: usize,
    pub moved_to_trash: usize,
    /// Notes both sides changed; the vault's version was kept.
    pub vault_won: usize,
    /// Set when so many notes were gone at once that nothing was deleted.
    pub held_gone: Option<usize>,
    pub errors: usize,
}

/// What the last copy left on both sides of one note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Recorded {
    /// SHA-256 of the vault file's bytes.
    vault: String,
    /// SHA-256 of the mirror file's bytes without its `origin_id` line.
    wiki: String,
    /// The page id the mirror file carried, which a rename in Wenlan keeps
    /// even when it also changes the file's `title:` line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct BridgeState {
    #[serde(default)]
    notes: BTreeMap<String, Recorded>,
}

/// The file the copy state of the vault at `vault` is kept in, under
/// `state_dir` (Wenlan's data folder, never the vault).
pub fn state_file(state_dir: &Path, vault: &Path) -> PathBuf {
    let digest = hex(&Sha256::digest(vault.to_string_lossy().as_bytes()));
    state_dir.join(format!("{}.json", &digest[..16]))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Checks that `vault` can be linked as the Wiki folder `folder` of the
/// pages folder `pages`. Errors are `Validation` codes the app turns into
/// words.
pub fn check_link(
    vault: &Path,
    pages: &Path,
    folder: &str,
    linked: &[LinkedVault],
) -> Result<(), WenlanError> {
    let invalid = |code: &str| WenlanError::Validation(code.to_string());
    if !vault.is_absolute() {
        return Err(invalid("linked_vault_not_absolute"));
    }
    match std::fs::symlink_metadata(vault) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
        _ => return Err(invalid("linked_vault_not_a_folder")),
    }
    let real_vault = std::fs::canonicalize(vault)?;
    let real_pages = std::fs::canonicalize(pages).unwrap_or_else(|_| pages.to_path_buf());
    if real_vault.starts_with(&real_pages) || real_pages.starts_with(&real_vault) {
        return Err(invalid("linked_vault_overlaps_pages_folder"));
    }
    for other in linked {
        let other_real = std::fs::canonicalize(&other.path).unwrap_or_else(|_| other.path.clone());
        if other_real.starts_with(&real_vault) || real_vault.starts_with(&other_real) {
            return Err(invalid("linked_vault_already_linked"));
        }
        if other.folder.eq_ignore_ascii_case(folder) {
            return Err(invalid("linked_vault_folder_taken"));
        }
    }
    if !valid_folder_name(folder) {
        return Err(invalid("linked_vault_folder_invalid"));
    }
    match std::fs::symlink_metadata(pages.join(folder)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
            if std::fs::read_dir(pages.join(folder))?.next().is_none() {
                Ok(())
            } else {
                Err(invalid("linked_vault_folder_taken"))
            }
        }
        Ok(_) => Err(invalid("linked_vault_folder_taken")),
        Err(error) => Err(error.into()),
    }
}

/// A Wiki folder name for the vault at `vault`: its own folder name, with
/// a number added when the pages folder already has one of that name.
pub fn folder_name_for(vault: &Path, pages: &Path, linked: &[LinkedVault]) -> String {
    let base: String = vault
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '\0') {
                '-'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
        .to_string();
    let base = if base.is_empty() {
        "Obsidian".to_string()
    } else {
        base
    };
    let free = |name: &str| {
        !linked.iter().any(|l| l.folder.eq_ignore_ascii_case(name))
            && match std::fs::read_dir(pages.join(name)) {
                Ok(mut entries) => entries.next().is_none(),
                Err(error) => error.kind() == std::io::ErrorKind::NotFound,
            }
    };
    if free(&base) {
        return base;
    }
    (2..1000)
        .map(|n| format!("{base} {n}"))
        .find(|name| free(name))
        .unwrap_or_else(|| format!("{base} {}", std::process::id()))
}

fn valid_folder_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':', '\0'])
        && name.trim() == name
}

/// One copy pass between `link.path` and `<pages>/<link.folder>`, with its
/// state in `state_dir`. Does nothing when either side cannot be read.
pub fn bridge_pass(
    link: &LinkedVault,
    pages: &Path,
    state_dir: &Path,
) -> Result<BridgeStats, WenlanError> {
    bridge_pass_at(link, pages, state_dir, std::time::SystemTime::now())
}

/// [`bridge_pass`] as if it ran at `now`.
pub(crate) fn bridge_pass_at(
    link: &LinkedVault,
    pages: &Path,
    state_dir: &Path,
    now: std::time::SystemTime,
) -> Result<BridgeStats, WenlanError> {
    let mut stats = BridgeStats::default();
    let vault = link.path.as_path();
    if !valid_folder_name(&link.folder) {
        return Err(WenlanError::Validation(
            "linked_vault_folder_invalid".into(),
        ));
    }
    // A vault on a drive that is not there: nothing to copy, and nothing
    // may be read as deleted.
    match std::fs::symlink_metadata(vault) {
        Ok(meta) if meta.is_dir() => {}
        _ => return Err(WenlanError::Validation("linked_vault_unavailable".into())),
    }
    if !pages.is_dir() {
        return Err(WenlanError::Validation(
            "linked_vault_pages_unavailable".into(),
        ));
    }
    let wiki = pages.join(&link.folder);
    std::fs::create_dir_all(&wiki)?;

    let state_path = state_file(state_dir, vault);
    let mut state = read_state(&state_path)?;

    let vault_files = markdown_files(vault)?;
    let wiki_files = markdown_files(&wiki)?;

    // Read both sides once.
    let mut vault_seen: HashMap<String, Seen> = HashMap::new();
    for rel in &vault_files {
        match read_seen(&vault.join(rel)) {
            Ok(Some(seen)) => {
                vault_seen.insert(rel.clone(), seen);
            }
            Ok(None) => {}
            Err(error) => {
                stats.errors += 1;
                log::warn!("[linked-vault] could not read {rel}: {error}");
            }
        }
    }
    let mut wiki_seen: HashMap<String, Seen> = HashMap::new();
    for rel in &wiki_files {
        match read_seen(&wiki.join(rel)) {
            Ok(Some(seen)) => {
                wiki_seen.insert(rel.clone(), seen);
            }
            Ok(None) => {}
            Err(error) => {
                stats.errors += 1;
                log::warn!("[linked-vault] could not read wiki copy {rel}: {error}");
            }
        }
    }
    let settled = |seen: &Seen| {
        now.duration_since(seen.modified)
            .is_ok_and(|age| age >= SETTLE)
    };

    // Renames first, so a renamed note is not read as one deleted and one new.
    follow_renames(
        vault,
        &wiki,
        &mut state,
        &mut vault_seen,
        &mut wiki_seen,
        &mut stats,
    );

    // Deletions are held when too many notes are gone from one side at once.
    let recorded = state.notes.len();
    let gone_from = |seen: &HashMap<String, Seen>| {
        state
            .notes
            .keys()
            .filter(|rel| !seen.contains_key(*rel))
            .count()
    };
    let too_many = |gone: usize| {
        gone > MASS_GONE_FLOOR && (gone as f64) >= (recorded as f64) * MASS_GONE_SHARE
    };
    let vault_gone = gone_from(&vault_seen);
    let wiki_gone = gone_from(&wiki_seen);
    let hold_deletes = too_many(vault_gone) || too_many(wiki_gone);
    if hold_deletes {
        stats.held_gone = Some(vault_gone.max(wiki_gone));
        log::warn!(
            "[linked-vault] {} notes of {} are gone from one side at once; deleting none of them",
            vault_gone.max(wiki_gone),
            vault.display()
        );
    }

    let all: BTreeSet<String> = vault_seen
        .keys()
        .chain(wiki_seen.keys())
        .chain(state.notes.keys())
        .cloned()
        .collect();
    for rel in all {
        let v = vault_seen.get(&rel);
        let w = wiki_seen.get(&rel);
        let result = sync_note(
            vault,
            &wiki,
            &rel,
            v,
            w,
            &mut state,
            hold_deletes,
            &settled,
            &mut stats,
        );
        if let Err(error) = result {
            stats.errors += 1;
            log::warn!("[linked-vault] {rel}: {error}");
        }
    }
    write_state(&state_path, &state)?;
    Ok(stats)
}

/// One side's file as it was read.
#[derive(Debug, Clone)]
struct Seen {
    bytes: Vec<u8>,
    modified: std::time::SystemTime,
}

impl Seen {
    fn vault_digest(&self) -> String {
        digest(&self.bytes)
    }

    /// The mirror's text without Wenlan's `origin_id` line.
    fn note_text(&self) -> Option<String> {
        String::from_utf8(self.bytes.clone())
            .ok()
            .map(|raw| without_id_line(&raw))
    }

    fn page_id(&self) -> Option<String> {
        std::str::from_utf8(&self.bytes)
            .ok()
            .and_then(crate::export::knowledge::origin_id_line)
    }

    fn wiki_digest(&self) -> String {
        match self.note_text() {
            Some(text) => digest(text.as_bytes()),
            None => digest(&self.bytes),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn sync_note(
    vault: &Path,
    wiki: &Path,
    rel: &str,
    v: Option<&Seen>,
    w: Option<&Seen>,
    state: &mut BridgeState,
    hold_deletes: bool,
    settled: &dyn Fn(&Seen) -> bool,
    stats: &mut BridgeStats,
) -> Result<(), WenlanError> {
    let recorded = state.notes.get(rel).cloned();
    match (v, w, recorded) {
        (None, None, Some(_)) => {
            state.notes.remove(rel);
        }
        (None, None, None) => {}
        // New in the vault.
        (Some(v), None, None) => {
            if !settled(v) {
                return Ok(());
            }
            write_wiki_copy(wiki, rel, &v.bytes, None)?;
            record(state, rel, v, &v.bytes);
            stats.to_wiki += 1;
        }
        // New in Wenlan (a page made in the folder).
        (None, Some(w), None) => {
            let Some(text) = w.note_text() else {
                return Ok(());
            };
            write_atomic(&vault.join(rel), text.as_bytes(), None)?;
            record_bytes(state, rel, text.as_bytes(), &w.bytes);
            stats.to_vault += 1;
        }
        // On both sides, never copied (a link made again after its state
        // was lost): the vault's text is the one kept.
        (Some(v), Some(w), None) => {
            if !settled(v) {
                return Ok(());
            }
            if !same_text(w.note_text().as_deref(), &v.bytes) {
                write_wiki_copy(wiki, rel, &v.bytes, Some(w))?;
                stats.to_wiki += 1;
            }
            let wiki_bytes = std::fs::read(wiki.join(rel))?;
            record_bytes(state, rel, &v.bytes, &wiki_bytes);
        }
        (Some(v), Some(w), Some(rec)) => {
            let vault_changed = v.vault_digest() != rec.vault;
            let wiki_changed = w.wiki_digest() != rec.wiki;
            match (vault_changed, wiki_changed) {
                (false, false) => {
                    // The page watcher has since made the copy a page.
                    if rec.id.is_none() {
                        if let (Some(id), Some(entry)) = (w.page_id(), state.notes.get_mut(rel)) {
                            entry.id = Some(id);
                        }
                    }
                }
                (true, _) => {
                    if !settled(v) {
                        return Ok(());
                    }
                    if wiki_changed && !same_text(w.note_text().as_deref(), &v.bytes) {
                        stats.vault_won += 1;
                        log::info!(
                            "[linked-vault] {rel} changed in the vault and in Wenlan; kept the vault's"
                        );
                    }
                    if !same_text(w.note_text().as_deref(), &v.bytes) {
                        write_wiki_copy(wiki, rel, &v.bytes, Some(w))?;
                        stats.to_wiki += 1;
                    }
                    let wiki_bytes = std::fs::read(wiki.join(rel))?;
                    record_bytes(state, rel, &v.bytes, &wiki_bytes);
                }
                (false, true) => {
                    let Some(text) = w.note_text() else {
                        return Ok(());
                    };
                    if same_text(Some(&text), &v.bytes) {
                        // Only blank lines moved: the vault stays as it is.
                        record_bytes(state, rel, &v.bytes, &w.bytes);
                        return Ok(());
                    }
                    write_atomic(&vault.join(rel), text.as_bytes(), Some(&v.bytes))?;
                    record_bytes(state, rel, text.as_bytes(), &w.bytes);
                    stats.to_vault += 1;
                }
            }
        }
        // Gone from the vault.
        (None, Some(w), Some(rec)) => {
            if w.wiki_digest() != rec.wiki {
                // Edited in Wenlan since: the edit is kept, the note comes back.
                let Some(text) = w.note_text() else {
                    return Ok(());
                };
                write_atomic(&vault.join(rel), text.as_bytes(), None)?;
                record_bytes(state, rel, text.as_bytes(), &w.bytes);
                stats.to_vault += 1;
                return Ok(());
            }
            if hold_deletes {
                return Ok(());
            }
            remove_if_unchanged(&wiki.join(rel), &w.bytes)?;
            state.notes.remove(rel);
            stats.removed_from_wiki += 1;
        }
        // Gone from Wenlan (the page was deleted or archived).
        (Some(v), None, Some(rec)) => {
            if v.vault_digest() != rec.vault {
                // Changed in the vault since: it comes back as a page.
                if !settled(v) {
                    return Ok(());
                }
                write_wiki_copy(wiki, rel, &v.bytes, None)?;
                record(state, rel, v, &v.bytes);
                stats.to_wiki += 1;
                return Ok(());
            }
            if hold_deletes {
                return Ok(());
            }
            move_to_trash(vault, rel, &v.bytes)?;
            state.notes.remove(rel);
            stats.moved_to_trash += 1;
        }
    }
    Ok(())
}

/// Renames on one side carried to the other: a recorded note gone from one
/// side while the other side still holds it unchanged, and a new file on
/// that side with exactly its text.
fn follow_renames(
    vault: &Path,
    wiki: &Path,
    state: &mut BridgeState,
    vault_seen: &mut HashMap<String, Seen>,
    wiki_seen: &mut HashMap<String, Seen>,
    stats: &mut BridgeStats,
) {
    // Renamed in the vault: rename the Wiki copy, so the page follows it.
    let gone: Vec<(String, Recorded)> = state
        .notes
        .iter()
        .filter(|(rel, rec)| {
            !vault_seen.contains_key(*rel)
                && wiki_seen
                    .get(*rel)
                    .is_some_and(|w| w.wiki_digest() == rec.wiki)
        })
        .map(|(rel, rec)| (rel.clone(), rec.clone()))
        .collect();
    for (old, rec) in gone {
        let found: Vec<&String> = vault_seen
            .iter()
            .filter(|(rel, seen)| {
                !state.notes.contains_key(*rel)
                    && !wiki_seen.contains_key(*rel)
                    && seen.vault_digest() == rec.vault
            })
            .map(|(rel, _)| rel)
            .collect();
        let [new] = found.as_slice() else {
            continue;
        };
        let new = (*new).clone();
        match rename_within(wiki, &old, &new) {
            Ok(()) => {
                if let Some(seen) = wiki_seen.remove(&old) {
                    wiki_seen.insert(new.clone(), seen);
                }
                state.notes.remove(&old);
                state.notes.insert(new.clone(), rec);
                stats.renamed += 1;
                log::info!("[linked-vault] {old} was renamed in the vault to {new}");
            }
            Err(error) => {
                stats.errors += 1;
                log::warn!("[linked-vault] could not follow {old} -> {new}: {error}");
            }
        }
    }
    // Renamed in Wenlan: rename the note in the vault.
    let gone: Vec<(String, Recorded)> = state
        .notes
        .iter()
        .filter(|(rel, rec)| {
            !wiki_seen.contains_key(*rel)
                && vault_seen
                    .get(*rel)
                    .is_some_and(|v| v.vault_digest() == rec.vault)
        })
        .map(|(rel, rec)| (rel.clone(), rec.clone()))
        .collect();
    for (old, rec) in gone {
        let found: Vec<&String> = wiki_seen
            .iter()
            .filter(|(rel, seen)| {
                !state.notes.contains_key(*rel)
                    && !vault_seen.contains_key(*rel)
                    && match (&rec.id, seen.page_id()) {
                        (Some(old), Some(new)) => *old == new,
                        _ => seen.wiki_digest() == rec.wiki,
                    }
            })
            .map(|(rel, _)| rel)
            .collect();
        let [new] = found.as_slice() else {
            continue;
        };
        let new = (*new).clone();
        match rename_within(vault, &old, &new) {
            Ok(()) => {
                if let Some(seen) = vault_seen.remove(&old) {
                    vault_seen.insert(new.clone(), seen);
                }
                state.notes.remove(&old);
                state.notes.insert(new.clone(), rec);
                stats.renamed += 1;
                log::info!("[linked-vault] {old} was renamed in Wenlan to {new}");
            }
            Err(error) => {
                stats.errors += 1;
                log::warn!("[linked-vault] could not rename {old} -> {new} in the vault: {error}");
            }
        }
    }
}

fn record(state: &mut BridgeState, rel: &str, v: &Seen, wiki_bytes: &[u8]) {
    record_bytes(state, rel, &v.bytes, wiki_bytes);
}

fn record_bytes(state: &mut BridgeState, rel: &str, vault_bytes: &[u8], wiki_bytes: &[u8]) {
    let (wiki, id) = match std::str::from_utf8(wiki_bytes) {
        Ok(raw) => (
            digest(without_id_line(raw).as_bytes()),
            crate::export::knowledge::origin_id_line(raw),
        ),
        Err(_) => (digest(wiki_bytes), None),
    };
    state.notes.insert(
        rel.to_string(),
        Recorded {
            vault: digest(vault_bytes),
            wiki,
            id,
        },
    );
}

/// Whether the mirror's text `wiki` (without its id line) says the same as
/// the vault's bytes, ignoring blank lines between the frontmatter and the
/// text, line endings and trailing whitespace.
fn same_text(wiki: Option<&str>, vault: &[u8]) -> bool {
    let (Some(wiki), Ok(vault)) = (wiki, std::str::from_utf8(vault)) else {
        return false;
    };
    normalized(wiki) == normalized(vault)
}

fn normalized(text: &str) -> String {
    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n");
    let (front, body) = match crate::sources::obsidian::split_frontmatter_untrimmed(&text) {
        Some((yaml, body)) => (Some(yaml.trim_matches('\n').to_string()), body),
        None => (None, text.as_str()),
    };
    let body = body
        .trim_start_matches('\n')
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    let body = body.trim_end();
    match front {
        Some(front) if !front.trim().is_empty() => format!("---\n{front}\n---\n{body}"),
        _ => body.to_string(),
    }
}

/// `raw` without the `origin_id` line the page watcher writes into a note it
/// makes a page; the frontmatter goes too when that line was all it held.
pub fn without_id_line(raw: &str) -> String {
    let had_bom = raw.starts_with('\u{feff}');
    let text = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let crlf = text.starts_with("---\r\n");
    let open = if crlf { "---\r\n" } else { "---\n" };
    let eol = if crlf { "\r\n" } else { "\n" };
    let Some(after_open) = text.strip_prefix(open) else {
        return raw.to_string();
    };
    // The closing line: `---` alone on a line.
    let close = format!("{eol}---");
    let (yaml, rest) = if let Some(rest) = after_open.strip_prefix("---") {
        ("", rest)
    } else {
        match after_open.find(&close) {
            Some(pos) => (&after_open[..pos], &after_open[pos + close.len()..]),
            None => return raw.to_string(),
        }
    };
    let lines: Vec<&str> = yaml.split(eol).collect();
    let kept: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| !line.starts_with("origin_id:"))
        .collect();
    if kept.len() == lines.len() {
        return raw.to_string();
    }
    let prefix = if had_bom { "\u{feff}" } else { "" };
    if kept.iter().all(|line| line.trim().is_empty()) {
        // Only the id was there: the note had no frontmatter of its own.
        let body = rest.strip_prefix(eol).unwrap_or(rest);
        let body = body.strip_prefix(eol).unwrap_or(body);
        return format!("{prefix}{body}");
    }
    format!("{prefix}{open}{}{close}{rest}", kept.join(eol))
}

/// `vault_text` with the id line of the copy it replaces, so the page keeps
/// its identity.
fn with_id_line_of(vault_text: &str, existing: Option<&Seen>) -> String {
    let id = existing
        .and_then(|seen| std::str::from_utf8(&seen.bytes).ok())
        .and_then(crate::export::knowledge::origin_id_line);
    let Some(id) = id else {
        return vault_text.to_string();
    };
    let text = vault_text.strip_prefix('\u{feff}').unwrap_or(vault_text);
    for open in ["---\r\n", "---\n"] {
        if text.starts_with(open)
            && crate::sources::obsidian::split_frontmatter_untrimmed(text).is_some()
        {
            let end = &open[3..];
            return format!("{open}origin_id: {id}{end}{}", &text[open.len()..]);
        }
    }
    format!("---\norigin_id: {id}\n---\n\n{text}")
}

fn write_wiki_copy(
    wiki: &Path,
    rel: &str,
    vault_bytes: &[u8],
    existing: Option<&Seen>,
) -> Result<(), WenlanError> {
    let bytes = match std::str::from_utf8(vault_bytes) {
        Ok(text) => with_id_line_of(text, existing).into_bytes(),
        Err(_) => vault_bytes.to_vec(),
    };
    write_atomic(
        &wiki.join(rel),
        &bytes,
        existing.map(|seen| seen.bytes.as_slice()),
    )
}

/// Writes `bytes` at `path` through a temporary file in the same folder and
/// a rename, so no reader sees half a file. With `expected`, the file must
/// still hold exactly those bytes right before, so a save that landed after
/// the pass read it is never overwritten.
fn write_atomic(path: &Path, bytes: &[u8], expected: Option<&[u8]>) -> Result<(), WenlanError> {
    let parent = path
        .parent()
        .ok_or_else(|| WenlanError::Validation("linked_vault_path_invalid".into()))?;
    std::fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| WenlanError::Validation("linked_vault_path_invalid".into()))?
        .to_string_lossy()
        .into_owned();
    let tmp = parent.join(format!(
        ".{name}.wenlan-{}-{}.tmp",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, bytes)?;
    let current = match std::fs::read(path) {
        Ok(current) => Some(current),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(error.into());
        }
    };
    let unchanged = match (expected, &current) {
        (Some(expected), Some(current)) => expected == current.as_slice(),
        (None, None) => true,
        // Expected none but one appeared, or expected one and it is gone.
        _ => false,
    };
    if !unchanged {
        let _ = std::fs::remove_file(&tmp);
        return Err(WenlanError::Conflict(
            "linked_vault_changed_meanwhile".into(),
        ));
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error.into());
    }
    Ok(())
}

fn remove_if_unchanged(path: &Path, expected: &[u8]) -> Result<(), WenlanError> {
    match std::fs::read(path) {
        Ok(current) if current == expected => Ok(std::fs::remove_file(path)?),
        Ok(_) => Err(WenlanError::Conflict(
            "linked_vault_changed_meanwhile".into(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Moves the vault note at `rel` to the vault's `.trash/`, as Obsidian does
/// with a deleted note. A name already in the trash gets a number.
fn move_to_trash(vault: &Path, rel: &str, expected: &[u8]) -> Result<(), WenlanError> {
    let from = vault.join(rel);
    match std::fs::read(&from) {
        Ok(current) if current == expected => {}
        Ok(_) => {
            return Err(WenlanError::Conflict(
                "linked_vault_changed_meanwhile".into(),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let trash = vault.join(TRASH);
    std::fs::create_dir_all(&trash)?;
    let name = Path::new(rel)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "note.md".into());
    let stem = name.strip_suffix(".md").unwrap_or(&name).to_string();
    let mut to = trash.join(&name);
    let mut n = 1;
    while to.exists() {
        to = trash.join(format!("{stem} {n}.md"));
        n += 1;
    }
    std::fs::rename(&from, &to)?;
    Ok(())
}

fn rename_within(root: &Path, old: &str, new: &str) -> Result<(), WenlanError> {
    let to = root.join(new);
    if to.exists() {
        return Err(WenlanError::Conflict(
            "linked_vault_rename_target_exists".into(),
        ));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(root.join(old), to)?;
    Ok(())
}

fn read_seen(path: &Path) -> Result<Option<Seen>, WenlanError> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !meta.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(path)?;
    Ok(Some(Seen {
        bytes,
        modified: meta.modified()?,
    }))
}

/// Every `.md` file under `root`, relative with `/`, leaving out folders
/// whose name starts with a dot (`.obsidian/`, `.trash/`, `.git/`), temporary
/// files and symbolic links.
fn markdown_files(root: &Path) -> Result<Vec<String>, WenlanError> {
    fn walk(
        dir: &Path,
        prefix: &str,
        depth: usize,
        out: &mut Vec<String>,
    ) -> Result<(), WenlanError> {
        if depth > DEPTH_LIMIT {
            return Ok(());
        }
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if file_type.is_dir() {
                walk(&entry.path(), &rel, depth + 1, out)?;
            } else if file_type.is_file() && name.to_lowercase().ends_with(".md") {
                out.push(rel);
                if out.len() > FILE_LIMIT {
                    return Err(WenlanError::Conflict("linked_vault_too_many_files".into()));
                }
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, "", 0, &mut out)?;
    Ok(out)
}

fn read_state(path: &Path) -> Result<BridgeState, WenlanError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| WenlanError::Validation(format!("linked_vault_state_invalid: {e}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BridgeState::default()),
        Err(error) => Err(error.into()),
    }
}

fn write_state(path: &Path, state: &BridgeState) -> Result<(), WenlanError> {
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|e| WenlanError::Validation(format!("linked_vault_state_invalid: {e}")))?;
    let parent = path
        .parent()
        .ok_or_else(|| WenlanError::Validation("linked_vault_path_invalid".into()))?;
    std::fs::create_dir_all(parent)?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Where the copy state of linked vaults is kept: beside `config.json`.
pub fn state_dir() -> PathBuf {
    crate::config::config_root().join("linked-vaults")
}

/// One copy pass for every vault `config` links, into its pages folder.
/// Each result is that vault's; one vault that cannot be read does not stop
/// the others.
pub fn bridge_all(
    config: &crate::config::Config,
) -> Vec<(LinkedVault, Result<BridgeStats, WenlanError>)> {
    if config.linked_vaults.is_empty() {
        return Vec::new();
    }
    let pages = config.knowledge_path_or_default();
    let state = state_dir();
    config
        .linked_vaults
        .iter()
        .map(|link| (link.clone(), bridge_pass(link, &pages, &state)))
        .collect()
}

#[cfg(test)]
#[path = "linked_vault_test.rs"]
mod tests;
