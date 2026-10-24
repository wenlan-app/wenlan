// SPDX-License-Identifier: Apache-2.0
//! Move the pages folder to a folder the person chose, such as one inside
//! their Obsidian vault, so Wenlan's pages sit next to their own notes.
//!
//! A move is asked for while the daemon runs ([`check_target`] then
//! `Config::pending_knowledge_path`) and carried out at the next start
//! ([`apply_pending_move`]), before anything reads or writes a page. Nothing
//! else touches the folder at that moment, so no page write can land in the
//! old folder halfway through.
//!
//! Each top-level entry moves with one `rename`, which is atomic on the same
//! volume. Across volumes an entry is copied under a private name in the new
//! folder, renamed into place, and only then removed from the old one. An
//! interrupted move is finished on the next start: an entry already in place
//! with the same bytes is not copied again.

use crate::error::WenlanError;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use wenlan_types::sources::{Source, SourceType};

/// Prefix of an entry being copied into the new folder. Never a page name:
/// pages are not dot-prefixed.
const MOVING_PREFIX: &str = ".wenlan-moving-";

/// Folders inside a vault that belong to the vault's app, not to its notes.
const APP_DIRS: &[&str] = &[".obsidian", ".trash", ".git", "logseq"];

/// Files an empty-looking folder may still hold.
const IGNORABLE: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini"];

fn invalid(code: &str) -> WenlanError {
    WenlanError::Validation(code.to_string())
}

fn resolved(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `path` with its nearest existing ancestor canonicalized, so a folder that
/// does not exist yet still compares correctly against existing ones.
fn resolved_maybe_missing(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut cursor = path.to_path_buf();
    loop {
        if let Ok(real) = std::fs::canonicalize(&cursor) {
            let mut out = real;
            for part in missing.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (cursor.file_name().map(ToOwned::to_owned), cursor.parent()) {
            (Some(name), Some(parent)) => {
                missing.push(name);
                cursor = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn nested(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn is_ignorable(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| IGNORABLE.contains(&name))
}

/// Whether `dir` holds nothing a person put there.
fn is_empty_dir(dir: &Path) -> Result<bool, WenlanError> {
    for entry in std::fs::read_dir(dir)? {
        if !is_ignorable(&entry?.file_name()) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Check that `target` can become the pages folder in place of `current`.
/// Returns the folder as it will be stored. Errors are `Validation` codes the
/// app turns into words.
pub fn check_target(
    current: &Path,
    target: &Path,
    sources: &[Source],
) -> Result<PathBuf, WenlanError> {
    if !target.is_absolute() {
        return Err(invalid("pages_folder_not_absolute"));
    }
    if target
        .components()
        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid("pages_folder_not_absolute"));
    }
    let wanted = resolved_maybe_missing(target);
    let now = resolved(current);
    if wanted == now {
        return Err(invalid("pages_folder_unchanged"));
    }
    if nested(&wanted, &now) {
        return Err(invalid("pages_folder_nested"));
    }
    if wanted.components().any(|part| {
        part.as_os_str()
            .to_str()
            .is_some_and(|name| APP_DIRS.contains(&name))
    }) {
        return Err(invalid("pages_folder_in_app_folder"));
    }
    match std::fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => return Err(invalid("pages_folder_symlink")),
        Ok(meta) if !meta.is_dir() => return Err(invalid("pages_folder_not_a_folder")),
        Ok(_) => {
            if !is_empty_dir(target)? {
                return Err(invalid("pages_folder_not_empty"));
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let parent_ok = target.parent().is_some_and(Path::is_dir);
            if !parent_ok {
                return Err(invalid("pages_folder_parent_missing"));
            }
        }
        Err(error) => return Err(error.into()),
    }
    // An OKF bundle is a whole-folder format of its own; pages inside one, or
    // one inside the pages folder, would mix two writers in one tree.
    if sources.iter().any(|source| {
        source.source_type == SourceType::Okf && nested(&wanted, &resolved(&source.path))
    }) {
        return Err(invalid("pages_folder_overlaps_bundle"));
    }
    Ok(target.to_path_buf())
}

/// Whether `path` is the pages folder or inside it. Source scans use this to
/// leave Wenlan's own pages, source stubs and archive out when the pages
/// folder sits inside a vault or a watched folder.
pub fn is_in_pages_folder(path: &Path, pages_folder: &Path) -> bool {
    path.starts_with(pages_folder) || resolved(path).starts_with(resolved(pages_folder))
}

/// Drop every path inside `pages_folder` from a source scan of `root`.
/// A no-op unless the pages folder is inside `root`, so an ordinary scan pays
/// one comparison.
pub fn without_pages_folder(root: &Path, pages_folder: &Path, paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let pages = resolved(pages_folder);
    let root = resolved(root);
    if !pages.starts_with(&root) {
        return paths;
    }
    paths
        .into_iter()
        .filter(|path| !is_in_pages_folder(path, &pages))
        .collect()
}

fn is_cross_device(error: &std::io::Error) -> bool {
    if error.kind() == ErrorKind::CrossesDevices {
        return true;
    }
    // Windows reports a move between volumes as ERROR_NOT_SAME_DEVICE.
    cfg!(windows) && error.raw_os_error() == Some(17)
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        // A page folder holds no links of its own; keep one as the link it is
        // rather than copying whatever it points at.
        let target = std::fs::read_link(from)?;
        #[cfg(unix)]
        return std::os::unix::fs::symlink(target, to);
        #[cfg(not(unix))]
        return std::fs::copy(target, to).map(|_| ());
    }
    if meta.is_dir() {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        std::fs::set_permissions(to, meta.permissions())?;
        return Ok(());
    }
    // `fs::copy` carries the permission bits, so a 0600 page stays 0600.
    std::fs::copy(from, to).map(|_| ())
}

fn remove_tree(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Whether `a` and `b` hold the same names and bytes all the way down.
fn same_tree(a: &Path, b: &Path) -> std::io::Result<bool> {
    let (ma, mb) = (std::fs::symlink_metadata(a)?, std::fs::symlink_metadata(b)?);
    if ma.file_type().is_symlink() || mb.file_type().is_symlink() {
        return Ok(ma.file_type().is_symlink()
            && mb.file_type().is_symlink()
            && std::fs::read_link(a)? == std::fs::read_link(b)?);
    }
    if ma.is_dir() != mb.is_dir() {
        return Ok(false);
    }
    if !ma.is_dir() {
        return Ok(ma.len() == mb.len() && std::fs::read(a)? == std::fs::read(b)?);
    }
    let names = |dir: &Path| -> std::io::Result<std::collections::BTreeSet<std::ffi::OsString>> {
        std::fs::read_dir(dir)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect()
    };
    let (na, nb) = (names(a)?, names(b)?);
    if na != nb {
        return Ok(false);
    }
    for name in na {
        if !same_tree(&a.join(&name), &b.join(&name))? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Move everything in `from` into `to`. Returns how many top-level entries
/// moved. Safe to call again after an interruption.
pub fn move_pages_folder(from: &Path, to: &Path) -> Result<usize, WenlanError> {
    std::fs::create_dir_all(to)?;
    // A copy cut short last time is incomplete by definition.
    for entry in std::fs::read_dir(to)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(MOVING_PREFIX))
        {
            remove_tree(&entry.path())?;
        }
    }
    if !from.is_dir() {
        return Ok(0);
    }
    let mut moved = 0;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if is_ignorable(&name) {
            continue;
        }
        let source = entry.path();
        let destination = to.join(&name);
        if std::fs::symlink_metadata(&destination).is_ok() {
            // Only an interrupted cross-volume move leaves both: the copy is
            // in place and the original was not removed yet.
            if same_tree(&source, &destination)? {
                remove_tree(&source)?;
                moved += 1;
                continue;
            }
            return Err(WenlanError::Conflict(format!(
                "pages_folder_move_conflict: {}",
                destination.display()
            )));
        }
        match std::fs::rename(&source, &destination) {
            Ok(()) => moved += 1,
            Err(error) if is_cross_device(&error) => {
                let staging = to.join(format!("{MOVING_PREFIX}{}", name.to_string_lossy()));
                copy_tree(&source, &staging)?;
                std::fs::rename(&staging, &destination)?;
                remove_tree(&source)?;
                moved += 1;
            }
            Err(error) => return Err(error.into()),
        }
    }
    // The old folder is left in place, empty, rather than removed: a person
    // may have it open in a file manager or bookmarked in an editor.
    Ok(moved)
}

/// What [`apply_pending_move`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveOutcome {
    /// No move was asked for.
    None,
    /// The pages folder is now `to`.
    Moved {
        from: PathBuf,
        to: PathBuf,
        entries: usize,
    },
    /// The move stopped. Pages stay readable where they are; the error is
    /// kept in `Config::pages_folder_move_error` for the app to show.
    Failed { error: String },
}

/// Carry out a move asked for with `Config::pending_knowledge_path`. Call at
/// daemon start, before anything opens the pages folder.
pub fn apply_pending_move() -> MoveOutcome {
    let config = crate::config::load_config();
    let Some(target) = config.pending_knowledge_path.clone() else {
        return MoveOutcome::None;
    };
    let from = config.knowledge_path_or_default();
    let result = move_pages_folder(&from, &target);
    let outcome = match &result {
        Ok(entries) => MoveOutcome::Moved {
            from: from.clone(),
            to: target.clone(),
            entries: *entries,
        },
        Err(error) => MoveOutcome::Failed {
            error: error.to_string(),
        },
    };
    let saved = crate::config::update_config(|config| {
        config.pending_knowledge_path = None;
        match &result {
            Ok(_) => {
                config.knowledge_path = Some(target.clone());
                config.pages_folder_move_error = None;
            }
            Err(error) => {
                // Put back what already moved, so the pages stay in one folder.
                if let Err(undo) = move_pages_folder(&target, &from) {
                    log::error!(
                        "[pages-folder] could not move pages back to {}: {undo}",
                        from.display()
                    );
                    // Half the pages are in each folder. Point at the new one,
                    // where the state file most likely went, and keep asking.
                    config.pending_knowledge_path = Some(target.clone());
                }
                config.pages_folder_move_error = Some(error.to_string());
            }
        }
        Ok(())
    });
    if let Err(error) = saved {
        log::error!("[pages-folder] could not record the move in config.json: {error}");
    }
    outcome
}

#[cfg(test)]
#[path = "pages_folder_test.rs"]
mod tests;
