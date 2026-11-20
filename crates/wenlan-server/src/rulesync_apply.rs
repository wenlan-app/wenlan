// SPDX-License-Identifier: Apache-2.0
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
    pub(crate) fn of(metadata: &Metadata) -> Self {
        Self(metadata.permissions().mode() & 0o7777)
    }

    #[cfg(windows)]
    pub(crate) fn of(metadata: &Metadata) -> Self {
        Self(u32::from(metadata.permissions().readonly()))
    }

    #[cfg(unix)]
    fn set(self, file: &cap_std::fs::File) -> io::Result<()> {
        file.set_permissions(cap_std::fs::Permissions::from_mode(self.0 & 0o7777))?;
        if self != Self::of(&file.metadata()?) {
            return Err(io::Error::other(
                "could not preserve the planned file permissions",
            ));
        }
        Ok(())
    }

    pub(crate) fn permits_child_writes(self) -> bool {
        !cfg!(unix) || self.0 & 0o700 == 0o700
    }

    #[cfg(windows)]
    fn set(self, file: &cap_std::fs::File) -> io::Result<()> {
        let mut permissions = file.metadata()?.permissions();
        permissions.set_readonly(self.0 != 0);
        file.set_permissions(permissions)
    }
}

impl std::fmt::Display for FileMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(unix)]
        {
            write!(f, "{:04o}", self.0)
        }
        #[cfg(windows)]
        f.write_str(if self.0 != 0 { "read-only" } else { "writable" })
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
    Directory {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<FileMode>,
    },
}

impl<Content: PartialEq> PartialEq for Snapshot<Content> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Absent, Self::Absent) => true,
            (Self::Directory { mode }, Self::Directory { mode: other }) => mode == other,
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

    pub(crate) fn file_with_mode(bytes: Vec<u8>, mode: u32) -> Self {
        Self::file_with_permissions(bytes, FileMode(mode))
    }

    pub(crate) fn file_with_permissions(bytes: Vec<u8>, mode: FileMode) -> Self {
        Self::File { bytes, mode }
    }

    pub(crate) fn directory() -> Self {
        Self::Directory { mode: None }
    }

    pub(crate) fn directory_with_mode(mode: u32) -> Self {
        Self::directory_with_permissions(FileMode(mode))
    }

    pub(crate) fn directory_with_permissions(mode: FileMode) -> Self {
        Self::Directory { mode: Some(mode) }
    }

    pub(crate) fn is_directory(&self) -> bool {
        matches!(self, Self::Directory { .. })
    }

    pub(crate) fn matches(&self, actual: &Self) -> bool {
        // Earlier journals recorded only the type of a directory.
        matches!(self, Self::Directory { mode: None }) && actual.is_directory() || self == actual
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

pub(crate) struct ApplyOptions<'a> {
    pub(crate) allow_symlink: bool,
    pub(crate) displaced: Option<(&'a FileRoot, &'a Path)>,
    pub(crate) stage_id: Option<&'a str>,
    pub(crate) verify_source: Option<&'a dyn Fn() -> io::Result<bool>>,
}

pub(crate) struct FileRoot {
    dir: Dir,
    harness: Option<HarnessGuard>,
    tool_home: bool,
    private_directories: bool,
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
            private_directories: false,
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

    pub(crate) fn with_private_directories(mut self) -> Self {
        self.private_directories = true;
        self
    }

    pub(crate) fn open_private_lock(&self) -> io::Result<std::fs::File> {
        #[cfg(unix)]
        {
            let mode = self.dir.dir_metadata()?.permissions().mode();
            if mode & 0o077 != 0 {
                // A capability may use O_PATH on Linux, so open the same directory
                // on a descriptor that supports chmod and sync.
                let directory = self.dir.open(".")?;
                directory.set_permissions(cap_std::fs::Permissions::from_mode(mode & 0o7700))?;
                directory.sync_all()?;
            }
        }
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        options.mode(0o600);
        Ok(self.dir.open_with("personal.lock", &options)?.into_std())
    }

    /// On writes, repair a matching existing file on the descriptor used to
    /// compare its contents. Reads always validate the descriptor before use.
    pub(crate) fn read_private(
        &self,
        path: &Path,
        repair_if_matches: Option<&[u8]>,
    ) -> io::Result<Snapshot> {
        let (relative_parent, name) = entry_path(path)?;
        let Some(parent) = self.open_parent(relative_parent, false)? else {
            return Ok(Snapshot::Absent);
        };
        read_leaf_with(&parent, name, false, |file, metadata| {
            if repair_if_matches.is_none() {
                require_private_permissions(metadata, "personal rules file")?;
            }
            let bytes = read_file_contents(file)?;
            #[cfg(unix)]
            if repair_if_matches == Some(bytes.as_slice())
                && metadata.permissions().mode() & 0o7777 != 0o600
            {
                if cap_std::fs::MetadataExt::nlink(metadata) != 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "cannot make a hard-linked personal rules file private",
                    ));
                }
                file.set_permissions(cap_std::fs::Permissions::from_mode(0o600))?;
                file.sync_all()?;
            }
            Ok(bytes)
        })
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
    /// Internal state belongs exclusively to Wenlan. Tool files additionally
    /// use a journaled recovery path for the inode held by a running editor.
    pub(crate) fn apply(
        &self,
        path: &Path,
        seen: &Snapshot,
        desired: &Snapshot,
        allow_symlink: bool,
    ) -> io::Result<ApplyOutcome> {
        self.apply_with(
            path,
            seen,
            desired,
            ApplyOptions {
                allow_symlink,
                displaced: None,
                stage_id: None,
                verify_source: None,
            },
        )
    }

    pub(crate) fn apply_with(
        &self,
        path: &Path,
        seen: &Snapshot,
        desired: &Snapshot,
        options: ApplyOptions<'_>,
    ) -> io::Result<ApplyOutcome> {
        self.apply_inner(path, seen, desired, options, || {})
    }

    fn apply_inner(
        &self,
        path: &Path,
        seen: &Snapshot,
        desired: &Snapshot,
        options: ApplyOptions<'_>,
        before_commit: impl FnOnce(),
    ) -> io::Result<ApplyOutcome> {
        let (relative_parent, name) = entry_path(path)?;
        self.check_tool_path(path)?;
        if let Snapshot::File { bytes, .. } = desired {
            check_file_size(bytes.len() as u64)?;
        }
        if !options.allow_symlink
            && (matches!(seen, Snapshot::Symlink { .. })
                || matches!(desired, Snapshot::Symlink { .. }))
        {
            return Err(symlink_denied());
        }
        let create = *seen == Snapshot::Absent
            && *desired != Snapshot::Absent
            && !(self.tool_home && options.stage_id.is_some());
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
        if !seen.matches(&read_leaf(&parent, name, true)?) {
            return Ok(ApplyOutcome::ChangedSincePlan);
        }
        if desired.matches(seen) {
            return Ok(ApplyOutcome::Applied);
        }
        let displaced = recovery_location(&parent, options.displaced)?;
        let mut staged = Stage::new(&parent, options.stage_id)?;
        let result = (|| {
            staged.write(desired)?;
            if options.stage_id.is_some() && matches!(desired, Snapshot::File { .. }) {
                staged.dir.hard_link(STAGED, &staged.dir, INCOMING)?;
            }
            #[cfg(test)]
            crash_at("staged", name);
            before_commit();
            let Some(current_parent) = self.open_parent(relative_parent, false)? else {
                return Ok(ApplyOutcome::ChangedSincePlan);
            };
            self.check_directory_leaf(&current_parent, name)?;
            if !same_directory(&parent, &current_parent)? || !matches_leaf(&parent, name, seen)? {
                return Ok(ApplyOutcome::ChangedSincePlan);
            }
            if let Some(verify) = options.verify_source {
                if !verify()? || !matches_leaf(&parent, name, seen)? {
                    return Ok(ApplyOutcome::ChangedSincePlan);
                }
            }
            match commit(&mut staged, name, seen, desired, displaced.as_ref()) {
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
        })();
        if !staged.preserve {
            staged.preserve = true;
            staged.clean()?;
        }
        result
    }

    /// Inspect only a stage named in the batch saved before this mutation.
    pub(crate) fn recover_stage(
        &self,
        path: &Path,
        seen: &Snapshot,
        desired: &Snapshot,
        actual: &Snapshot,
        options: ApplyOptions<'_>,
    ) -> io::Result<()> {
        let Some(id) = options.stage_id else {
            return Ok(());
        };
        let (relative_parent, _) = entry_path(path)?;
        let Some(parent) = self.open_parent(relative_parent, false)? else {
            return Ok(());
        };
        let name = stage_name(id)?;
        let dir = match parent.open_dir_nofollow(&name) {
            Ok(dir) => dir,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let entry = read_leaf(&dir, Path::new(STAGED), true)?;
        let incoming = match dir.symlink_metadata(INCOMING) {
            Ok(metadata) if metadata.is_file() => dir
                .symlink_metadata(STAGED)
                .is_ok_and(|staged| staged.is_file() && same_identity(&staged, &metadata)),
            Ok(_) => return Err(io::Error::other("invalid rulesync staging entry")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // The incoming hardlink is created before replacement. If
                // the original is still in place, a partial incoming write
                // may be the only entry.
                desired.matches(&entry)
                    || (seen.matches(actual) && matches!(desired, Snapshot::File { .. }))
            }
            Err(error) => return Err(error),
        };
        if entry != Snapshot::Absent && !incoming {
            if matches!(entry, Snapshot::File { .. }) {
                let displaced = recovery_location(&parent, options.displaced)?
                    .ok_or_else(|| io::Error::other("a displaced file requires manual recovery"))?;
                move_displaced(&dir, &displaced)?;
                #[cfg(unix)]
                displaced.0.open(".")?.sync_all()?;
            } else if !matches_leaf(&dir, Path::new(STAGED), seen)? || !desired.matches(actual) {
                return Err(io::Error::other(format!(
                    "an unexpected entry is saved in {name}/{STAGED}. Check it before retrying"
                )));
            }
        }
        let stage = Stage {
            parent: &parent,
            dir,
            name,
            preserve: true,
        };
        stage.clean()
    }

    fn open_parent(&self, path: &Path, create: bool) -> io::Result<Option<Dir>> {
        self.check_tool_path(path)?;
        let mut parent = self.dir.try_clone()?;
        if let Some(guard) = &self.harness {
            guard.check(&parent.dir_metadata()?)?;
        }
        if self.private_directories {
            require_private_permissions(&parent.dir_metadata()?, "rulesync data directory")?;
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
            if self.private_directories {
                require_private_permissions(&parent.dir_metadata()?, "rulesync data directory")?;
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
    Ok(same_identity(&first, &second))
}

fn same_identity(first: &Metadata, second: &Metadata) -> bool {
    (first.dev(), first.ino()) == (second.dev(), second.ino())
}

fn create_directory(parent: &Dir, name: &Path) -> io::Result<()> {
    #[allow(unused_mut)]
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    parent.create_dir_with(name, &builder)
}

fn create_planned_directory(parent: &Dir, name: &Path, mode: Option<FileMode>) -> io::Result<()> {
    create_directory(parent, name)?;
    if let Some(mode) = mode {
        let dir = parent.open_dir_nofollow(name)?;
        #[cfg(unix)]
        {
            let file = dir.open(".")?;
            mode.set(&file)?;
            file.sync_all()?;
        }
        #[cfg(windows)]
        {
            let mut permissions = dir.dir_metadata()?.permissions();
            permissions.set_readonly(mode.0 != 0);
            dir.set_permissions(".", permissions)?;
        }
    }
    Ok(())
}

fn require_private_permissions(metadata: &Metadata, description: &str) -> io::Result<()> {
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{description} must have private permissions"),
        ));
    }
    // On Windows, rulesync inherits the data root's ACL. The portable
    // permissions API only exposes the read-only attribute there.
    #[cfg(windows)]
    let _ = (metadata, description);
    Ok(())
}

fn read_leaf(parent: &Dir, name: &Path, allow_symlink: bool) -> io::Result<Snapshot> {
    read_leaf_with(parent, name, allow_symlink, |file, _| {
        read_file_contents(file)
    })
}

fn read_leaf_with(
    parent: &Dir,
    name: &Path,
    allow_symlink: bool,
    read: impl FnOnce(&cap_std::fs::File, &Metadata) -> io::Result<Vec<u8>>,
) -> io::Result<Snapshot> {
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
        return Ok(Snapshot::Directory {
            mode: Some(FileMode::of(&metadata)),
        });
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
    Ok(Snapshot::File {
        bytes: read(&file, &metadata)?,
        mode: FileMode::of(&metadata),
    })
}

fn read_file_contents(file: &cap_std::fs::File) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(MAX_ADOPTED_SKILL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    check_file_size(bytes.len() as u64)?;
    Ok(bytes)
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
    if !expected.matches(&read_leaf(parent, name, true)?) {
        return Ok(false);
    }
    if expected.is_directory() {
        let directory = parent.open_dir_nofollow(name)?;
        return Ok(directory.entries()?.next().transpose()?.is_none());
    }
    Ok(true)
}

const STAGED: &str = "entry";
const INCOMING: &str = "incoming";

fn stage_name(id: &str) -> io::Result<String> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_path());
    }
    Ok(format!(".wenlan-rules-stage-{id}"))
}

fn recovery_location<'a>(
    parent: &Dir,
    option: Option<(&FileRoot, &'a Path)>,
) -> io::Result<Option<(Dir, &'a Path)>> {
    option
        .map(|(root, path)| {
            let (directory, name) = entry_path(path)?;
            let directory = root.open_parent(directory, false)?.ok_or_else(invalid_path)?;
            if directory.dir_metadata()?.dev() != parent.dir_metadata()?.dev() {
                return Err(io::Error::new(
                    io::ErrorKind::CrossesDevices,
                    "place Wenlan's rulesync data on the same volume as this tool file to keep concurrent saves recoverable",
                ));
            }
            Ok((directory, name))
        })
        .transpose()
}

fn move_displaced(dir: &Dir, displaced: &(Dir, &Path)) -> io::Result<()> {
    let (directory, name) = displaced;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    rustix::fs::renameat_with(
        dir,
        STAGED,
        directory,
        *name,
        rustix::fs::RenameFlags::NOREPLACE,
    )?;
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    match directory.symlink_metadata(name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            dir.rename(STAGED, directory, *name)?
        }
        Err(error) => return Err(error),
        Ok(_) => return Err(io::Error::from(io::ErrorKind::AlreadyExists)),
    }
    Ok(())
}

#[cfg(test)]
fn crash_at(point: &str, name: &Path) {
    if std::env::var("WENLAN_TEST_FILE_APPLY_CRASH")
        .ok()
        .as_deref()
        == Some(&format!("{point}:{}", name.display()))
    {
        // Simulate a daemon exit before any destructors or batch checkpoints.
        std::process::exit(76);
    }
}

/// The private directory holds the staged or displaced leaf and may also
/// contain a second link identifying the incoming file.
/// Cleanup never recurses, even when a concurrent save adds to a directory.
struct Stage<'a> {
    parent: &'a Dir,
    dir: Dir,
    name: String,
    preserve: bool,
}

impl<'a> Stage<'a> {
    fn new(parent: &'a Dir, id: Option<&str>) -> io::Result<Self> {
        let name = match id {
            Some(id) => stage_name(id)?,
            None => stage_name(&uuid::Uuid::new_v4().simple().to_string())?,
        };
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
            Snapshot::Directory { mode } => {
                create_planned_directory(&self.dir, Path::new(STAGED), *mode)
            }
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

    fn clean(&self) -> io::Result<()> {
        for leaf in [STAGED, INCOMING] {
            match self.dir.symlink_metadata(leaf) {
                Ok(metadata) if metadata.is_dir() => self.dir.remove_dir(leaf)?,
                Ok(_) => self.dir.remove_file_or_symlink(leaf)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error),
            }
        }
        self.dir.try_clone()?.remove_open_dir()
    }
}

impl Drop for Stage<'_> {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = self.clean();
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn commit(
    stage: &mut Stage<'_>,
    name: &Path,
    seen: &Snapshot,
    desired: &Snapshot,
    displaced: Option<&(Dir, &Path)>,
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
    #[cfg(test)]
    crash_at("displaced", name);

    // From this point the stage contains the displaced entry. Leave it
    // reachable if we cannot establish that it is safe to discard.
    stage.preserve = true;
    let verify = matches_leaf(&stage.dir, held, seen);
    if matches!(verify, Ok(true)) {
        if let Some(displaced) = displaced {
            // Keep the same inode reachable after verification. An editor may
            // still write through a handle it opened before the exchange.
            move_displaced(&stage.dir, displaced)?;
        }
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
    displaced: Option<&(Dir, &Path)>,
) -> io::Result<ApplyOutcome> {
    if let Some((directory, recovery)) = displaced {
        // Link the original before replacing the Windows destination.
        // Existing handles retain the same object even after replacement.
        stage.parent.hard_link(name, directory, recovery)?;
        if !matches_leaf(stage.parent, name, seen)? {
            return Ok(ApplyOutcome::ChangedSincePlan);
        }
    }
    match (seen, desired) {
        (Snapshot::Absent, Snapshot::File { .. }) => {
            stage.dir.hard_link(STAGED, stage.parent, name)?;
        }
        (Snapshot::Absent, Snapshot::Directory { mode }) => {
            create_planned_directory(stage.parent, name, *mode)?
        }
        (Snapshot::Directory { .. }, Snapshot::Absent) => stage.parent.remove_dir(name)?,
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

    fn direct(allow_symlink: bool) -> ApplyOptions<'static> {
        ApplyOptions {
            allow_symlink,
            displaced: None,
            stage_id: None,
            verify_source: None,
        }
    }

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

    #[cfg(unix)]
    #[test]
    fn run9_adoption_rechecks_the_link_referent_after_staging() {
        use std::os::unix::fs::symlink;
        use wenlan_core::rulesync_adopt::{discover, AdoptionTarget};

        let (scratch, root) = root();
        let path = Path::new("AGENTS.md");
        let installed = scratch.path().join(path);
        let source = scratch.path().join("source.md");
        fs::write(&source, b"Approved rules.\n").unwrap();
        symlink("source.md", &installed).unwrap();
        let adoption = discover(
            scratch.path(),
            &[AdoptionTarget {
                tool: "codexcli".into(),
                rules_path: Some(path.into()),
                skills_path: None,
            }],
        )
        .unwrap();
        let verify = || {
            adoption.links[path]
                .matches(scratch.path(), path)
                .map_err(io::Error::other)
        };
        let seen = root.read(path, true).unwrap();
        let desired = Snapshot::file(b"Approved rules.\n".to_vec());
        assert_eq!(
            root.apply_inner(
                path,
                &seen,
                &desired,
                ApplyOptions {
                    verify_source: Some(&verify),
                    ..direct(true)
                },
                || fs::write(source, b"Saved while staging.\n").unwrap(),
            )
            .unwrap(),
            ApplyOutcome::ChangedSincePlan,
        );
        assert_eq!(fs::read_link(&installed).unwrap().as_os_str(), "source.md");
        assert_eq!(fs::read(installed).unwrap(), b"Saved while staging.\n");
        assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 2);
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
            let result = root.apply_inner(name, &seen, &desired, direct(false), || {
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
        let result = root.apply_inner(name, &Snapshot::Absent, &desired, direct(false), || {
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
            root.apply(name, &seen, &Snapshot::directory(), true)
                .unwrap(),
            ApplyOutcome::Applied
        );
        assert!(root.read(name, false).unwrap().is_directory());
        let late_child = path.join("personal-notes");
        let result = root.apply_inner(name, &Snapshot::directory(), &seen, direct(true), || {
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
            root.apply(name, &Snapshot::directory(), &seen, true)
                .unwrap(),
            ApplyOutcome::Applied
        );
        assert_eq!(fs::read_link(path).unwrap(), referent);
        assert_eq!(
            fs::read_dir(scratch.path().join("skills")).unwrap().count(),
            1
        );
    }
}
