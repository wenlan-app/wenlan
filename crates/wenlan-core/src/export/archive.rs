// SPDX-License-Identifier: Apache-2.0
//! One place for "move this file aside" instead of unlinking it.
//!
//! The projection root is very often the user's own Obsidian vault, so every
//! writer under `export/` that decides a file is no longer wanted answers the
//! same question first: the database can rebuild whatever Wenlan generated,
//! but it cannot rebuild what a person typed into the file afterwards. A
//! marker in frontmatter says Wenlan WROTE a file; it never says the bytes are
//! still Wenlan's, because an edit made in place in the vault changes the body
//! and leaves the frontmatter alone. So a file leaves the projection by moving
//! into `archive/`, in plain sight, rather than by an unlink.

use crate::error::WenlanError;
use crate::export::knowledge::page_stem_clear_of_reserved;
use crate::export::vault_fs;
use cap_fs_ext::DirExt as _;
use cap_std::fs::Dir;

/// Plain and visible, in the projection root. Not a dotfile: a person looking
/// for a file that disappeared should be able to find it without being told
/// where to look.
pub(crate) const ARCHIVE_DIR: &str = "archive";

/// How many suffixed names to try before giving up on a free destination.
const MAX_ARCHIVE_ATTEMPTS: u32 = 100;

/// Open (creating if needed) `archive/` under the projection root.
pub(crate) fn open_archive_dir(root: &Dir) -> Result<Dir, WenlanError> {
    match root.symlink_metadata(ARCHIVE_DIR) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(WenlanError::Conflict(
                "page_archive_target_invalid".to_string(),
            ))
        }
        // `AlreadyExists` here means something created `archive/` between the
        // probe above and this call -- the user's sync client, or a second
        // pass. That is the state we wanted, not a failure; letting it
        // propagate would strand a file that is still readable.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match root.create_dir(ARCHIVE_DIR) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(WenlanError::Io(e)),
            }
        }
        Err(error) => return Err(WenlanError::Io(error)),
    }
    root.open_dir_nofollow(ARCHIVE_DIR).map_err(WenlanError::Io)
}

/// The name an archived copy of `filename` takes inside `archive/`.
///
/// OKF reserves `index.md` and `log.md` at EVERY level of the hierarchy (spec
/// 3.1), and `archive/` is a level. Archiving a file that is being moved off a
/// reserved name under that same name would push the violation one directory
/// down rather than end it, so a reserved name takes the `-page` suffix the
/// projection already gives a page titled "Index" or "Log".
fn archive_basename(filename: &str) -> (String, &'static str) {
    match filename.strip_suffix(".md") {
        Some(stem) => (page_stem_clear_of_reserved(stem.to_string()), ".md"),
        None => (filename.to_string(), ""),
    }
}

/// Move `filename` out of `source` and into the projection root's `archive/`.
///
/// `source` is usually `root` itself; the `_sources/` stub GC passes its own
/// directory so an edited stub lands in the same visible place a page does.
/// Refuses anything that is not a plain file: a directory or special file
/// where a file was expected is a conflict rather than something to move, and
/// no symlink is followed. Already-absent is success -- the invariant is that
/// the file is gone from `source`, not that this pass is the one that moved
/// it.
///
/// The file is renamed into `archive/` under a private name FIRST, which
/// atomically takes whatever is at `filename` at that instant, and only then
/// given its final name with a rename that cannot replace. Probing for a free
/// name and renaming afterwards left a window in which a sync client could
/// create the destination, and `rename` replaces without a word.
pub(crate) fn archive_file_from(
    root: &Dir,
    source: &Dir,
    filename: &str,
) -> Result<(), WenlanError> {
    archive_file_from_with_hook(root, source, filename, &mut |_| {})
}

/// [`archive_file_from`] with a seam before each attempt at a final name.
/// `before_place` receives the candidate name in `archive/`. Production passes
/// a no-op.
fn archive_file_from_with_hook(
    root: &Dir,
    source: &Dir,
    filename: &str,
    before_place: &mut dyn FnMut(&str),
) -> Result<(), WenlanError> {
    match source.symlink_metadata(filename) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(WenlanError::Conflict(
                "page_projection_target_invalid".to_string(),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(WenlanError::Io(error)),
    }
    let archive = open_archive_dir(root)?;
    let held = vault_fs::private_name("aside");
    match source.rename(filename, &archive, &held) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(WenlanError::Io(error)),
    }
    // The pre-check above and the rename are two steps; what was moved is
    // whatever sat at the name at the second. Anything that is not a plain
    // file goes back rather than being filed as an archived page.
    let plain = archive
        .symlink_metadata(&held)
        .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink());
    if !plain {
        if let Err(error) = vault_fs::rename_noreplace(&archive, &held, source, filename) {
            log::warn!("[archive] could not put {filename} back from {held}: {error}");
        }
        return Err(WenlanError::Conflict(
            "page_projection_target_invalid".to_string(),
        ));
    }
    place_held(&archive, &held, filename, before_place)
}

/// Archive a file that was already taken aside in `source` under the private
/// name `aside` (see [`vault_fs::take_aside`]) and turned out to be an edit.
/// `filename` is the name it had, which decides its name in `archive/`.
pub(crate) fn archive_aside_from(
    root: &Dir,
    source: &Dir,
    aside: &str,
    filename: &str,
) -> Result<(), WenlanError> {
    let archive = open_archive_dir(root)?;
    let held = vault_fs::private_name("aside");
    source
        .rename(aside, &archive, &held)
        .map_err(WenlanError::Io)?;
    place_held(&archive, &held, filename, &mut |_| {})
}

fn place_held(
    archive: &Dir,
    held: &str,
    filename: &str,
    before_place: &mut dyn FnMut(&str),
) -> Result<(), WenlanError> {
    let (stem, extension) = archive_basename(filename);
    // The destination can be an older archived copy -- the one thing under
    // this root that the database cannot reproduce -- so it is never replaced:
    // suffix until a rename that refuses to replace succeeds.
    match place_suffixed(archive, held, &stem, extension, before_place).map_err(WenlanError::Io)? {
        Some(_) => Ok(()),
        // The bytes stay in `archive/` under the private name, so nothing is
        // lost; the caller sees the same refusal as before.
        None => Err(WenlanError::Conflict(
            "page_archive_target_exhausted".to_string(),
        )),
    }
}

/// Give the file at `held` in `dir` the first free name among `stem`,
/// `stem-2`, ... `stem-100`, each with `extension`. A name that turns out to be
/// taken at the moment of the rename is skipped, not replaced. Returns the name
/// used, or `None` when every candidate was taken.
pub(crate) fn place_suffixed(
    dir: &Dir,
    held: &str,
    stem: &str,
    extension: &str,
    before_place: &mut dyn FnMut(&str),
) -> std::io::Result<Option<String>> {
    for attempt in 1..=MAX_ARCHIVE_ATTEMPTS {
        let target = if attempt == 1 {
            format!("{stem}{extension}")
        } else {
            format!("{stem}-{attempt}{extension}")
        };
        before_place(&target);
        match vault_fs::rename_noreplace(dir, held, dir, &target) {
            Ok(()) => return Ok(Some(target)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> (tempfile::TempDir, Dir) {
        let dir = tempfile::tempdir().unwrap();
        let root = Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();
        (dir, root)
    }

    /// An ordinary page archives under its own name.
    #[test]
    fn archives_under_the_original_name() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("alpha.md"), b"body").unwrap();
        archive_file_from(&root, &root, "alpha.md").unwrap();
        assert!(!dir.path().join("alpha.md").exists());
        assert_eq!(
            std::fs::read(dir.path().join("archive/alpha.md")).unwrap(),
            b"body"
        );
    }

    /// A file leaving a reserved name does not take that name into `archive/`,
    /// where OKF reserves it just as firmly (spec 3.1).
    #[test]
    fn reserved_names_do_not_follow_the_file_into_archive() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("log.md"), b"a concept, not a log").unwrap();
        std::fs::write(dir.path().join("index.md"), b"a concept, not an index").unwrap();
        archive_file_from(&root, &root, "log.md").unwrap();
        archive_file_from(&root, &root, "index.md").unwrap();
        assert!(!dir.path().join("archive/log.md").exists());
        assert!(!dir.path().join("archive/index.md").exists());
        assert_eq!(
            std::fs::read(dir.path().join("archive/log-page.md")).unwrap(),
            b"a concept, not a log"
        );
        assert_eq!(
            std::fs::read(dir.path().join("archive/index-page.md")).unwrap(),
            b"a concept, not an index"
        );
    }

    /// A second archived copy never overwrites the first.
    #[test]
    fn suffixes_past_an_occupied_destination() {
        let (dir, root) = temp_root();
        std::fs::create_dir(dir.path().join("archive")).unwrap();
        std::fs::write(dir.path().join("archive/log-page.md"), b"older").unwrap();
        std::fs::write(dir.path().join("log.md"), b"newer").unwrap();
        archive_file_from(&root, &root, "log.md").unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("archive/log-page.md")).unwrap(),
            b"older"
        );
        assert_eq!(
            std::fs::read(dir.path().join("archive/log-page-2.md")).unwrap(),
            b"newer"
        );
    }

    /// A file in a subdirectory archives into the ROOT's `archive/`, which is
    /// how `_sources/` stubs reach the same visible place pages do.
    #[test]
    fn archives_from_a_subdirectory_into_the_root_archive() {
        let (dir, root) = temp_root();
        std::fs::create_dir(dir.path().join("_sources")).unwrap();
        std::fs::write(dir.path().join("_sources/note.md"), b"mine").unwrap();
        let sources = root.open_dir("_sources").unwrap();
        archive_file_from(&root, &sources, "note.md").unwrap();
        assert!(!dir.path().join("_sources/note.md").exists());
        assert_eq!(
            std::fs::read(dir.path().join("archive/note.md")).unwrap(),
            b"mine"
        );
    }

    /// Already gone is success, not an error.
    #[test]
    fn missing_file_is_success() {
        let (_dir, root) = temp_root();
        archive_file_from(&root, &root, "gone.md").unwrap();
    }

    /// A destination created after the name was settled on -- a sync client
    /// restoring an older archived copy, say -- is not replaced. The file
    /// lands under the next suffix instead.
    #[test]
    fn a_destination_created_after_the_name_is_chosen_is_not_replaced() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("alpha.md"), b"the page").unwrap();
        let archive_path = dir.path().join("archive");
        let mut created = false;
        archive_file_from_with_hook(&root, &root, "alpha.md", &mut |candidate| {
            if !created {
                assert_eq!(
                    candidate, "alpha.md",
                    "the first candidate is the plain name"
                );
                std::fs::write(archive_path.join(candidate), b"arrived meanwhile").unwrap();
                created = true;
            }
        })
        .unwrap();
        assert!(created, "the hook ran before the first placement");
        assert_eq!(
            std::fs::read(dir.path().join("archive/alpha.md")).unwrap(),
            b"arrived meanwhile",
            "the destination that appeared is untouched"
        );
        assert_eq!(
            std::fs::read(dir.path().join("archive/alpha-2.md")).unwrap(),
            b"the page",
            "the archived file took the next suffix"
        );
        assert!(!dir.path().join("alpha.md").exists());
        assert_eq!(
            std::fs::read_dir(dir.path().join("archive"))
                .unwrap()
                .count(),
            2,
            "no private name is left behind"
        );
    }

    /// Every suffix taken: the same refusal as before, and the file is still
    /// in `archive/` (under a private name) rather than lost.
    #[test]
    fn exhausted_suffixes_refuse_without_losing_the_file() {
        let (dir, root) = temp_root();
        std::fs::create_dir(dir.path().join("archive")).unwrap();
        std::fs::write(dir.path().join("archive/alpha.md"), b"1").unwrap();
        for n in 2..=MAX_ARCHIVE_ATTEMPTS {
            std::fs::write(dir.path().join(format!("archive/alpha-{n}.md")), b"n").unwrap();
        }
        std::fs::write(dir.path().join("alpha.md"), b"the page").unwrap();
        let error = archive_file_from(&root, &root, "alpha.md").unwrap_err();
        assert!(
            matches!(&error, WenlanError::Conflict(m) if m == "page_archive_target_exhausted"),
            "{error:?}"
        );
        assert!(!dir.path().join("alpha.md").exists());
        let kept: Vec<_> = std::fs::read_dir(dir.path().join("archive"))
            .unwrap()
            .map(|e| e.unwrap())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".wenlan-aside-")
            })
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read(kept[0].path()).unwrap(), b"the page");
    }

    /// A directory where a file was expected is still a conflict, untouched.
    #[test]
    fn a_directory_is_a_conflict_and_stays_put() {
        let (dir, root) = temp_root();
        std::fs::create_dir(dir.path().join("alpha.md")).unwrap();
        let error = archive_file_from(&root, &root, "alpha.md").unwrap_err();
        assert!(matches!(error, WenlanError::Conflict(_)), "{error:?}");
        assert!(dir.path().join("alpha.md").is_dir());
    }

    /// An edit set aside by the stub GC archives under the name it had.
    #[test]
    fn an_aside_archives_under_its_original_name() {
        let (dir, root) = temp_root();
        std::fs::create_dir(dir.path().join("_sources")).unwrap();
        let sources = root.open_dir("_sources").unwrap();
        std::fs::write(dir.path().join("_sources/.wenlan-aside-x.tmp"), b"edit").unwrap();
        archive_aside_from(&root, &sources, ".wenlan-aside-x.tmp", "mem_1.md").unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("archive/mem_1.md")).unwrap(),
            b"edit"
        );
        assert!(!dir.path().join("_sources/.wenlan-aside-x.tmp").exists());
    }
}
