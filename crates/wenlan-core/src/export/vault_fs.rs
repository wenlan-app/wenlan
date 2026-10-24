// SPDX-License-Identifier: Apache-2.0
//! Acting on the file that was classified, not on whatever the name points at
//! by the time we act.
//!
//! The projection root is very often a live Obsidian vault with a sync client
//! beside it, so an editor can save between any two filesystem calls. Every
//! writer under `export/` used to read a file, decide it was Wenlan's to
//! remove or overwrite, and then act on the PATHNAME. A save that landed in
//! between was deleted or replaced. A digest re-check does not fix that: it
//! narrows the window and still acts on the name afterwards.
//!
//! The pieces here close it by making the decision about bytes we hold:
//!
//! 1. [`take_aside`] renames the file to a private name in the same
//!    directory. The rename is atomic and captures exactly what was at the
//!    name at that instant; an editor's next save creates a NEW file at the
//!    name, which nothing here touches.
//! 2. The caller re-reads and re-classifies the bytes it now holds. Still
//!    safe to discard: unlink the private name (nobody else knows it). Not
//!    safe: [`rename_noreplace`] puts it back, and if the name was taken in the
//!    meantime the bytes stay reachable at the private name for the caller to
//!    archive. Nothing is ever lost.
//! 3. A planned write installs its bytes with [`rename_noreplace`] too, so a
//!    file that appeared at the name is kept rather than replaced.
//!
//! Residual: an editor that kept the OLD inode open and writes through that
//! handle after the final read and unlink is not recoverable from here. No
//! pathname-level scheme can close that, only keeping the inode alive can.

use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt as _};
use cap_std::fs::{Dir, OpenOptions, Permissions};
use std::io::{self, Read as _, Write as _};
use std::path::Path;

/// A name nobody but this process knows, in the directory the file is already
/// in. Dot-prefixed so the importer, the page watcher and `wenlan pages` skip
/// it, and not shaped like a `write_page` temp (`.{page_id}.{pid}.{seq}.tmp`)
/// so the startup sweep never reaps a copy that holds somebody's edit.
pub(crate) fn private_name(purpose: &str) -> String {
    format!(".wenlan-{purpose}-{}.tmp", uuid::Uuid::new_v4().simple())
}

fn single_component(name: &str) -> io::Result<()> {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(_)), None) => Ok(()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name:?} is not a single file name"),
        )),
    }
}

/// Rename `src` in `src_dir` to `dst` in `dst_dir`, failing with
/// [`io::ErrorKind::AlreadyExists`] instead of replacing a destination that
/// exists. The check and the rename are one atomic step where the platform
/// has one; see [`rename_noreplace_via_hard_link`] and
/// [`rename_noreplace_probing`] for what happens where it does not.
///
/// `src` must be a private name this process created (see [`private_name`]).
/// The hard-link fallback unlinks `src` afterwards, and that is only safe when
/// `src` is not a name a person could be using.
pub(crate) fn rename_noreplace(
    src_dir: &Dir,
    src: &str,
    dst_dir: &Dir,
    dst: &str,
) -> io::Result<()> {
    single_component(src)?;
    single_component(dst)?;
    match rename_noreplace_native(src_dir, src, dst_dir, dst) {
        Err(error) if native_noreplace_unsupported(&error) => {}
        other => return other,
    }
    match rename_noreplace_via_hard_link(src_dir, src, dst_dir, dst) {
        Err(error)
            if !matches!(
                error.kind(),
                io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound
            ) =>
        {
            // Hard links are not available here either (FAT, exFAT, some
            // network shares). Nothing atomic is left.
            rename_noreplace_probing(src_dir, src, dst_dir, dst)
        }
        other => other,
    }
}

/// `renameat2(RENAME_NOREPLACE)` on Linux and Android, `renameatx_np(RENAME_EXCL)`
/// on macOS: one syscall that renames only if the destination is absent.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn rename_noreplace_native(src_dir: &Dir, src: &str, dst_dir: &Dir, dst: &str) -> io::Result<()> {
    rustix::fs::renameat_with(
        src_dir,
        src,
        dst_dir,
        dst,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(io::Error::from)
}

/// No single-call no-replace rename on this platform.
#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn rename_noreplace_native(
    _src_dir: &Dir,
    _src: &str,
    _dst_dir: &Dir,
    _dst: &str,
) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// The filesystem (or kernel) does not implement the flag, as opposed to
/// refusing this particular rename.
fn native_noreplace_unsupported(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::Unsupported {
        return true;
    }
    #[cfg(unix)]
    {
        use rustix::io::Errno;
        matches!(
            error.raw_os_error(),
            Some(code) if code == Errno::INVAL.raw_os_error()
                || code == Errno::NOSYS.raw_os_error()
                || code == Errno::OPNOTSUPP.raw_os_error()
        )
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// No-replace rename built from `link` + `unlink`: creating a hard link fails
/// with `AlreadyExists` if the destination exists, atomically, and the private
/// source name is dropped once the destination holds the inode.
///
/// Used where the filesystem has no no-replace rename (Windows, filesystems
/// that reject the flag). `src` must be private for the reason on
/// [`rename_noreplace`].
pub(crate) fn rename_noreplace_via_hard_link(
    src_dir: &Dir,
    src: &str,
    dst_dir: &Dir,
    dst: &str,
) -> io::Result<()> {
    src_dir.hard_link(src, dst_dir, dst)?;
    if let Err(error) = src_dir.remove_file(src) {
        if error.kind() != io::ErrorKind::NotFound {
            // The destination already holds the bytes; a leftover private
            // name costs clutter, not data.
            log::warn!("[export] could not drop private name {src} after linking: {error}");
        }
    }
    Ok(())
}

/// Probe, then rename. NOT atomic: a file that appears at `dst` between the
/// probe and the rename is replaced. This is the last-resort fallback for a
/// filesystem with neither a no-replace rename nor hard links, and the
/// residual race it leaves is the one the callers used to have everywhere.
pub(crate) fn rename_noreplace_probing(
    src_dir: &Dir,
    src: &str,
    dst_dir: &Dir,
    dst: &str,
) -> io::Result<()> {
    match dst_dir.symlink_metadata(dst) {
        Ok(_) => return Err(io::Error::from(io::ErrorKind::AlreadyExists)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    src_dir.rename(src, dst_dir, dst)
}

/// Atomically move whatever is at `name` to a private name in `dir`, and
/// return that name. `None` means nothing was there.
///
/// This captures the file at `name` at that instant, whichever inode it is. A
/// later save by an editor makes a new file at `name`; this one is ours to
/// inspect without it changing names under us.
pub(crate) fn take_aside(dir: &Dir, name: &str) -> io::Result<Option<String>> {
    single_component(name)?;
    let aside = private_name("aside");
    match dir.rename(name, dir, &aside) {
        Ok(()) => Ok(Some(aside)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The bytes of the plain file at `name`, or `None` when it is not a plain
/// file, cannot be read, or is larger than `limit`. A file this pass cannot
/// read whole is a file it cannot classify, and an unclassified file is kept.
pub(crate) fn read_plain_bounded(dir: &Dir, name: &str, limit: u64) -> Option<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let mut file = dir.open_with(name, &options).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    (&mut file).take(limit + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
}

/// What [`remove_if_unchanged`] found and did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Disposal {
    /// Nothing was at the name.
    Absent,
    /// The bytes still passed the caller's test and were unlinked.
    Removed,
    /// The bytes no longer passed (somebody edited the file after it was
    /// classified). They are back at the name, untouched.
    Kept,
    /// As [`Disposal::Kept`], except a newer file had taken the name, so the
    /// captured bytes could not go back. They are still in the directory under
    /// this private name, and the caller must preserve them.
    Displaced(String),
}

/// Remove the file at `name` only if its bytes, read AFTER it was taken aside,
/// still pass `is_ours`.
///
/// `is_ours` sees `None` for something it cannot read whole (not a plain file,
/// unreadable, over `limit`) and must answer `false` for it.
pub(crate) fn remove_if_unchanged(
    dir: &Dir,
    name: &str,
    limit: u64,
    is_ours: impl FnOnce(&[u8]) -> bool,
) -> io::Result<Disposal> {
    remove_if_unchanged_with_hook(dir, name, limit, is_ours, |_| {})
}

/// [`remove_if_unchanged`] with a seam between taking the file aside and
/// re-reading it. `after_aside` receives the private name. Production passes a
/// no-op.
pub(crate) fn remove_if_unchanged_with_hook(
    dir: &Dir,
    name: &str,
    limit: u64,
    is_ours: impl FnOnce(&[u8]) -> bool,
    after_aside: impl FnOnce(&str),
) -> io::Result<Disposal> {
    let Some(aside) = take_aside(dir, name)? else {
        return Ok(Disposal::Absent);
    };
    after_aside(&aside);
    let still_ours = read_plain_bounded(dir, &aside, limit).is_some_and(|bytes| is_ours(&bytes));
    if still_ours {
        return match dir.remove_file(&aside) {
            Ok(()) => Ok(Disposal::Removed),
            // Our private name is gone already; so is the file.
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Disposal::Removed),
            Err(error) => {
                log::warn!("[export] could not unlink {aside} (was {name}): {error}");
                Err(error)
            }
        };
    }
    put_back(dir, &aside, name)
}

/// Return a file taken aside to its name without replacing anything there.
fn put_back(dir: &Dir, aside: &str, name: &str) -> io::Result<Disposal> {
    match rename_noreplace(dir, aside, dir, name) {
        Ok(()) => Ok(Disposal::Kept),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Ok(Disposal::Displaced(aside.to_string()))
        }
        Err(error) => {
            log::warn!("[export] could not put {name} back from {aside}: {error}");
            Err(error)
        }
    }
}

/// What [`replace_if_unchanged`] did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Replacement {
    /// The new bytes are at the name.
    Written,
    /// The file at the name already held exactly the new bytes. Nothing was
    /// done, so nothing could be lost.
    Unchanged,
    /// The file at the name is not one the caller may replace, or a file the
    /// caller did not classify took the name. It was left at the name and the
    /// new bytes were not written.
    Kept,
    /// As [`Replacement::Kept`], with the captured bytes still in the directory
    /// under this private name because a newer file had taken the name. The
    /// caller must preserve them.
    Displaced(String),
}

/// Write `bytes` to `name`, replacing the file there only if its bytes, read
/// after it was taken aside, still pass `is_ours`.
///
/// The new bytes are staged under a private name first, so a failure to write
/// them (disk full) happens before the existing file is touched. The old file
/// then moves aside, is re-read, and either gives way (the staged file is
/// installed with [`rename_noreplace`], then the old bytes are unlinked) or
/// goes back. A name that is free, or that a file appeared at meanwhile, is
/// never overwritten.
///
/// A non-file at the name is an `AlreadyExists` error, as before: the caller
/// reports the path as failed rather than moving something it does not
/// understand.
pub(crate) fn replace_if_unchanged(
    dir: &Dir,
    name: &str,
    bytes: &[u8],
    limit: u64,
    is_ours: impl FnOnce(&[u8]) -> bool,
) -> io::Result<Replacement> {
    replace_if_unchanged_with_hook(dir, name, bytes, limit, is_ours, false, |_| {})
}

/// [`replace_if_unchanged`] for a file that holds private state, such as the
/// projection's `index.md`: a file created at a free name is `0600`, as the
/// projection's atomic page writer makes it. A replaced file keeps the
/// permissions it had either way.
pub(crate) fn replace_private_if_unchanged(
    dir: &Dir,
    name: &str,
    bytes: &[u8],
    limit: u64,
    is_ours: impl FnOnce(&[u8]) -> bool,
) -> io::Result<Replacement> {
    replace_if_unchanged_with_hook(dir, name, bytes, limit, is_ours, true, |_| {})
}

pub(crate) fn replace_if_unchanged_with_hook(
    dir: &Dir,
    name: &str,
    bytes: &[u8],
    limit: u64,
    is_ours: impl FnOnce(&[u8]) -> bool,
    private_when_new: bool,
    after_aside: impl FnOnce(&str),
) -> io::Result<Replacement> {
    single_component(name)?;
    let permissions = match dir.symlink_metadata(name) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            Some(metadata.permissions())
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{name} is not a plain file; refusing to replace it"),
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    // Nothing to replace: the bytes are already there. Doing nothing cannot
    // lose an edit, and it spares the vault a delete-and-recreate on every
    // export of an unchanged file.
    if permissions.is_some() && read_plain_bounded(dir, name, limit).as_deref() == Some(bytes) {
        return Ok(Replacement::Unchanged);
    }

    let staged = private_name("new");
    stage_file(dir, &staged, bytes, permissions, private_when_new)?;
    let result = install_staged(dir, name, &staged, limit, is_ours, after_aside);
    // On success the staged name was renamed away; this is for every other
    // path and is a no-op when it is already gone.
    let _ = dir.remove_file(&staged);
    result
}

fn stage_file(
    dir: &Dir,
    staged: &str,
    bytes: &[u8],
    permissions: Option<Permissions>,
    private_when_new: bool,
) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    if private_when_new {
        use cap_std::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private_when_new;
    let mut file = dir.open_with(staged, &options)?;
    let written = (|| {
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if written.is_err() {
        drop(file);
        let _ = dir.remove_file(staged);
    }
    written
}

fn install_staged(
    dir: &Dir,
    name: &str,
    staged: &str,
    limit: u64,
    is_ours: impl FnOnce(&[u8]) -> bool,
    after_aside: impl FnOnce(&str),
) -> io::Result<Replacement> {
    let aside = take_aside(dir, name)?;
    if let Some(aside) = &aside {
        after_aside(aside);
        let still_ours = read_plain_bounded(dir, aside, limit).is_some_and(|bytes| is_ours(&bytes));
        if !still_ours {
            return Ok(match put_back(dir, aside, name)? {
                Disposal::Displaced(aside) => Replacement::Displaced(aside),
                _ => Replacement::Kept,
            });
        }
    }
    match rename_noreplace(dir, staged, dir, name) {
        Ok(()) => {
            if let Some(aside) = aside {
                drop_private(dir, &aside, name);
            }
            Ok(Replacement::Written)
        }
        // A file took the name after the old one went aside. It is not ours
        // to replace. The old bytes passed the test above, so they are
        // discardable and the newer file stays.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if let Some(aside) = aside {
                drop_private(dir, &aside, name);
            }
            Ok(Replacement::Kept)
        }
        // The install failed outright. The old bytes go back if they can.
        Err(error) => match aside {
            Some(aside) => match put_back(dir, &aside, name) {
                Ok(Disposal::Displaced(aside)) => Ok(Replacement::Displaced(aside)),
                Ok(_) | Err(_) => Err(error),
            },
            None => Err(error),
        },
    }
}

/// Unlink a private name. A leftover costs clutter, never data, so a failure
/// is logged rather than raised.
fn drop_private(dir: &Dir, private: &str, name: &str) {
    if let Err(error) = dir.remove_file(private) {
        if error.kind() != io::ErrorKind::NotFound {
            log::warn!("[export] could not unlink {private} (was {name}): {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> (tempfile::TempDir, Dir) {
        let dir = tempfile::tempdir().unwrap();
        let root = Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();
        (dir, root)
    }

    fn assert_kind(result: io::Result<()>, kind: io::ErrorKind) {
        assert_eq!(result.unwrap_err().kind(), kind);
    }

    /// Nothing at the destination: the source moves, atomically.
    #[test]
    fn rename_noreplace_moves_to_a_free_name() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join(".src.tmp"), b"payload").unwrap();
        rename_noreplace(&root, ".src.tmp", &root, "dst.md").unwrap();
        assert!(!dir.path().join(".src.tmp").exists());
        assert_eq!(
            std::fs::read(dir.path().join("dst.md")).unwrap(),
            b"payload"
        );
    }

    /// The point of the helper: an existing destination is refused, left
    /// exactly as it is, and the source stays where it was.
    #[test]
    fn rename_noreplace_refuses_an_existing_destination() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join(".src.tmp"), b"payload").unwrap();
        std::fs::write(dir.path().join("dst.md"), b"theirs").unwrap();
        assert_kind(
            rename_noreplace(&root, ".src.tmp", &root, "dst.md"),
            io::ErrorKind::AlreadyExists,
        );
        assert_eq!(std::fs::read(dir.path().join("dst.md")).unwrap(), b"theirs");
        assert_eq!(
            std::fs::read(dir.path().join(".src.tmp")).unwrap(),
            b"payload"
        );
    }

    /// It works across two directories, the shape `archive/` uses.
    #[test]
    fn rename_noreplace_works_across_directories() {
        let (dir, root) = temp_root();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let sub = root.open_dir("sub").unwrap();
        std::fs::write(dir.path().join(".src.tmp"), b"payload").unwrap();
        rename_noreplace(&root, ".src.tmp", &sub, "dst.md").unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("sub/dst.md")).unwrap(),
            b"payload"
        );
        std::fs::write(dir.path().join(".again.tmp"), b"second").unwrap();
        assert_kind(
            rename_noreplace(&root, ".again.tmp", &sub, "dst.md"),
            io::ErrorKind::AlreadyExists,
        );
    }

    /// A name that is not one path component is refused before any syscall.
    #[test]
    fn rename_noreplace_refuses_a_path() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join(".src.tmp"), b"payload").unwrap();
        assert_kind(
            rename_noreplace(&root, ".src.tmp", &root, "sub/dst.md"),
            io::ErrorKind::InvalidInput,
        );
        assert_kind(
            rename_noreplace(&root, ".src.tmp", &root, ".."),
            io::ErrorKind::InvalidInput,
        );
    }

    /// The fallback used where the filesystem has no no-replace rename.
    #[test]
    fn hard_link_fallback_moves_to_a_free_name_and_drops_the_source() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join(".src.tmp"), b"payload").unwrap();
        rename_noreplace_via_hard_link(&root, ".src.tmp", &root, "dst.md").unwrap();
        assert!(!dir.path().join(".src.tmp").exists());
        assert_eq!(
            std::fs::read(dir.path().join("dst.md")).unwrap(),
            b"payload"
        );
    }

    #[test]
    fn hard_link_fallback_refuses_an_existing_destination_and_keeps_the_source() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join(".src.tmp"), b"payload").unwrap();
        std::fs::write(dir.path().join("dst.md"), b"theirs").unwrap();
        assert_kind(
            rename_noreplace_via_hard_link(&root, ".src.tmp", &root, "dst.md"),
            io::ErrorKind::AlreadyExists,
        );
        assert_eq!(std::fs::read(dir.path().join("dst.md")).unwrap(), b"theirs");
        assert_eq!(
            std::fs::read(dir.path().join(".src.tmp")).unwrap(),
            b"payload"
        );
    }

    /// The last-resort probe still refuses a destination that is visibly
    /// there; it is the window after the probe that it cannot close.
    #[test]
    fn probing_fallback_refuses_a_visible_destination() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join(".src.tmp"), b"payload").unwrap();
        std::fs::write(dir.path().join("dst.md"), b"theirs").unwrap();
        assert_kind(
            rename_noreplace_probing(&root, ".src.tmp", &root, "dst.md"),
            io::ErrorKind::AlreadyExists,
        );
        assert_eq!(std::fs::read(dir.path().join("dst.md")).unwrap(), b"theirs");
        std::fs::remove_file(dir.path().join("dst.md")).unwrap();
        rename_noreplace_probing(&root, ".src.tmp", &root, "dst.md").unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("dst.md")).unwrap(),
            b"payload"
        );
    }

    /// Still ours when re-read after being taken aside: gone, with nothing
    /// left behind under a private name.
    #[test]
    fn remove_if_unchanged_removes_bytes_that_still_pass() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("a.md"), b"pristine").unwrap();
        let outcome = remove_if_unchanged(&root, "a.md", 1024, |b| b == b"pristine").unwrap();
        assert_eq!(outcome, Disposal::Removed);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn remove_if_unchanged_reports_a_missing_file() {
        let (_dir, root) = temp_root();
        assert_eq!(
            remove_if_unchanged(&root, "a.md", 1024, |_| true).unwrap(),
            Disposal::Absent
        );
    }

    /// An edit made through the same inode after the file was taken aside is
    /// seen by the re-read, and the file goes back to its name.
    #[test]
    fn remove_if_unchanged_puts_back_bytes_edited_after_the_take() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("a.md"), b"pristine").unwrap();
        let base = dir.path().to_path_buf();
        let outcome = remove_if_unchanged_with_hook(
            &root,
            "a.md",
            1024,
            |b| b == b"pristine",
            |aside| std::fs::write(base.join(aside), b"edited through an open handle").unwrap(),
        )
        .unwrap();
        assert_eq!(outcome, Disposal::Kept);
        assert_eq!(
            std::fs::read(dir.path().join("a.md")).unwrap(),
            b"edited through an open handle"
        );
    }

    /// A newer file took the name while the edited bytes were aside: the
    /// newer file stays, and the edit is handed back at a private name.
    #[test]
    fn remove_if_unchanged_does_not_replace_a_file_that_took_the_name() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("a.md"), b"pristine").unwrap();
        let base = dir.path().to_path_buf();
        let outcome = remove_if_unchanged_with_hook(
            &root,
            "a.md",
            1024,
            |b| b == b"pristine",
            |aside| {
                std::fs::write(base.join(aside), b"edit").unwrap();
                std::fs::write(base.join("a.md"), b"newer save").unwrap();
            },
        )
        .unwrap();
        let Disposal::Displaced(aside) = outcome else {
            panic!("expected the edit to be handed back, got {outcome:?}");
        };
        assert_eq!(
            std::fs::read(dir.path().join("a.md")).unwrap(),
            b"newer save"
        );
        assert_eq!(std::fs::read(dir.path().join(aside)).unwrap(), b"edit");
    }

    /// Something that is not a plain file cannot be classified and is kept.
    #[cfg(unix)]
    #[test]
    fn remove_if_unchanged_keeps_a_symlink() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("target.md"), b"pristine").unwrap();
        std::os::unix::fs::symlink("target.md", dir.path().join("a.md")).unwrap();
        let outcome = remove_if_unchanged(&root, "a.md", 1024, |_| true).unwrap();
        assert_eq!(outcome, Disposal::Kept);
        assert!(std::fs::symlink_metadata(dir.path().join("a.md"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn replace_if_unchanged_installs_over_bytes_that_pass() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("a.md"), b"old").unwrap();
        let outcome = replace_if_unchanged(&root, "a.md", b"new", 1024, |b| b == b"old").unwrap();
        assert_eq!(outcome, Replacement::Written);
        assert_eq!(std::fs::read(dir.path().join("a.md")).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn replace_if_unchanged_writes_a_free_name() {
        let (dir, root) = temp_root();
        let outcome = replace_if_unchanged(&root, "a.md", b"new", 1024, |_| false).unwrap();
        assert_eq!(outcome, Replacement::Written);
        assert_eq!(std::fs::read(dir.path().join("a.md")).unwrap(), b"new");
    }

    /// Identical bytes: no delete-and-recreate for the vault to notice.
    #[test]
    fn replace_if_unchanged_leaves_identical_bytes_alone() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("a.md"), b"same").unwrap();
        let before = std::fs::metadata(dir.path().join("a.md")).unwrap();
        let outcome = replace_if_unchanged(&root, "a.md", b"same", 1024, |_| false).unwrap();
        assert_eq!(outcome, Replacement::Unchanged);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let after = std::fs::metadata(dir.path().join("a.md")).unwrap();
            assert_eq!(before.ino(), after.ino(), "same inode: nothing was done");
        }
        #[cfg(not(unix))]
        let _ = before;
    }

    #[test]
    fn replace_if_unchanged_keeps_bytes_that_no_longer_pass() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("a.md"), b"their edit").unwrap();
        let outcome = replace_if_unchanged(&root, "a.md", b"new", 1024, |b| b == b"old").unwrap();
        assert_eq!(outcome, Replacement::Kept);
        assert_eq!(
            std::fs::read(dir.path().join("a.md")).unwrap(),
            b"their edit"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// Edit lands on the old inode after it was taken aside.
    #[test]
    fn replace_if_unchanged_keeps_an_edit_made_after_the_take() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("a.md"), b"old").unwrap();
        let base = dir.path().to_path_buf();
        let outcome = replace_if_unchanged_with_hook(
            &root,
            "a.md",
            b"new",
            1024,
            |b| b == b"old",
            false,
            |aside| std::fs::write(base.join(aside), b"edit").unwrap(),
        )
        .unwrap();
        assert_eq!(outcome, Replacement::Kept);
        assert_eq!(std::fs::read(dir.path().join("a.md")).unwrap(), b"edit");
    }

    /// A save that creates the name while the old file is aside is not
    /// replaced by our write; the old bytes are handed back.
    #[test]
    fn replace_if_unchanged_does_not_replace_a_file_created_while_aside() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("a.md"), b"old").unwrap();
        let base = dir.path().to_path_buf();
        let outcome = replace_if_unchanged_with_hook(
            &root,
            "a.md",
            b"new",
            1024,
            |b| b == b"old",
            false,
            |_| std::fs::write(base.join("a.md"), b"their save").unwrap(),
        )
        .unwrap();
        // The captured bytes were still ours (discardable), so the only thing
        // in the way is the newer file: it stays and the write is skipped.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(outcome, Replacement::Kept);
        assert_eq!(
            std::fs::read(dir.path().join("a.md")).unwrap(),
            b"their save"
        );
    }

    #[test]
    fn replace_if_unchanged_refuses_a_directory() {
        let (dir, root) = temp_root();
        std::fs::create_dir(dir.path().join("a.md")).unwrap();
        let error = replace_if_unchanged(&root, "a.md", b"new", 1024, |_| true).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    }

    /// A staged file never outlives the call, whatever the outcome.
    #[test]
    fn replace_if_unchanged_leaves_no_private_names_behind() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("a.md"), b"edit").unwrap();
        replace_if_unchanged(&root, "a.md", b"new", 1024, |_| false).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("a.md")]);
    }

    /// A private file created at a free name is `0600`; a replaced one keeps
    /// the permissions it had.
    #[cfg(unix)]
    #[test]
    fn replace_private_if_unchanged_creates_0600_and_keeps_existing_modes() {
        use std::os::unix::fs::PermissionsExt as _;
        let (dir, root) = temp_root();
        let outcome =
            replace_private_if_unchanged(&root, "index.md", b"new", 1024, |_| true).unwrap();
        assert_eq!(outcome, Replacement::Written);
        let mode = |name: &str| {
            std::fs::metadata(dir.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("index.md"), 0o600);

        std::fs::set_permissions(
            dir.path().join("index.md"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let outcome =
            replace_private_if_unchanged(&root, "index.md", b"newer", 1024, |_| true).unwrap();
        assert_eq!(outcome, Replacement::Written);
        assert_eq!(mode("index.md"), 0o644);
    }

    /// A save at the name when the staged file is about to go in is taken
    /// aside, found not to be ours, and put back: the save stays.
    #[test]
    fn a_staged_file_does_not_replace_a_save_at_the_name() {
        let (dir, root) = temp_root();
        stage_file(&root, ".wenlan-new-x.tmp", b"ours", None, false).unwrap();
        std::fs::write(dir.path().join("a.md"), b"their save").unwrap();
        let outcome =
            install_staged(&root, "a.md", ".wenlan-new-x.tmp", 1024, |_| false, |_| {}).unwrap();
        assert_eq!(outcome, Replacement::Kept);
        assert_eq!(
            std::fs::read(dir.path().join("a.md")).unwrap(),
            b"their save"
        );
    }
}
