//! Apply a planned change within an opened directory. Callers serialize their
//! writes. Adoption also records the plan and backup before calling `apply`.

use cap_fs_ext::{DirExt as _, FollowSymlinks, MetadataExt as _, OpenOptionsFollowExt as _};
use cap_std::fs::{Dir, DirBuilder, Metadata, OpenOptions};
#[cfg(unix)]
use cap_std::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use wenlan_core::rulesync_adopt::MAX_ADOPTED_SKILL_BYTES;
use wenlan_core::rulesync_personal::{is_harness_path, HarnessGuard};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileMode(u32);

impl FileMode {
    #[cfg(unix)]
    fn of(metadata: &Metadata) -> Self {
        Self(metadata.permissions().mode() & 0o777)
    }

    #[cfg(windows)]
    fn of(metadata: &Metadata) -> Self {
        Self(u32::from(metadata.permissions().readonly()))
    }

    #[cfg(unix)]
    fn set(self, file: &cap_std::fs::File) -> io::Result<()> {
        file.set_permissions(cap_std::fs::Permissions::from_mode(self.0 & 0o777))
    }

    #[cfg(windows)]
    fn set(self, file: &cap_std::fs::File) -> io::Result<()> {
        let mut permissions = file.metadata()?.permissions();
        permissions.set_readonly(self.0 != 0);
        file.set_permissions(permissions)
    }
}

#[derive(Debug, Clone, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Snapshot<Content = Vec<u8>> {
    Absent,
    File {
        bytes: Content,
        mode: FileMode,
    },
    Symlink {
        #[serde(with = "link_target")]
        target: PathBuf,
        // Windows distinguishes these types in the link itself. On Unix the
        // link carries only its target, and this is always false when read.
        directory: bool,
    },
    /// Directory entry only. The applier can create or replace an empty
    /// directory. Its children are planned and applied independently.
    Directory,
}

impl<Content: PartialEq> PartialEq for Snapshot<Content> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Absent, Self::Absent) | (Self::Directory, Self::Directory) => true,
            (
                Self::File { bytes, mode },
                Self::File {
                    bytes: other_bytes,
                    mode: other_mode,
                },
            ) => bytes == other_bytes && mode == other_mode,
            (
                Self::Symlink { target, directory },
                Self::Symlink {
                    target: other_target,
                    directory: other_directory,
                },
            ) => target.as_os_str() == other_target.as_os_str() && directory == other_directory,
            _ => false,
        }
    }
}

impl Snapshot {
    pub(crate) fn file(bytes: Vec<u8>) -> Self {
        Self::File {
            bytes,
            mode: FileMode(if cfg!(unix) { 0o600 } else { 0 }),
        }
    }

    #[cfg(unix)]
    pub(crate) fn file_with_mode(bytes: Vec<u8>, mode: u32) -> Self {
        Self::File {
            bytes,
            mode: FileMode(mode & 0o777),
        }
    }
}

// Keep the original native target, including dangling, absolute, and
// non-UTF-8 paths. The serialized backup is local to the originating OS.
mod link_target {
    use super::*;
    use std::ffi::OsString;

    #[cfg(unix)]
    pub(super) fn serialize<S: serde::Serializer>(
        path: &Path,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        use std::os::unix::ffi::OsStrExt as _;
        serializer.serialize_bytes(path.as_os_str().as_bytes())
    }

    #[cfg(unix)]
    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PathBuf, D::Error> {
        use std::os::unix::ffi::OsStringExt as _;
        Vec::<u8>::deserialize(deserializer).map(|bytes| OsString::from_vec(bytes).into())
    }

    #[cfg(windows)]
    pub(super) fn serialize<S: serde::Serializer>(
        path: &Path,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        use std::os::windows::ffi::OsStrExt as _;
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide.serialize(serializer)
    }

    #[cfg(windows)]
    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PathBuf, D::Error> {
        use std::os::windows::ffi::OsStringExt as _;
        Vec::<u16>::deserialize(deserializer).map(|wide| OsString::from_wide(&wide).into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyOutcome {
    Applied,
    ChangedSincePlan,
}

pub(crate) struct FileRoot {
    dir: Dir,
    harness: Option<HarnessGuard>,
    tool_home: bool,
}

impl FileRoot {
    /// The caller selects the absolute root. Open the root itself and each
    /// descendant directory without following a link at that name.
    pub(crate) fn open(root: &Path) -> io::Result<Self> {
        let (Some(parent), Some(name)) = (root.parent(), root.file_name()) else {
            return Err(invalid_path());
        };
        if !root.is_absolute() {
            return Err(invalid_path());
        }
        let ambient_parent = Dir::open_ambient_dir(parent, cap_std::ambient_authority())?;
        Ok(Self {
            dir: ambient_parent.open_dir_nofollow(Path::new(name))?,
            harness: None,
            tool_home: false,
        })
    }

    pub(crate) fn open_home(home: &Path) -> io::Result<Self> {
        let mut root = Self::open(home)?;
        root.harness = Some(HarnessGuard::new(home)?);
        root.tool_home = true;
        Ok(root)
    }

    pub(crate) fn open_data(home: &Path, path: &Path) -> io::Result<Self> {
        let guard = HarnessGuard::new(home)?;
        guard.check_absolute(path)?;
        let mut root = Self::open(path)?;
        guard.check(&root.dir.dir_metadata()?)?;
        root.harness = Some(guard);
        Ok(root)
    }

    pub(crate) fn read(&self, path: &Path, allow_symlink: bool) -> io::Result<Snapshot> {
        let (relative_parent, name) = entry_path(path)?;
        match self.open_parent(relative_parent, false)? {
            Some(parent) => read_leaf(&parent, name, allow_symlink),
            None => Ok(Snapshot::Absent),
        }
    }

    pub(crate) fn sync_directory(&self, path: &Path) -> io::Result<()> {
        #[cfg(unix)]
        if let Some(directory) = self.open_parent(path, false)? {
            // On Linux the capability itself can be an O_PATH descriptor.
            directory.open(".")?.sync_all()?;
        }
        #[cfg(not(unix))]
        let _ = path;
        Ok(())
    }

    pub(crate) fn remove_private_directory(&self, path: &Path) -> io::Result<()> {
        entry_path(path)?;
        match self.open_parent(path, false)? {
            Some(directory) => directory.remove_open_dir_all(),
            None => Ok(()),
        }
    }

    /// Check the current entry against the planned snapshot before committing.
    /// On macOS and Linux, retain and recheck the exact displaced entry, so a
    /// save that reaches the pathname during the final check is recoverable.
    /// An editor that writes through an open handle after that entry has been
    /// checked and removed is outside this pathname-level guarantee. Windows
    /// replacement and removal have a final check-to-mutation interval.
    pub(crate) fn apply(
        &self,
        path: &Path,
        seen: &Snapshot,
        desired: &Snapshot,
        allow_symlink: bool,
    ) -> io::Result<ApplyOutcome> {
        self.apply_inner(path, seen, desired, allow_symlink, || {})
    }

    fn apply_inner(
        &self,
        path: &Path,
        seen: &Snapshot,
        desired: &Snapshot,
        allow_symlink: bool,
        before_commit: impl FnOnce(),
    ) -> io::Result<ApplyOutcome> {
        let (relative_parent, name) = entry_path(path)?;
        self.check_tool_path(path)?;
        if let Snapshot::File { bytes, .. } = desired {
            check_file_size(bytes.len() as u64)?;
        }
        if !allow_symlink
            && (matches!(seen, Snapshot::Symlink { .. })
                || matches!(desired, Snapshot::Symlink { .. }))
        {
            return Err(symlink_denied());
        }
        let create = *seen == Snapshot::Absent && *desired != Snapshot::Absent;
        let Some(parent) = self.open_parent(relative_parent, create)? else {
            return Ok(
                if *seen == Snapshot::Absent && *desired == Snapshot::Absent {
                    ApplyOutcome::Applied
                } else {
                    ApplyOutcome::ChangedSincePlan
                },
            );
        };
        self.check_directory_leaf(&parent, name)?;
        if read_leaf(&parent, name, true)? != *seen {
            return Ok(ApplyOutcome::ChangedSincePlan);
        }
        if seen == desired {
            return Ok(ApplyOutcome::Applied);
        }
        let mut staged = Stage::new(&parent)?;
        staged.write(desired)?;
        let Some(current_parent) = self.open_parent(relative_parent, false)? else {
            return Ok(ApplyOutcome::ChangedSincePlan);
        };
        self.check_directory_leaf(&current_parent, name)?;
        if !same_directory(&parent, &current_parent)? || !matches_leaf(&parent, name, seen)? {
            return Ok(ApplyOutcome::ChangedSincePlan);
        }
        before_commit();

        match commit(&mut staged, name, seen, desired) {
            Err(error)
                if !staged.preserve
                    && matches!(
                        error.kind(),
                        io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound
                    ) =>
            {
                Ok(ApplyOutcome::ChangedSincePlan)
            }
            other => other,
        }
    }

    fn open_parent(&self, path: &Path, create: bool) -> io::Result<Option<Dir>> {
        self.check_tool_path(path)?;
        let mut parent = self.dir.try_clone()?;
        if let Some(guard) = &self.harness {
            guard.check(&parent.dir_metadata()?)?;
        }
        for part in path.components() {
            let Component::Normal(part) = part else {
                return Err(invalid_path());
            };
            let part = Path::new(part);
            parent = match parent.open_dir_nofollow(part) {
                Ok(dir) => dir,
                Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
                    match create_directory(&parent, part) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error),
                    }
                    parent.open_dir_nofollow(part)?
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            if let Some(guard) = &self.harness {
                guard.check(&parent.dir_metadata()?)?;
            }
        }
        Ok(Some(parent))
    }

    fn check_tool_path(&self, path: &Path) -> io::Result<()> {
        if self.tool_home && is_harness_path(path) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "inside the read-only ~/.agents harness. Choose a path outside it",
            ));
        }
        Ok(())
    }

    fn check_directory_leaf(&self, parent: &Dir, name: &Path) -> io::Result<()> {
        if let Some(guard) = &self.harness {
            match parent.symlink_metadata(name) {
                Ok(metadata) if metadata.is_dir() => {
                    guard.check(&parent.open_dir_nofollow(name)?.dir_metadata()?)?;
                }
                Ok(_) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

fn invalid_path() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "expected a relative file path")
}

fn symlink_denied() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "symlink requires adoption")
}

fn entry_path(path: &Path) -> io::Result<(&Path, &Path)> {
    if !path.components().all(|part| match part {
        Component::Normal(name) => !cfg!(windows) || !name.as_encoded_bytes().contains(&b':'),
        _ => false,
    }) {
        return Err(invalid_path());
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Ok((parent, Path::new(name))),
        _ => Err(invalid_path()),
    }
}

fn same_directory(first: &Dir, second: &Dir) -> io::Result<bool> {
    let first = first.dir_metadata()?;
    let second = second.dir_metadata()?;
    Ok((first.dev(), first.ino()) == (second.dev(), second.ino()))
}

fn create_directory(parent: &Dir, name: &Path) -> io::Result<()> {
    #[allow(unused_mut)]
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    parent.create_dir_with(name, &builder)
}

fn read_leaf(parent: &Dir, name: &Path, allow_symlink: bool) -> io::Result<Snapshot> {
    let metadata = match parent.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Snapshot::Absent),
        Err(error) => return Err(error),
    };
    if metadata.is_symlink() {
        if !allow_symlink {
            return Err(symlink_denied());
        }
        #[cfg(windows)]
        let directory = {
            use cap_std::fs::MetadataExt as _;
            const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
            metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
        };
        #[cfg(unix)]
        let directory = false;
        return Ok(Snapshot::Symlink {
            target: parent.read_link_contents(name)?,
            directory,
        });
    }
    if metadata.is_dir() {
        return Ok(Snapshot::Directory);
    }
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported file type",
        ));
    }
    check_file_size(metadata.len())?;
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    // A FIFO substituted after the metadata check must not block the writer.
    options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    let file = parent.open_with(name, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file type changed",
        ));
    }
    check_file_size(metadata.len())?;
    let mut bytes = Vec::new();
    file.take(MAX_ADOPTED_SKILL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    check_file_size(bytes.len() as u64)?;
    Ok(Snapshot::File {
        bytes,
        mode: FileMode::of(&metadata),
    })
}

fn check_file_size(len: u64) -> io::Result<()> {
    if len > MAX_ADOPTED_SKILL_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!(
                "file exceeds the {} MiB rulesync limit. Move the large file aside before trying again",
                MAX_ADOPTED_SKILL_BYTES / (1024 * 1024)
            ),
        ));
    }
    Ok(())
}

fn matches_leaf(parent: &Dir, name: &Path, expected: &Snapshot) -> io::Result<bool> {
    if read_leaf(parent, name, true)? != *expected {
        return Ok(false);
    }
    if *expected == Snapshot::Directory {
        let directory = parent.open_dir_nofollow(name)?;
        return Ok(directory.entries()?.next().transpose()?.is_none());
    }
    Ok(true)
}

const STAGED: &str = "entry";

/// Its name is exclusive and the only entry is the staged or displaced leaf.
/// Cleanup never recurses, even when a concurrent save adds to a directory.
struct Stage<'a> {
    parent: &'a Dir,
    dir: Dir,
    name: String,
    preserve: bool,
}

impl<'a> Stage<'a> {
    fn new(parent: &'a Dir) -> io::Result<Self> {
        let name = format!(".wenlan-rules-stage-{}", uuid::Uuid::new_v4().simple());
        let path = Path::new(&name);
        create_directory(parent, path)?;
        let dir = parent.open_dir_nofollow(path)?;
        Ok(Self {
            parent,
            dir,
            name,
            preserve: false,
        })
    }

    fn write(&self, data: &Snapshot) -> io::Result<()> {
        match data {
            Snapshot::Absent => Ok(()),
            Snapshot::Directory => create_directory(&self.dir, Path::new(STAGED)),
            Snapshot::File { bytes, mode } => {
                let mut options = OpenOptions::new();
                options
                    .write(true)
                    .create_new(true)
                    .follow(FollowSymlinks::No);
                #[cfg(unix)]
                options.mode(0o600);
                let mut file = self.dir.open_with(STAGED, &options)?;
                file.write_all(bytes)?;
                mode.set(&file)?;
                file.sync_all()
            }
            Snapshot::Symlink { target, directory } => {
                #[cfg(unix)]
                {
                    let _ = directory;
                    self.dir.symlink_contents(target, STAGED)
                }
                #[cfg(windows)]
                if *directory {
                    self.dir.symlink_dir(target, STAGED)
                } else {
                    self.dir.symlink_file(target, STAGED)
                }
            }
        }
    }
}

impl Drop for Stage<'_> {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = self.dir.remove_file_or_symlink(STAGED);
            let _ = self.dir.remove_dir(STAGED);
            let _ = self.parent.remove_dir(&self.name);
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn commit(
    stage: &mut Stage<'_>,
    name: &Path,
    seen: &Snapshot,
    desired: &Snapshot,
) -> io::Result<ApplyOutcome> {
    use rustix::fs::{renameat_with, RenameFlags};

    let parent = stage.parent;
    let held = Path::new(STAGED);
    if *seen == Snapshot::Absent {
        renameat_with(&stage.dir, held, parent, name, RenameFlags::NOREPLACE)?;
        return Ok(ApplyOutcome::Applied);
    }
    if *desired == Snapshot::Absent {
        parent.rename(name, &stage.dir, held)?;
    } else {
        renameat_with(&stage.dir, held, parent, name, RenameFlags::EXCHANGE)?;
    }

    // From this point the stage contains the displaced entry. Leave it
    // reachable if we cannot establish that it is safe to discard.
    stage.preserve = true;
    let verify = matches_leaf(&stage.dir, held, seen);
    if matches!(verify, Ok(true)) {
        stage.preserve = false;
        return Ok(ApplyOutcome::Applied);
    }

    let restore = if *desired == Snapshot::Absent {
        renameat_with(&stage.dir, held, parent, name, RenameFlags::NOREPLACE)
            .map_err(io::Error::from)
    } else if matches!(matches_leaf(parent, name, desired), Ok(true)) {
        renameat_with(&stage.dir, held, parent, name, RenameFlags::EXCHANGE)
            .map_err(io::Error::from)
            .and_then(|()| {
                if matches_leaf(&stage.dir, held, desired)? {
                    Ok(())
                } else {
                    Err(io::Error::other("target changed during recovery"))
                }
            })
    } else {
        Err(io::Error::other("target changed during recovery"))
    };
    match restore {
        Ok(()) => {
            stage.preserve = false;
            verify.map(|_| ApplyOutcome::ChangedSincePlan)
        }
        Err(error) => Err(io::Error::other(format!(
            "{error}. Preserved the displaced entry at {}/{STAGED} beside {}",
            stage.name,
            name.display(),
        ))),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn commit(
    stage: &mut Stage<'_>,
    name: &Path,
    seen: &Snapshot,
    desired: &Snapshot,
) -> io::Result<ApplyOutcome> {
    match (seen, desired) {
        (Snapshot::Absent, Snapshot::File { .. }) => {
            stage.dir.hard_link(STAGED, stage.parent, name)?;
        }
        (Snapshot::Absent, Snapshot::Directory) => create_directory(stage.parent, name)?,
        (Snapshot::Directory, Snapshot::Absent) => stage.parent.remove_dir(name)?,
        (_, Snapshot::Absent) => stage.parent.remove_file_or_symlink(name)?,
        (Snapshot::File { .. }, Snapshot::File { .. }) => {
            stage.dir.rename(STAGED, stage.parent, name)?;
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "atomic replacement of this entry type is unavailable",
            ));
        }
    }
    Ok(ApplyOutcome::Applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn root() -> (TempDir, FileRoot) {
        let scratch = tempfile::tempdir().unwrap();
        let root = FileRoot::open(scratch.path()).unwrap();
        (scratch, root)
    }

    #[test]
    fn planned_edit_leaves_a_file_changed_since_the_plan() {
        let (scratch, root) = root();
        let name = Path::new("rules");
        let path = scratch.path().join(name);
        fs::write(&path, b"before planning\n").unwrap();
        let seen = root.read(name, false).unwrap();
        fs::write(&path, b"editor save\n").unwrap();
        let desired = Snapshot::file(b"generated rules\n".to_vec());

        assert_eq!(
            root.apply(name, &seen, &desired, false).unwrap(),
            ApplyOutcome::ChangedSincePlan,
        );
        assert_eq!(fs::read(path).unwrap(), b"editor save\n");
        assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 1);
    }

    #[test]
    fn new_file_can_be_restored_to_absence() {
        let (scratch, root) = root();
        let name = Path::new("tool/skills/rules");
        let before = root.read(name, false).unwrap();
        let desired = Snapshot::file(vec![0, 0xff, b'\n']);
        assert_eq!(before, Snapshot::Absent);
        assert!(!scratch.path().join("tool").exists());
        assert_eq!(
            root.apply(name, &before, &desired, false).unwrap(),
            ApplyOutcome::Applied,
        );
        assert_eq!(root.read(name, false).unwrap(), desired);
        assert_eq!(
            root.apply(name, &desired, &before, false).unwrap(),
            ApplyOutcome::Applied,
        );
        assert_eq!(root.read(name, false).unwrap(), Snapshot::Absent);
        assert_eq!(
            fs::read_dir(scratch.path().join("tool/skills"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn refuses_paths_traversing_outside_the_root() {
        let (scratch, root) = root();
        let desired = Snapshot::file(b"new".to_vec());
        for path in [
            Path::new("../outside"),
            Path::new("missing/../file"),
            Path::new(""),
        ] {
            let error = root
                .apply(path, &Snapshot::Absent, &desired, false)
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
        let absolute = scratch.path().join("target");
        assert_eq!(
            root.read(&absolute, false).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        #[cfg(windows)]
        assert_eq!(
            root.read(Path::new("data/rules:stream"), false)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn root_and_descendant_directory_links_are_not_opened() {
        use std::os::unix::fs::symlink;

        let (scratch, root) = root();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("rules"), b"outside").unwrap();
        symlink(outside.path(), scratch.path().join("linked")).unwrap();
        symlink(
            outside.path().join("missing"),
            scratch.path().join("dangling"),
        )
        .unwrap();
        assert!(FileRoot::open(&scratch.path().join("linked")).is_err());
        let desired = Snapshot::file(b"new".to_vec());
        for path in [Path::new("linked/rules"), Path::new("dangling/rules")] {
            assert!(root.read(path, true).is_err());
            assert!(root.apply(path, &Snapshot::Absent, &desired, true).is_err());
        }
        assert_eq!(fs::read(outside.path().join("rules")).unwrap(), b"outside");
        assert!(!outside.path().join("missing").exists());
    }

    #[cfg(all(unix, any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn adoption_and_restore_touch_only_the_symlink_leaf() {
        use std::os::unix::fs::symlink;

        let (scratch, root) = root();
        let outside = tempfile::tempdir().unwrap();
        let referent = outside.path().join("rules");
        fs::write(&referent, b"unmanaged").unwrap();
        let name = Path::new("rules");
        let path = scratch.path().join(name);
        symlink(&referent, &path).unwrap();
        assert_eq!(
            root.read(name, false).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        let seen = root.read(name, true).unwrap();
        let saved = serde_json::to_vec(&seen).unwrap();
        let desired = Snapshot::file(b"managed".to_vec());
        assert_eq!(
            root.apply(name, &seen, &desired, false).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            root.apply(name, &seen, &desired, true).unwrap(),
            ApplyOutcome::Applied
        );
        assert_eq!(root.read(name, false).unwrap(), desired);
        let restore = serde_json::from_slice(&saved).unwrap();
        assert_eq!(
            root.apply(name, &desired, &restore, true).unwrap(),
            ApplyOutcome::Applied
        );
        assert_eq!(fs::read_link(path).unwrap(), referent);
        assert_eq!(fs::read(referent).unwrap(), b"unmanaged");
    }

    #[cfg(unix)]
    #[test]
    fn backup_serializes_the_exact_native_symlink_target() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let link = Snapshot::Symlink {
            target: OsString::from_vec(b"../non-utf8-\xff".to_vec()).into(),
            directory: false,
        };
        assert_eq!(
            serde_json::from_slice::<Snapshot>(&serde_json::to_vec(&link).unwrap()).unwrap(),
            link
        );
        assert_ne!(
            Snapshot::<Vec<u8>>::Symlink {
                target: PathBuf::from("parent//target"),
                directory: false,
            },
            Snapshot::Symlink {
                target: PathBuf::from("parent/target"),
                directory: false,
            }
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn restores_original_file_bytes_and_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let (scratch, root) = root();
        let name = Path::new("rules");
        let path = scratch.path().join(name);
        let before = vec![0, 0xff, b'\n'];
        fs::write(&path, &before).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let seen = root.read(name, false).unwrap();
        let desired = Snapshot::file(b"generated".to_vec());
        assert_eq!(
            root.apply(name, &seen, &desired, false).unwrap(),
            ApplyOutcome::Applied
        );
        assert_eq!(
            root.apply(name, &desired, &seen, false).unwrap(),
            ApplyOutcome::Applied
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn pathname_save_during_final_check_survives_replacement_or_removal() {
        for desired in [Snapshot::file(b"generated".to_vec()), Snapshot::Absent] {
            let (scratch, root) = root();
            let name = Path::new("rules");
            let path = scratch.path().join(name);
            fs::write(&path, b"planned").unwrap();
            let seen = root.read(name, false).unwrap();
            let result = root.apply_inner(name, &seen, &desired, false, || {
                let save = scratch.path().join("editor-save");
                fs::write(&save, b"editor save").unwrap();
                fs::rename(save, &path).unwrap();
            });
            assert_eq!(result.unwrap(), ApplyOutcome::ChangedSincePlan);
            assert_eq!(fs::read(path).unwrap(), b"editor save");
            assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn late_creation_cannot_be_replaced_by_an_absent_plan() {
        let (scratch, root) = root();
        let name = Path::new("rules");
        let path = scratch.path().join(name);
        let desired = Snapshot::file(b"generated".to_vec());
        let result = root.apply_inner(name, &Snapshot::Absent, &desired, false, || {
            fs::write(&path, b"editor save").unwrap();
        });
        assert_eq!(result.unwrap(), ApplyOutcome::ChangedSincePlan);
        assert_eq!(fs::read(path).unwrap(), b"editor save");
        assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 1);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn directory_link_round_trips_and_late_children_survive_restore() {
        use std::os::unix::fs::symlink;

        let (scratch, root) = root();
        fs::create_dir(scratch.path().join("skills")).unwrap();
        fs::create_dir(scratch.path().join("source")).unwrap();
        let referent = Path::new("../source");
        let name = Path::new("skills/example");
        let path = scratch.path().join(name);
        symlink(referent, &path).unwrap();
        let seen = root.read(name, true).unwrap();
        assert_eq!(
            root.apply(name, &seen, &Snapshot::Directory, true).unwrap(),
            ApplyOutcome::Applied
        );
        assert_eq!(root.read(name, false).unwrap(), Snapshot::Directory);
        let late_child = path.join("personal-notes");
        let result = root.apply_inner(name, &Snapshot::Directory, &seen, true, || {
            fs::write(&late_child, b"new skill notes").unwrap();
        });
        assert_eq!(result.unwrap(), ApplyOutcome::ChangedSincePlan);
        assert_eq!(fs::read(&late_child).unwrap(), b"new skill notes");
        assert_eq!(
            fs::read_dir(scratch.path().join("source")).unwrap().count(),
            0
        );

        fs::remove_file(late_child).unwrap();
        assert_eq!(
            root.apply(name, &Snapshot::Directory, &seen, true).unwrap(),
            ApplyOutcome::Applied
        );
        assert_eq!(fs::read_link(path).unwrap(), referent);
        assert_eq!(
            fs::read_dir(scratch.path().join("skills")).unwrap().count(),
            1
        );
    }
}
