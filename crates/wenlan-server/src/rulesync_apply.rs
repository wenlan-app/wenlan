// SPDX-License-Identifier: Apache-2.0
//! Apply a planned change within an opened directory. Callers serialize their
//! writes. Adoption also records the plan and backup before calling `apply`.

use cap_fs_ext::{DirExt as _, FollowSymlinks, MetadataExt as _, OpenOptionsFollowExt as _};
use cap_std::fs::{Dir, DirBuilder, Metadata, OpenOptions};
#[cfg(unix)]
use cap_std::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;
use wenlan_core::rulesync_adopt::MAX_ADOPTED_SKILL_BYTES;
use wenlan_core::rulesync_adopt::SHARED_RULES;
use wenlan_core::rulesync_personal::{case_alias_in, is_harness_path, HarnessGuard};
#[cfg(windows)]
use wenlan_core::rulesync_personal::{
    check_windows_replacement, copy_windows_dacl, open_windows_tool_dir,
    prepare_windows_file_replacement, read_windows_owner, windows_access_error,
    windows_permissions_error,
};
use wenlan_core::rulesync_personal_files::{
    directory_entry_identity, directory_is_case_sensitive, is_personal_path, location_of,
    readonly_personal_file, require_ascii_entry_name, require_plain_personal_directory,
    require_plain_personal_file, NON_ASCII_PERSONAL_NAME,
};
use wenlan_core::rulesync_settings::personal::is_settings_path;

pub(crate) fn is_personal_entry(path: &Path) -> bool {
    is_personal_path(path) || is_settings_path(path)
}

const PERSONAL_LOCK: &str = "personal.lock";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileMode(u32);

impl FileMode {
    pub(crate) fn grants_more_read_access_than(self, source: Self) -> bool {
        cfg!(unix) && self.0 & !source.0 & 0o044 != 0
    }

    pub(crate) fn description(self) -> String {
        if cfg!(unix) {
            format!("{:04o}", self.0)
        } else if self.0 == 0 {
            "writable".into()
        } else {
            "read-only".into()
        }
    }

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

    pub(crate) fn with_child_writes(self) -> Self {
        Self(if cfg!(unix) { self.0 | 0o700 } else { 0 })
    }

    #[cfg(unix)]
    fn set_directory(self, dir: &Dir) -> io::Result<()> {
        let file = dir.open(".")?;
        self.set(&file)?;
        file.sync_all()
    }

    #[cfg(windows)]
    fn set_directory(self, dir: &Dir) -> io::Result<()> {
        let mut permissions = dir.dir_metadata()?.permissions();
        permissions.set_readonly(self.0 != 0);
        dir.set_permissions(".", permissions)
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
        mode: FileMode,
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

    pub(crate) fn file_mode(&self) -> Option<FileMode> {
        match self {
            Self::File { mode, .. } => Some(*mode),
            _ => None,
        }
    }

    pub(crate) fn private_directory() -> Self {
        Self::directory_with_mode(if cfg!(unix) { 0o700 } else { 0 })
    }

    pub(crate) fn directory_with_mode(mode: u32) -> Self {
        Self::directory_with_permissions(FileMode(mode))
    }

    pub(crate) fn directory_with_permissions(mode: FileMode) -> Self {
        Self::Directory { mode }
    }

    pub(crate) fn is_directory(&self) -> bool {
        matches!(self, Self::Directory { .. })
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
    shared_rule_source: bool,
    private_directories: bool,
    #[cfg(test)]
    pub(crate) simulate_cross_volume: bool,
    #[cfg(test)]
    fail_key_file_after_rename: bool,
    #[cfg(all(test, not(windows)))]
    pub(crate) refuse_windows_acl_in_test: bool,
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
            shared_rule_source: false,
            private_directories: false,
            #[cfg(test)]
            simulate_cross_volume: false,
            #[cfg(test)]
            fail_key_file_after_rename: false,
            #[cfg(all(test, not(windows)))]
            refuse_windows_acl_in_test: false,
        })
    }

    pub(crate) fn open_home(home: &Path) -> io::Result<Self> {
        let mut root = Self::open(home)?;
        root.harness = Some(HarnessGuard::new(home)?);
        root.tool_home = true;
        Ok(root)
    }

    /// Only an approved rule share and its recorded undo use this view.
    /// Other harness paths and aliases keep the normal protection.
    pub(crate) fn with_shared_rule_source(&self) -> io::Result<Self> {
        Ok(Self {
            dir: self.dir.try_clone()?,
            harness: self.harness,
            tool_home: self.tool_home,
            shared_rule_source: true,
            private_directories: self.private_directories,
            #[cfg(test)]
            simulate_cross_volume: self.simulate_cross_volume,
            #[cfg(test)]
            fail_key_file_after_rename: self.fail_key_file_after_rename,
            #[cfg(all(test, not(windows)))]
            refuse_windows_acl_in_test: self.refuse_windows_acl_in_test,
        })
    }

    pub(crate) fn open_data(home: &Path, path: &Path) -> io::Result<Self> {
        Self::open(path)?.guard_data(home, path)
    }

    pub(crate) fn guard_data(mut self, home: &Path, path: &Path) -> io::Result<Self> {
        let guard = HarnessGuard::new(home)?;
        guard.check_absolute(path)?;
        guard.check(&self.dir.dir_metadata()?)?;
        self.harness = Some(guard);
        Ok(self)
    }

    pub(crate) fn with_private_directories(mut self) -> Self {
        self.private_directories = true;
        self
    }

    pub(crate) fn check_recovery_volume(
        &self,
        path: &Path,
        data: &FileRoot,
        recovery_directory: &Path,
    ) -> io::Result<()> {
        let (parent, _) = entry_path(path)?;
        let parent = self.open_parent(parent, false)?.ok_or_else(invalid_path)?;
        let recovery = data.open_parent(recovery_directory, false)?;
        let volume = recovery.as_ref().unwrap_or(&data.dir).dir_metadata()?.dev();
        #[cfg(test)]
        let volume = volume ^ u64::from(self.simulate_cross_volume);
        same_recovery_volume(parent.dir_metadata()?.dev(), volume)
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
        Ok(self.dir.open_with(PERSONAL_LOCK, &options)?.into_std())
    }

    /// Open the setup lock without creating anything or repairing permissions.
    pub(crate) fn open_existing_private_lock(&self) -> io::Result<std::fs::File> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).follow(FollowSymlinks::No);
        Ok(self.dir.open_with(PERSONAL_LOCK, &options)?.into_std())
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
        read_leaf_with(&parent, name, false, false, |file, metadata| {
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
        let Some(parent) = self.open_parent(relative_parent, false)? else {
            return Ok(Snapshot::Absent);
        };
        #[cfg(windows)]
        if self.tool_home {
            self.check_directory_leaf(&parent, name)?;
        }
        self.read_entry(&parent, name, allow_symlink)
    }

    fn read_entry(&self, parent: &Dir, name: &Path, allow_symlink: bool) -> io::Result<Snapshot> {
        #[cfg(not(windows))]
        let _ = self;
        #[cfg(windows)]
        if self.tool_home {
            return read_leaf_with(parent, name, allow_symlink, false, |file, _| {
                check_windows_replacement(file)?;
                read_file_contents(file)
            })
            .map_err(windows_access_error);
        }
        read_leaf(parent, name, allow_symlink)
    }

    #[cfg(windows)]
    pub(crate) fn windows_entry_owner(&self, path: &Path) -> io::Result<Vec<u8>> {
        let (relative_parent, name) = entry_path(path)?;
        let parent = self
            .open_parent(relative_parent, false)?
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        read_windows_owner(&open_windows_entry(&parent, name)?)
    }

    /// Resolve only the components below the configured tool path. Tool
    /// roots use their configured spelling even if listed differently.
    pub(crate) fn case_spelling(&self, path: &Path, scope: &Path) -> io::Result<Option<PathBuf>> {
        let (parent, leaf) = entry_path(path)?;
        let Some(directory) = self.open_parent(parent, false)? else {
            return Ok(None);
        };
        match directory.symlink_metadata(leaf) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
        let mut resolved = scope.to_owned();
        let components = path
            .ancestors()
            .take_while(|part| *part != scope)
            .collect::<Vec<_>>();
        for component in components.into_iter().rev() {
            let (parent, name) = entry_path(component)?;
            let directory = self.open_parent(parent, false)?.ok_or_else(invalid_path)?;
            let name = name.as_os_str();
            resolved.push(case_alias_in(&directory, name)?.as_deref().unwrap_or(name));
        }
        Ok((resolved != path).then_some(resolved))
    }

    /// Check the journaled spelling from the home down, even if the leaf
    /// is absent. Only restore planning can account for a missing parent.
    pub(crate) fn restore_leaf_alias(
        &self,
        path: &Path,
        can_restore_parent: impl Fn(&Path) -> bool,
    ) -> io::Result<Option<std::ffi::OsString>> {
        let (parents, leaf) = entry_path(path)?;
        self.check_tool_path(path)?;
        let mut directory = Some(self.dir.try_clone()?);
        let mut prefix = PathBuf::new();
        for part in parents {
            prefix.push(part);
            if let Some(parent) = directory.take() {
                if let Some(alias) = case_alias_in(&parent, part)? {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!(
                            "the parent spelling changed from {} to {}",
                            prefix.display(),
                            Path::new(&alias).display()
                        ),
                    ));
                }
                directory = match parent.open_dir_nofollow(part) {
                    Ok(directory) => Some(directory),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error),
                };
            }
            if directory.is_none() && !can_restore_parent(&prefix) {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("the parent {} is missing", prefix.display()),
                ));
            }
        }
        match directory {
            Some(parent) => case_alias_in(&parent, leaf.as_os_str()),
            None => Ok(None),
        }
    }

    pub(crate) fn read_personal_file(
        &self,
        path: &Path,
        allow_symlink: bool,
    ) -> io::Result<Snapshot> {
        require_ascii_personal_path(path)?;
        let (parent, name) = entry_path(path)?;
        let Some(parent) = self.open_parent_checked(parent, false, true)? else {
            return Ok(Snapshot::Absent);
        };
        #[cfg(windows)]
        if self.tool_home {
            self.check_directory_leaf(&parent, name)?;
            return read_single_link_leaf(&parent, name, allow_symlink)
                .map_err(windows_access_error);
        }
        read_single_link_leaf(&parent, name, allow_symlink)
    }

    pub(crate) fn children(&self, path: &Path) -> io::Result<BTreeSet<PathBuf>> {
        entry_path(path)?;
        match self.open_parent(path, false)? {
            Some(directory) => directory
                .entries()?
                .map(|entry| Ok(path.join(entry?.file_name())))
                .collect(),
            None => Ok(BTreeSet::new()),
        }
    }

    pub(crate) fn entry_identity(&self, path: &Path) -> io::Result<Option<(u64, u64)>> {
        Ok(self
            .entry_metadata(path)?
            .as_ref()
            .and_then(directory_entry_identity))
    }

    fn entry_metadata(&self, path: &Path) -> io::Result<Option<Metadata>> {
        let (parent, leaf) = entry_path(path)?;
        let Some(parent) = self.open_parent(parent, false)? else {
            return Ok(None);
        };
        match parent.symlink_metadata(leaf) {
            Ok(metadata) => Ok(Some(metadata)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn file_stamp(&self, path: &Path) -> io::Result<Option<(SystemTime, u64)>> {
        let Some(metadata) = self.entry_metadata(path)? else {
            return Ok(None);
        };
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected a regular file",
            ));
        }
        Ok(Some((metadata.modified()?.into_std(), metadata.len())))
    }

    pub(crate) fn case_alias(&self, path: &Path, known: &Path) -> io::Result<bool> {
        if path == known {
            return Ok(false);
        }
        let live = (self.entry_identity(path), self.entry_identity(known));
        if let (Ok(Some(left)), Ok(Some(right))) = &live {
            return Ok(left == right);
        }
        if !path.as_os_str().is_ascii() || !known.as_os_str().is_ascii() {
            // A legacy Unicode receipt with no live identity has unknown
            // aliases. Reserve its personal location until it is reconciled.
            return Ok(location_of(path)
                .zip(location_of(known))
                .is_some_and(|(left, right)| left == right));
        }
        if path.components().count() != known.components().count()
            || !path
                .iter()
                .zip(known)
                .all(|(left, right)| left.eq_ignore_ascii_case(right))
        {
            return Ok(false);
        }
        live.0?;
        live.1?;
        for (left, right) in path.ancestors().zip(known.ancestors()) {
            let (Some(left_name), Some(right_name)) = (left.file_name(), right.file_name()) else {
                continue;
            };
            if left_name == right_name {
                continue;
            }
            // On an absent leaf, use the containing directory's case rules.
            // If it is also absent, continue toward the closest existing parent.
            for ancestor in left.ancestors().skip(1) {
                if let Some(parent) = self.open_parent(ancestor, false)? {
                    if directory_is_case_sensitive(&parent) {
                        return Ok(false);
                    }
                    break;
                }
            }
        }
        Ok(true)
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

    /// Replace a personal settings file using a sibling name recorded in the
    /// durable rulesync manifest. Key-bearing settings need this separate
    /// writer: the generic journal retains a displaced copy on Windows.
    pub(crate) fn apply_key_file(
        &self,
        path: &Path,
        temp: &Path,
        seen: &Snapshot,
        desired: &Snapshot,
    ) -> io::Result<ApplyOutcome> {
        self.apply_key_file_after(path, temp, seen, desired, || Ok(()))
    }

    #[cfg(test)]
    pub(crate) fn inject_key_file_error_after_rename(&mut self) {
        self.fail_key_file_after_rename = true;
    }

    fn apply_key_file_after(
        &self,
        path: &Path,
        temp: &Path,
        seen: &Snapshot,
        desired: &Snapshot,
        after_replace: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<ApplyOutcome> {
        let (relative_parent, name) = entry_path(path)?;
        let (temp_parent, temp_name) = entry_path(temp)?;
        if relative_parent != temp_parent
            || !matches!(seen, Snapshot::File { .. })
            || !matches!(desired, Snapshot::File { .. } | Snapshot::Absent)
        {
            return Err(invalid_path());
        }
        self.check_tool_path(path)?;
        self.check_tool_path(temp)?;
        require_ascii_personal_path(path)?;
        require_ascii_personal_path(temp)?;
        let parent = self
            .open_parent_checked(relative_parent, false, true)?
            .ok_or_else(invalid_path)?;
        self.check_directory_leaf(&parent, name)?;
        if *seen != self.read_entry(&parent, name, false)?
            || *seen != read_single_link_leaf(&parent, name, false)?
        {
            return Ok(ApplyOutcome::ChangedSincePlan);
        }
        if seen == desired {
            return Ok(ApplyOutcome::Applied);
        }
        match parent.symlink_metadata(temp_name) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "the recorded settings temp already exists; recover it before retrying",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        #[cfg(not(windows))]
        let mut incoming_file = None;
        if let Snapshot::File { bytes, .. } = desired {
            let mut options = OpenOptions::new();
            options
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            #[cfg(unix)]
            options.mode(0o600);
            let mut incoming = parent.open_with(temp_name, &options)?;
            #[cfg(unix)]
            wenlan_core::rulesync_personal_files::require_plain_unix_acl(&incoming)?;
            #[cfg(windows)]
            copy_windows_dacl(
                &open_windows_entry(&parent, name)?,
                &open_windows_entry(&parent, temp_name)?,
            )?;
            incoming.write_all(bytes)?;
            incoming.sync_all()?;
            #[cfg(not(windows))]
            {
                incoming_file = Some(incoming);
            }
            #[cfg(windows)]
            {
                drop(incoming);
            }
        }

        let current_parent = self
            .open_parent_checked(relative_parent, false, true)?
            .ok_or_else(invalid_path)?;
        if !same_directory(&parent, &current_parent)?
            || !matches_leaf_with(&parent, name, seen, read_single_link_leaf)?
        {
            self.remove_key_temp_in(&parent, temp_name)?;
            return Ok(ApplyOutcome::ChangedSincePlan);
        }
        #[cfg(windows)]
        {
            let old = open_windows_entry(&parent, name)?;
            check_windows_replacement(&old)?;
            if matches!(desired, Snapshot::File { .. }) {
                let incoming = open_windows_entry(&parent, temp_name)?;
                prepare_windows_file_replacement(&old, &incoming)?;
                drop(incoming);
                drop(old);
                // ReplaceFileW's backup argument is null: no displaced copy
                // of the key-bearing file is created.
                windows_paths::replace_file(&parent, name, &parent, temp_name)
                    .map_err(windows_permissions_error)?;
            } else {
                drop(old);
                parent.remove_file(name)?;
            }
        }
        #[cfg(not(windows))]
        if matches!(desired, Snapshot::File { .. }) {
            parent.rename(temp_name, &parent, name)?;
        } else {
            parent.remove_file(name)?;
        }
        // Keep the incoming file private at its temporary name. Apply the
        // destination mode only after publication so key bytes are never
        // transiently broader-accessible through a second pathname.
        #[cfg(not(windows))]
        if let (Some(file), Snapshot::File { mode, .. }) = (incoming_file.as_ref(), desired) {
            mode.set(file)?;
            file.sync_all()?;
        }
        #[cfg(windows)]
        if let Snapshot::File { mode, .. } = desired {
            let replaced = open_windows_entry(&parent, name)?;
            mode.set(&replaced)?;
        }
        #[cfg(test)]
        crash_at("key-renamed", name);
        #[cfg(test)]
        if self.fail_key_file_after_rename {
            return Err(io::Error::other("injected post-rename error"));
        }
        after_replace()?;
        #[cfg(unix)]
        parent.open(".")?.sync_all()?;
        Ok(ApplyOutcome::Applied)
    }

    /// Remove only the exact regular file named by a durable key-write record.
    pub(crate) fn cleanup_key_temp(&self, destination: &Path, temp: &Path) -> io::Result<()> {
        let (destination_parent, _) = entry_path(destination)?;
        let (relative_parent, name) = entry_path(temp)?;
        if destination_parent != relative_parent {
            return Err(invalid_path());
        }
        self.check_tool_path(temp)?;
        require_ascii_personal_path(temp)?;
        let Some(parent) = self.open_parent_checked(relative_parent, false, true)? else {
            return Ok(());
        };
        self.remove_key_temp_in(&parent, name)
    }

    fn remove_key_temp_in(&self, parent: &Dir, name: &Path) -> io::Result<()> {
        match parent.symlink_metadata(name) {
            Ok(metadata) if metadata.is_file() && !metadata.is_symlink() => {
                parent.remove_file(name)
            }
            Ok(_) => Err(io::Error::other(
                "the recorded settings temp is not a regular file; it was left in place",
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
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
        let personal = self.tool_home && is_personal_entry(path);
        if personal {
            require_ascii_personal_path(path)?;
        }
        let read: ReadLeaf = if personal {
            read_single_link_leaf
        } else {
            read_leaf
        };
        if let Snapshot::File { bytes, .. } = desired {
            check_file_size(bytes.len() as u64)?;
        }
        if cfg!(windows) && personal && desired.file_mode().is_some_and(|mode| mode.0 != 0) {
            return Err(readonly_personal_file());
        }
        if !options.allow_symlink
            && (matches!(seen, Snapshot::Symlink { .. })
                || matches!(desired, Snapshot::Symlink { .. }))
        {
            return Err(symlink_denied());
        }
        #[cfg(windows)]
        if self.tool_home
            && (matches!(seen, Snapshot::Symlink { .. })
                || matches!(desired, Snapshot::Symlink { .. }))
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "left unmanaged: Windows tool links cannot be safely replaced",
            ));
        }
        let create = *seen == Snapshot::Absent
            && *desired != Snapshot::Absent
            && !(self.tool_home && options.stage_id.is_some());
        let Some(parent) = self.open_parent_checked(relative_parent, create, personal)? else {
            return Ok(
                if *seen == Snapshot::Absent && *desired == Snapshot::Absent {
                    ApplyOutcome::Applied
                } else {
                    ApplyOutcome::ChangedSincePlan
                },
            );
        };
        self.check_directory_leaf(&parent, name)?;
        // A personal file must also pass the single-link reader.
        if *seen != self.read_entry(&parent, name, true)? || *seen != read(&parent, name, true)? {
            return Ok(ApplyOutcome::ChangedSincePlan);
        }
        if desired == seen {
            return Ok(ApplyOutcome::Applied);
        }
        if let (Snapshot::Directory { .. }, Snapshot::Directory { mode }) = (seen, desired) {
            // Change metadata on the open directory without replacing its subtree.
            let directory = parent.open_dir_nofollow(name)?;
            let Some(current_parent) =
                self.open_parent_checked(relative_parent, false, personal)?
            else {
                return Ok(ApplyOutcome::ChangedSincePlan);
            };
            if !same_directory(&parent, &current_parent)?
                || !same_directory(&directory, &current_parent.open_dir_nofollow(name)?)?
                || *seen
                    != Snapshot::directory_with_permissions(FileMode::of(
                        &directory.dir_metadata()?,
                    ))
            {
                return Ok(ApplyOutcome::ChangedSincePlan);
            }
            if let Some(verify) = options.verify_source {
                if !verify()? {
                    return Ok(ApplyOutcome::ChangedSincePlan);
                }
            }
            self.check_directory_leaf(&current_parent, name)?;
            #[cfg(windows)]
            check_windows_replacement(&directory)?;
            if personal {
                require_plain_personal_directory(&directory)?;
            }
            mode.set_directory(&directory)?;
            return Ok(ApplyOutcome::Applied);
        }
        let displaced = recovery_location(&parent, options.displaced)?;
        let mut staged = Stage::new(
            &parent,
            options.stage_id,
            personal && matches!(desired, Snapshot::File { .. }),
        )?;
        let result = (|| {
            #[cfg(all(test, not(windows)))]
            if self.refuse_windows_acl_in_test
                && *seen != Snapshot::Absent
                && (self.tool_home || options.stage_id.is_some() || displaced.is_some())
            {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "left unmanaged: could not preserve Windows permissions (the Windows owner is not the process user or one of its owner groups)",
                ));
            }
            staged.write(desired, personal.then_some(name))?;
            if options.stage_id.is_some() && matches!(desired, Snapshot::File { .. }) {
                staged.dir.hard_link(STAGED, &staged.dir, INCOMING)?;
            }
            #[cfg(test)]
            crash_at("staged", name);
            before_commit();
            let Some(current_parent) =
                self.open_parent_checked(relative_parent, false, personal)?
            else {
                return Ok(ApplyOutcome::ChangedSincePlan);
            };
            self.check_directory_leaf(&current_parent, name)?;
            if !same_directory(&parent, &current_parent)?
                || !matches_leaf_with(&parent, name, seen, read)?
            {
                return Ok(ApplyOutcome::ChangedSincePlan);
            }
            if let Some(verify) = options.verify_source {
                if !verify()? || !matches_leaf_with(&parent, name, seen, read)? {
                    return Ok(ApplyOutcome::ChangedSincePlan);
                }
            }
            match commit(
                &mut staged,
                name,
                seen,
                desired,
                displaced.as_ref(),
                read,
                personal,
            ) {
                Err(error)
                    if !staged.preserve
                        && matches!(
                            error.kind(),
                            io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound
                        ) =>
                {
                    Ok(ApplyOutcome::ChangedSincePlan)
                }
                Err(error)
                    if !staged.preserve
                        && seen.is_directory()
                        && *desired == Snapshot::Absent
                        && error.kind() == io::ErrorKind::DirectoryNotEmpty =>
                {
                    Ok(ApplyOutcome::ChangedSincePlan)
                }
                other => other,
            }
        })();
        if !staged.preserve {
            staged.preserve = true;
            let cleanup = staged.clean();
            return result.and_then(|outcome| cleanup.map(|()| outcome));
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
        let (relative_parent, leaf) = entry_path(path)?;
        let Some(parent) = self.open_parent(relative_parent, false)? else {
            return Ok(());
        };
        #[cfg(not(windows))]
        let _ = leaf;
        #[cfg(windows)]
        let check_recovery_file = |held_dir: &Dir, held: &Path| {
            if self.tool_home && self.restore_leaf_alias(path, |_| false)?.is_some() {
                return Err(io::Error::other("the destination spelling changed"));
            }
            check_windows_replacement(&open_windows_entry(held_dir, held)?)?;
            Ok::<_, io::Error>(())
        };
        #[cfg(windows)]
        if *desired == Snapshot::Absent && *actual == Snapshot::Absent {
            if let Some((saved_dir, saved)) = recovery_location(&parent, options.displaced)? {
                let entry = read_leaf(&saved_dir, saved, true)?;
                if matches!(entry, Snapshot::File { .. }) && *seen != entry {
                    check_recovery_file(&saved_dir, saved)?;
                    return keep_changed_deletion(&saved_dir, saved, &parent, leaf, actual);
                }
            }
        }
        let personal = self.tool_home && is_personal_entry(path);
        let read: ReadLeaf = if personal {
            read_single_link_leaf
        } else {
            read_leaf
        };
        if *desired == Snapshot::Absent || personal && matches!(seen, Snapshot::File { .. }) {
            if let Some((saved_dir, saved)) = recovery_location(&parent, options.displaced)? {
                check_displaced_entry(&saved_dir, saved, &parent, leaf, seen, read)?;
            }
        }
        let name = stage_name(id)?;
        let dir = match parent.open_dir_nofollow(&name) {
            Ok(dir) => dir,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let stage = Stage {
            parent: &parent,
            dir,
            name,
            preserve: true,
            file_beside_destination: personal && matches!(desired, Snapshot::File { .. }),
        };
        stage.remove_incoming_sibling()?;
        let dir = &stage.dir;
        #[cfg(windows)]
        match dir.symlink_metadata(HELD) {
            Ok(_) => {
                return recover_windows_entry(
                    &stage,
                    leaf,
                    seen,
                    desired,
                    actual,
                    options.displaced,
                );
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
        let staged = Path::new(STAGED);
        #[cfg(windows)]
        if personal {
            // An interrupted ACL merge may leave a second journal link to the
            // original. Release only that alias before checking its link count.
            remove_unused_recovery_link(dir, staged, (dir, Path::new(MERGED)))?;
        }
        let entry = read_leaf(dir, staged, true)?;
        #[cfg(windows)]
        if *desired == Snapshot::Absent && matches!(entry, Snapshot::File { .. }) && *seen != entry
        {
            if *actual == Snapshot::Absent {
                check_recovery_file(dir, staged)?;
            }
            return keep_changed_deletion(dir, staged, &parent, leaf, actual);
        }
        if *desired == Snapshot::Absent {
            check_displaced_entry(dir, staged, &parent, leaf, seen, read)?;
        }
        // Only an incoming link to a different file proves that the
        // original was exchanged into the stage.
        let (incoming, exchanged) = match dir.symlink_metadata(INCOMING) {
            Ok(metadata) if metadata.is_file() => {
                let staged = dir.symlink_metadata(STAGED);
                (
                    staged
                        .as_ref()
                        .is_ok_and(|staged| staged.is_file() && same_identity(staged, &metadata)),
                    staged
                        .is_ok_and(|staged| staged.is_file() && !same_identity(&staged, &metadata)),
                )
            }
            Ok(_) => return Err(io::Error::other("invalid rulesync staging entry")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // The incoming hardlink is created before replacement. If
                // the original is still in place, a partial incoming write
                // may be the only entry. A personal replacement links it
                // before any exchange, so without the link the entry is
                // never the person's original. Drop it rather than retain
                // partial bytes where a later recovery could put them back.
                let file = matches!(desired, Snapshot::File { .. });
                (
                    *desired == entry || (file && (personal || seen == actual)),
                    false,
                )
            }
            Err(error) => return Err(error),
        };
        if personal && exchanged && *desired != Snapshot::Absent && *seen != Snapshot::Absent {
            check_displaced_entry(dir, staged, &parent, leaf, seen, read)?;
        }
        #[cfg(windows)]
        let restored = personal
            && exchanged
            && matches!(entry, Snapshot::File { .. })
            && *actual == Snapshot::Absent
            && matches!(seen, Snapshot::File { .. })
            && matches!(desired, Snapshot::File { .. })
            && {
                check_recovery_file(dir, staged)?;
                restore_windows_held(dir, staged, &parent, leaf)?
            };
        #[cfg(not(windows))]
        let restored = false;
        if entry != Snapshot::Absent && !incoming && !restored {
            if matches!(entry, Snapshot::File { .. }) {
                let displaced = recovery_location(&parent, options.displaced)?
                    .ok_or_else(|| io::Error::other("a displaced file requires manual recovery"))?;
                // Put back only an original the exchange moved into the stage.
                #[cfg(windows)]
                let put_back = exchanged
                    && *actual == Snapshot::Absent
                    && matches!(seen, Snapshot::File { .. })
                    && matches!(desired, Snapshot::File { .. });
                #[cfg(windows)]
                if put_back {
                    check_recovery_file(dir, staged)?;
                }
                move_displaced(dir, &displaced)?;
                #[cfg(windows)]
                if put_back {
                    restore_windows_held(&displaced.0, displaced.1, &parent, leaf)?;
                }
                #[cfg(unix)]
                displaced.0.open(".")?.sync_all()?;
            } else if !matches_leaf(dir, staged, seen)? || desired != actual {
                return Err(io::Error::other(format!(
                    "an unexpected entry is saved in {}/{STAGED}. Check it before retrying",
                    stage.name,
                )));
            }
        }
        #[cfg(windows)]
        if personal {
            if let Some(displaced) = recovery_location(&parent, options.displaced)? {
                remove_unused_recovery_link(&parent, leaf, (&displaced.0, displaced.1))?;
            }
        }
        stage.clean()
    }

    fn open_parent(&self, path: &Path, create: bool) -> io::Result<Option<Dir>> {
        self.open_parent_checked(path, create, false)
    }

    fn open_parent_checked(
        &self,
        path: &Path,
        create: bool,
        personal: bool,
    ) -> io::Result<Option<Dir>> {
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
            let open = |directory: &Dir| {
                #[cfg(windows)]
                if self.tool_home {
                    return open_windows_tool_dir(directory, part);
                }
                directory.open_dir_nofollow(part)
            };
            let directory = match open(&parent) {
                Ok(dir) => dir,
                Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
                    match create_directory(&parent, part) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error),
                    }
                    open(&parent)?
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            if personal {
                require_ascii_entry_name(&parent, part.as_os_str(), &directory.dir_metadata()?)?;
                require_plain_personal_directory(&directory)?;
            }
            parent = directory;
            if let Some(guard) = &self.harness {
                if !(self.shared_rule_source && path == Path::new(".agents")) {
                    guard.check(&parent.dir_metadata()?)?;
                }
            }
            if self.private_directories {
                require_private_permissions(&parent.dir_metadata()?, "rulesync data directory")?;
            }
        }
        Ok(Some(parent))
    }

    fn check_tool_path(&self, path: &Path) -> io::Result<()> {
        if self.tool_home
            && is_harness_path(path)
            && !(self.shared_rule_source && is_shared_rule_source_path(path))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "inside the read-only ~/.agents harness. Choose a path outside it",
            ));
        }
        Ok(())
    }

    fn check_directory_leaf(&self, parent: &Dir, name: &Path) -> io::Result<()> {
        if self.shared_rule_source
            && name == Path::new(".agents")
            && same_directory(parent, &self.dir)?
        {
            return Ok(());
        }
        if let Some(guard) = &self.harness {
            match parent.symlink_metadata(name) {
                Ok(metadata) if metadata.is_dir() => {
                    #[cfg(windows)]
                    let directory = if self.tool_home {
                        open_windows_tool_dir(parent, name)?
                    } else {
                        parent.open_dir_nofollow(name)?
                    };
                    #[cfg(not(windows))]
                    let directory = parent.open_dir_nofollow(name)?;
                    guard.check(&directory.dir_metadata()?)?;
                }
                Ok(_) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => {
                    #[cfg(windows)]
                    let error = if self.tool_home {
                        windows_access_error(error)
                    } else {
                        error
                    };
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn is_shared_rule_source_path(path: &Path) -> bool {
    path == Path::new(SHARED_RULES) || path == Path::new(".agents")
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

fn create_planned_directory(parent: &Dir, name: &Path, mode: FileMode) -> io::Result<()> {
    create_directory(parent, name)?;
    mode.set_directory(&parent.open_dir_nofollow(name)?)
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
    read_leaf_with(parent, name, allow_symlink, false, |file, _| {
        read_file_contents(file)
    })
}

fn read_single_link_leaf(parent: &Dir, name: &Path, allow_symlink: bool) -> io::Result<Snapshot> {
    read_leaf_with(parent, name, allow_symlink, true, single_link_contents)
}

fn single_link_contents(file: &cap_std::fs::File, metadata: &Metadata) -> io::Result<Vec<u8>> {
    require_plain_personal_file(file, metadata)?;
    read_file_contents(file)
}

fn require_ascii_personal_path(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_ascii() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            NON_ASCII_PERSONAL_NAME,
        ))
    }
}

fn read_leaf_with(
    parent: &Dir,
    name: &Path,
    allow_symlink: bool,
    personal: bool,
    read: impl FnOnce(&cap_std::fs::File, &Metadata) -> io::Result<Vec<u8>>,
) -> io::Result<Snapshot> {
    let metadata = match parent.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Snapshot::Absent),
        Err(error) => return Err(error),
    };
    if personal {
        require_ascii_entry_name(parent, name.as_os_str(), &metadata)?;
    }
    if metadata.is_symlink() {
        if !allow_symlink {
            return Err(symlink_denied());
        }
        #[cfg(target_os = "macos")]
        if personal {
            wenlan_core::rulesync_personal_files::require_plain_macos_link(parent, name)?;
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
        if personal {
            require_plain_personal_directory(&parent.open_dir_nofollow(name)?)?;
        }
        return Ok(Snapshot::Directory {
            mode: FileMode::of(&metadata),
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

type ReadLeaf = fn(&Dir, &Path, bool) -> io::Result<Snapshot>;

fn matches_leaf(parent: &Dir, name: &Path, expected: &Snapshot) -> io::Result<bool> {
    matches_leaf_with(parent, name, expected, read_leaf)
}

fn matches_leaf_with(
    parent: &Dir,
    name: &Path,
    expected: &Snapshot,
    read: ReadLeaf,
) -> io::Result<bool> {
    if *expected != read(parent, name, true)? {
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
#[cfg(windows)]
const MERGED: &str = "merged";
#[cfg(windows)]
const REPLACEMENT: &str = "replacement";
#[cfg(windows)]
const HELD: &str = "held";

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
            let directory = root
                .open_parent(directory, false)?
                .ok_or_else(invalid_path)?;
            same_recovery_volume(
                parent.dir_metadata()?.dev(),
                directory.dir_metadata()?.dev(),
            )?;
            Ok((directory, name))
        })
        .transpose()
}

fn same_recovery_volume(source: u64, recovery: u64) -> io::Result<()> {
    if source == recovery {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::CrossesDevices,
            "place Wenlan's rulesync data on the same volume as this tool file to keep concurrent saves recoverable",
        ))
    }
}

fn move_displaced(dir: &Dir, displaced: &(Dir, &Path)) -> io::Result<()> {
    move_displaced_entry(dir, Path::new(STAGED), displaced)
}

fn move_displaced_entry(dir: &Dir, leaf: &Path, displaced: &(Dir, &Path)) -> io::Result<()> {
    let (directory, name) = displaced;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    rustix::fs::renameat_with(
        dir,
        leaf,
        directory,
        *name,
        rustix::fs::RenameFlags::NOREPLACE,
    )?;
    #[cfg(windows)]
    windows_paths::move_entry(dir, leaf, directory, name)?;
    Ok(())
}

fn check_displaced_entry(
    held_dir: &Dir,
    held: &Path,
    parent: &Dir,
    name: &Path,
    seen: &Snapshot,
    read: ReadLeaf,
) -> io::Result<()> {
    match held_dir.symlink_metadata(held) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(_) => (),
    }
    let reason = match matches_leaf_with(held_dir, held, seen, read) {
        Ok(true) => return Ok(()),
        Ok(false) => "the displaced entry differs from the approved snapshot".to_owned(),
        Err(error) => error.to_string(),
    };
    let outcome = match restore_held_file(held_dir, held, parent, name) {
        Ok(true) => "A newer or protected entry was put back.".into(),
        Ok(false) => {
            "The destination is occupied. The newer or protected entry is still in recovery.".into()
        }
        Err(error) => {
            format!("Could not put the entry back ({error}). Check the retained recovery files.")
        }
    };
    Err(io::Error::other(format!(
        "{outcome} The interrupted change is still pending ({reason})."
    )))
}

fn restore_held_file(held_dir: &Dir, held: &Path, parent: &Dir, name: &Path) -> io::Result<bool> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let restored = rustix::fs::renameat_with(
        held_dir,
        held,
        parent,
        name,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(io::Error::from);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let restored = held_dir
        .hard_link(held, parent, name)
        .and_then(|()| held_dir.remove_file(held));
    match restored {
        Ok(()) => {
            #[cfg(unix)]
            {
                parent.open(".")?.sync_all()?;
                held_dir.open(".")?.sync_all()?;
            }
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
pub(crate) fn crash_at(point: &str, name: &Path) {
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
    file_beside_destination: bool,
}

impl<'a> Stage<'a> {
    fn new(parent: &'a Dir, id: Option<&str>, file_beside_destination: bool) -> io::Result<Self> {
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
            file_beside_destination,
        })
    }

    fn write(&self, data: &Snapshot, personal: Option<&Path>) -> io::Result<()> {
        match data {
            Snapshot::Absent => Ok(()),
            Snapshot::Directory { mode } => {
                create_planned_directory(&self.dir, Path::new(STAGED), *mode)?;
                if personal.is_some() {
                    require_plain_personal_directory(&self.dir.open_dir_nofollow(STAGED)?)?;
                }
                Ok(())
            }
            Snapshot::File { bytes, mode } => {
                let mut options = OpenOptions::new();
                options
                    .write(true)
                    .create_new(true)
                    .follow(FollowSymlinks::No);
                #[cfg(unix)]
                options.mode(0o600);
                let mut file = if self.file_beside_destination {
                    // Inherit from the destination folder before publishing
                    // by hard link, which keeps the existing descriptor.
                    // Anchor the empty file in the journal before writing.
                    let sibling = self.incoming_sibling();
                    let empty = self.parent.open_with(&sibling, &options)?;
                    self.parent.hard_link(&sibling, &self.dir, STAGED)?;
                    if !same_identity(&empty.metadata()?, &self.dir.symlink_metadata(STAGED)?) {
                        return Err(io::Error::other("the incoming file changed during staging"));
                    }
                    self.remove_incoming_sibling()?;
                    empty
                } else {
                    self.dir.open_with(STAGED, &options)?
                };
                #[cfg(unix)]
                if personal.is_some() {
                    wenlan_core::rulesync_personal_files::require_plain_unix_acl(&file)?;
                }
                #[cfg(test)]
                if let Some(name) = personal {
                    crash_at("before-incoming", name);
                }
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

    fn incoming_sibling(&self) -> String {
        format!("{}-file", self.name)
    }

    fn remove_incoming_sibling(&self) -> io::Result<()> {
        if !self.file_beside_destination {
            return Ok(());
        }
        let sibling = self.incoming_sibling();
        match self.parent.symlink_metadata(&sibling) {
            Ok(metadata) => {
                let anchored = self.dir.symlink_metadata(STAGED).is_ok_and(|staged| {
                    metadata.is_file() && staged.is_file() && same_identity(&metadata, &staged)
                });
                if !anchored {
                    return Err(io::Error::other(format!(
                        "Interrupted staging left an unverified file at {sibling}. Check it before retrying."
                    )));
                }
                self.parent.remove_file(&sibling)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn clean(&self) -> io::Result<()> {
        self.remove_incoming_sibling()?;
        #[cfg(windows)]
        let leaves = [MERGED, REPLACEMENT, HELD, STAGED, INCOMING];
        #[cfg(not(windows))]
        let leaves = [STAGED, INCOMING];
        for leaf in leaves {
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
    read: ReadLeaf,
    _personal_file: bool,
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
    let verify = matches_leaf_with(&stage.dir, held, seen, read);
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
    _read: ReadLeaf,
    personal_file: bool,
) -> io::Result<ApplyOutcome> {
    commit_windows(
        stage,
        name,
        seen,
        desired,
        displaced,
        personal_file,
        || {},
        || {},
    )
}

#[cfg(any(windows, test))]
#[allow(clippy::too_many_arguments)]
fn commit_windows(
    stage: &mut Stage<'_>,
    name: &Path,
    seen: &Snapshot,
    desired: &Snapshot,
    displaced: Option<&(Dir, &Path)>,
    personal_file: bool,
    before_displace: impl FnOnce(),
    after_displace: impl FnOnce(),
) -> io::Result<ApplyOutcome> {
    let file_replacement = matches!(seen, Snapshot::File { .. })
        && matches!(desired, Snapshot::Absent | Snapshot::File { .. });
    if personal_file && file_replacement && displaced.is_none() {
        return Err(io::Error::other("missing file recovery location"));
    }
    let read: ReadLeaf = if personal_file {
        read_single_link_leaf
    } else {
        read_leaf
    };
    if let Some(displaced) = displaced {
        if file_replacement {
            return commit_windows_personal_file(
                stage,
                name,
                seen,
                desired,
                displaced,
                read,
                before_displace,
                after_displace,
            );
        }
    }
    match (seen, desired) {
        (Snapshot::Absent, Snapshot::File { .. }) => {
            stage.dir.hard_link(STAGED, stage.parent, name)?
        }
        (Snapshot::Absent, Snapshot::Directory { mode }) => {
            create_planned_directory(stage.parent, name, *mode)?
        }
        (_, Snapshot::Absent) => {
            #[cfg(windows)]
            check_windows_replacement(&open_windows_entry(stage.parent, name)?)?;
            if seen.is_directory() {
                stage.parent.remove_dir(name)?;
            } else {
                stage.parent.remove_file_or_symlink(name)?;
            }
        }
        (Snapshot::File { .. }, Snapshot::File { .. }) => {
            #[cfg(windows)]
            return replace_windows_live_file(stage, name, seen);
            #[cfg(not(windows))]
            stage.dir.rename(STAGED, stage.parent, name)?;
        }
        #[cfg(windows)]
        (Snapshot::Directory { .. }, Snapshot::File { .. })
        | (Snapshot::File { .. }, Snapshot::Directory { .. }) => {
            return replace_windows_entry(stage, name, seen, displaced, read);
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

#[cfg(any(windows, test))]
#[allow(clippy::too_many_arguments)]
fn commit_windows_personal_file(
    stage: &mut Stage<'_>,
    name: &Path,
    seen: &Snapshot,
    desired: &Snapshot,
    displaced: &(Dir, &Path),
    read: ReadLeaf,
    before_displace: impl FnOnce(),
    after_displace: impl FnOnce(),
) -> io::Result<ApplyOutcome> {
    let expected = stage.parent.symlink_metadata(name)?;
    #[cfg(windows)]
    check_windows_replacement(&open_windows_entry(stage.parent, name)?)?;
    if !matches_leaf_with(stage.parent, name, seen, read)? {
        return Ok(ApplyOutcome::ChangedSincePlan);
    }
    let replacing = matches!(desired, Snapshot::File { .. });
    if replacing {
        if let Err(error) = stage.dir.hard_link(STAGED, &stage.dir, INCOMING) {
            if error.kind() != io::ErrorKind::AlreadyExists
                || !same_identity(
                    &stage.dir.symlink_metadata(STAGED)?,
                    &stage.dir.symlink_metadata(INCOMING)?,
                )
            {
                return Err(error);
            }
        }
        stage.dir.remove_file(STAGED)?;
    }
    before_displace();

    // Take the actual destination into the journaled stage so a save that
    // landed after the check stays reachable. Publish only at a vacant name.
    stage.parent.rename(name, &stage.dir, STAGED)?;
    stage.preserve = true;
    #[cfg(test)]
    crash_at("displaced", name);
    let held = stage.dir.symlink_metadata(STAGED)?;
    if !held.is_file() {
        return Err(io::Error::other(format!(
            "The path changed type during delivery. Its entry is preserved at {}/{STAGED}",
            stage.name,
        )));
    }
    if !same_identity(&expected, &held)
        || !matches!(
            matches_leaf_with(&stage.dir, Path::new(STAGED), seen, read),
            Ok(true)
        )
    {
        finish_windows_displacement(stage, name, displaced, true)?;
        return Ok(ApplyOutcome::ChangedSincePlan);
    }
    #[cfg(windows)]
    if let Err(error) = prepare_held_windows_file(stage, replacing) {
        finish_windows_displacement(stage, name, displaced, true)?;
        return Err(error);
    }
    after_displace();
    if replacing {
        #[cfg(windows)]
        if !matches!(
            matches_leaf_with(&stage.dir, Path::new(STAGED), seen, read),
            Ok(true)
        ) {
            finish_windows_displacement(stage, name, displaced, true)?;
            return Ok(ApplyOutcome::ChangedSincePlan);
        }
        if let Err(error) = stage.dir.hard_link(INCOMING, stage.parent, name) {
            finish_windows_displacement(stage, name, displaced, true)?;
            return if error.kind() == io::ErrorKind::AlreadyExists {
                Ok(ApplyOutcome::ChangedSincePlan)
            } else {
                Err(error)
            };
        }
    }
    finish_windows_displacement(stage, name, displaced, false)?;
    Ok(ApplyOutcome::Applied)
}

#[cfg(any(windows, test))]
fn finish_windows_displacement(
    stage: &mut Stage<'_>,
    name: &Path,
    displaced: &(Dir, &Path),
    rollback: bool,
) -> io::Result<()> {
    if rollback {
        restore_windows_held(&stage.dir, Path::new(STAGED), stage.parent, name)?;
    }
    // Keep the held inode even when rollback filled the live name. An
    // editor can replace that name again before the stage is cleaned.
    move_displaced(&stage.dir, displaced)?;
    stage.preserve = false;
    Ok(())
}

#[cfg(any(windows, test))]
fn restore_windows_held(
    held_dir: &Dir,
    held: &Path,
    parent: &Dir,
    name: &Path,
) -> io::Result<bool> {
    match held_dir.hard_link(held, parent, name) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn open_windows_entry(directory: &Dir, name: &Path) -> io::Result<cap_std::fs::File> {
    use cap_std::fs::{MetadataExt as _, OpenOptionsExt as _};

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    let file = directory
        .open_with(name, &options)
        .map_err(windows_access_error)?;
    let metadata = file.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "left unmanaged: Windows reparse points cannot be safely replaced",
        ));
    }
    Ok(file)
}

#[cfg(windows)]
fn prepare_held_windows_file(stage: &Stage<'_>, replacing: bool) -> io::Result<()> {
    let held = open_windows_entry(&stage.dir, Path::new(STAGED))?;
    if !replacing {
        check_windows_replacement(&held)?;
        return Ok(());
    }
    let incoming = open_windows_entry(&stage.dir, Path::new(INCOMING))?;
    prepare_windows_file_replacement(&held, &incoming)?;
    drop(incoming);
    drop(held);

    // Merge the two file identities while they are in the stage. ReplaceFileW
    // consumes its source name, so use aliases to leave both journal names
    // available. The live name is still published without overwriting a save.
    stage.dir.hard_link(STAGED, &stage.dir, MERGED)?;
    stage.dir.hard_link(INCOMING, &stage.dir, REPLACEMENT)?;
    windows_paths::replace_file(
        &stage.dir,
        Path::new(MERGED),
        &stage.dir,
        Path::new(REPLACEMENT),
    )
    .map_err(windows_permissions_error)
}

#[cfg(windows)]
fn replace_windows_live_file(
    stage: &mut Stage<'_>,
    name: &Path,
    seen: &Snapshot,
) -> io::Result<ApplyOutcome> {
    let old = open_windows_entry(stage.parent, name)?;
    let incoming = open_windows_entry(&stage.dir, Path::new(STAGED))?;
    prepare_windows_file_replacement(&old, &incoming)?;
    drop(incoming);
    let expected = old.metadata()?;
    drop(old);

    // The no-backup form of ReplaceFileW can fail after removing the live
    // name. Keep a link to its original inode until the call has completed.
    stage.parent.hard_link(name, &stage.dir, HELD)?;
    let held = Path::new(HELD);
    if !same_identity(&expected, &stage.dir.symlink_metadata(held)?)
        || !same_identity(&expected, &stage.parent.symlink_metadata(name)?)
        || !matches_leaf(stage.parent, name, seen)?
    {
        return Ok(ApplyOutcome::ChangedSincePlan);
    }

    stage.preserve = true;
    if let Err(error) =
        windows_paths::replace_file(stage.parent, name, &stage.dir, Path::new(STAGED))
    {
        let recovered = match stage.parent.symlink_metadata(name) {
            Ok(metadata) => same_identity(&expected, &metadata),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                matches!(
                    restore_windows_held(&stage.dir, held, stage.parent, name),
                    Ok(true)
                )
            }
            Err(_) => false,
        };
        if recovered {
            stage.preserve = false;
            return Err(windows_permissions_error(error));
        }
        return Err(windows_permissions_error(io::Error::other(format!(
            "{error}. The original file is preserved at {}/{HELD}",
            stage.name,
        ))));
    }
    stage.preserve = false;
    Ok(ApplyOutcome::Applied)
}

#[cfg(windows)]
fn replace_windows_entry(
    stage: &mut Stage<'_>,
    name: &Path,
    seen: &Snapshot,
    displaced: Option<&(Dir, &Path)>,
    read: ReadLeaf,
) -> io::Result<ApplyOutcome> {
    let original = open_windows_entry(stage.parent, name)?;
    check_windows_replacement(&original)?;
    let expected = original.metadata()?;
    drop(original);

    let held = Path::new(HELD);
    windows_paths::move_entry(stage.parent, name, &stage.dir, held)?;
    stage.preserve = true;
    #[cfg(test)]
    crash_at("displaced", name);
    let applied = (|| {
        let original = open_windows_entry(&stage.dir, held)?;
        if !same_identity(&expected, &original.metadata()?)
            || !matches_leaf_with(&stage.dir, held, seen, read)?
        {
            return Ok(ApplyOutcome::ChangedSincePlan);
        }
        let incoming = Path::new(STAGED);
        copy_windows_dacl(&original, &open_windows_entry(&stage.dir, incoming)?)?;
        windows_paths::move_entry(&stage.dir, incoming, stage.parent, name)?;
        Ok(ApplyOutcome::Applied)
    })();
    if matches!(applied, Ok(ApplyOutcome::Applied)) {
        if let Some(displaced) = displaced {
            move_displaced_entry(&stage.dir, held, displaced)?;
        }
    } else if let Err(error) = windows_paths::move_entry(&stage.dir, held, stage.parent, name) {
        return Err(io::Error::other(format!(
            "{error}. The displaced entry is preserved at {}/{HELD}",
            stage.name,
        )));
    }
    stage.preserve = false;
    applied
}

#[cfg(windows)]
fn recover_windows_entry(
    stage: &Stage<'_>,
    name: &Path,
    seen: &Snapshot,
    desired: &Snapshot,
    actual: &Snapshot,
    displaced: Option<(&FileRoot, &Path)>,
) -> io::Result<()> {
    let held = Path::new(HELD);
    let matches_original = matches_leaf(&stage.dir, held, seen)?;
    let live_is_held = stage.parent.symlink_metadata(name).is_ok_and(|live| {
        stage
            .dir
            .symlink_metadata(held)
            .is_ok_and(|saved| same_identity(&live, &saved))
    });
    if live_is_held {
        // A direct replacement stopped before it removed the live name.
        return stage.clean();
    }
    if *actual == Snapshot::Absent {
        windows_paths::move_entry(&stage.dir, held, stage.parent, name)?;
        return stage.clean();
    }
    if matches_original
        && actual == desired
        && stage
            .dir
            .symlink_metadata(STAGED)
            .is_err_and(|err| err.kind() == io::ErrorKind::NotFound)
    {
        if let Some(displaced) = recovery_location(stage.parent, displaced)? {
            move_displaced_entry(&stage.dir, held, &displaced)?;
        }
        return stage.clean();
    }
    Err(io::Error::other(format!(
        "The displaced entry is preserved at {}/{HELD}. Check it before retrying",
        stage.name,
    )))
}

#[cfg(windows)]
mod windows_paths {
    use cap_std::fs::Dir;
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, RawHandle},
    };
    use std::path::Path;
    use std::ptr::{null, null_mut};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFinalPathNameByHandleW(
            handle: RawHandle,
            path: *mut u16,
            length: u32,
            flags: u32,
        ) -> u32;
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut c_void,
            reserved: *mut c_void,
        ) -> i32;
        fn MoveFileW(from: *const u16, to: *const u16) -> i32;
    }

    fn leaf_path(parent: &Dir, name: &Path) -> io::Result<Vec<u16>> {
        // Use the opened directory, which cap-std keeps open without delete
        // sharing. Win32 returns an extended-length path for this handle.
        let leaf = name.as_os_str();
        if name.file_name() != Some(leaf)
            || leaf
                .as_encoded_bytes()
                .iter()
                .any(|byte| matches!(byte, b':' | 0))
        {
            return Err(super::invalid_path());
        }
        let mut path = vec![0; 512];
        loop {
            // SAFETY: parent stays open and the buffer is writable.
            let n = unsafe {
                GetFinalPathNameByHandleW(
                    parent.as_raw_handle(),
                    path.as_mut_ptr(),
                    path.len() as u32,
                    0,
                )
            } as usize;
            if n == 0 {
                return Err(io::Error::last_os_error());
            }
            if n < path.len() {
                path.truncate(n);
                break;
            }
            path.resize(n + 1, 0);
        }
        if !path.ends_with(&[u16::from(b'\\')]) {
            path.push(u16::from(b'\\'));
        }
        path.extend(leaf.encode_wide());
        path.push(0);
        Ok(path)
    }

    pub(super) fn replace_file(
        parent: &Dir,
        name: &Path,
        source: &Dir,
        incoming: &Path,
    ) -> io::Result<()> {
        let replaced = leaf_path(parent, name)?;
        let replacement = leaf_path(source, incoming)?;
        // SAFETY: both paths are NUL-terminated and the directory handles
        // remain open. Backup and the reserved pointers are optional.
        // Zero flags require Windows to report an ACL merge failure.
        if unsafe {
            ReplaceFileW(
                replaced.as_ptr(),
                replacement.as_ptr(),
                null(),
                0,
                null_mut(),
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn move_entry(from: &Dir, old: &Path, to: &Dir, new: &Path) -> io::Result<()> {
        let old = leaf_path(from, old)?;
        let new = leaf_path(to, new)?;
        // SAFETY: both paths are NUL-terminated and the directory handles
        // remain open. MoveFileW requires the new name to be vacant.
        if unsafe { MoveFileW(old.as_ptr(), new.as_ptr()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(windows)]
fn keep_changed_deletion(
    held_dir: &Dir,
    held: &Path,
    parent: &Dir,
    name: &Path,
    actual: &Snapshot,
) -> io::Result<()> {
    if *actual == Snapshot::Absent && restore_windows_held(held_dir, held, parent, name)? {
        return Err(io::Error::other(
            "A newer file was put back and retained in recovery after an interrupted deletion. The deletion is still pending.",
        ));
    }
    Err(io::Error::other(
        "A newer file is retained in recovery after an interrupted deletion. The deletion is still pending.",
    ))
}

#[cfg(any(windows, test))]
fn remove_unused_recovery_link(
    parent: &Dir,
    name: &Path,
    displaced: (&Dir, &Path),
) -> io::Result<()> {
    let (directory, recovery) = displaced;
    // Keep the retained inode whenever another save displaced the live path.
    if let (Ok(live), Ok(saved)) = (
        parent.symlink_metadata(name),
        directory.symlink_metadata(recovery),
    ) {
        if live.is_file() && saved.is_file() && same_identity(&live, &saved) {
            directory.remove_file(recovery)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn rulesync_windows_personal_replacement_does_not_overwrite_a_concurrent_save() {
        let (temp, root) = root();
        let path = Path::new("review.md");
        let seen = Snapshot::file(b"original".to_vec());
        let desired = Snapshot::file(b"desired".to_vec());
        assert_eq!(
            root.apply(path, &Snapshot::Absent, &seen, false).unwrap(),
            ApplyOutcome::Applied
        );
        let mut stage = Stage::new(&root.dir, None, true).unwrap();
        stage.write(&desired, Some(path)).unwrap();
        let previous = (root.dir.try_clone().unwrap(), Path::new("retained.md"));
        let applied = commit_windows(
            &mut stage,
            path,
            &seen,
            &desired,
            Some(&previous),
            true,
            || {},
            || {
                let editor = temp.path().join("editor.tmp");
                fs::write(&editor, b"editor's atomic save").unwrap();
                fs::rename(editor, temp.path().join(path)).unwrap();
            },
        )
        .unwrap();
        assert_eq!(applied, ApplyOutcome::ChangedSincePlan);
        assert_eq!(
            fs::read(temp.path().join(path)).unwrap(),
            b"editor's atomic save"
        );
        assert_eq!(fs::read(temp.path().join(previous.1)).unwrap(), b"original");
    }

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
    fn rulesync_key_file_post_rename_error_leaves_destination_and_recorded_temp_recoverable() {
        let (scratch, root) = root();
        let path = Path::new("settings.json");
        let temp = Path::new(".settings.json.wenlan-key-test.tmp");
        let seen = Snapshot::file(b"before".to_vec());
        let desired = Snapshot::file(b"after".to_vec());
        assert_eq!(
            root.apply(path, &Snapshot::Absent, &seen, false).unwrap(),
            ApplyOutcome::Applied
        );

        let error = root
            .apply_key_file_after(path, temp, &seen, &desired, || {
                Err(io::Error::other("injected post-rename failure"))
            })
            .unwrap_err();

        assert_eq!(error.to_string(), "injected post-rename failure");
        assert_eq!(fs::read(scratch.path().join(path)).unwrap(), b"after");
        assert!(!scratch.path().join(temp).exists());
        // The caller's durable intent remains necessary: cleanup and receipt
        // settlement happen only after this method returns successfully.
    }

    #[test]
    fn rulesync_key_temp_cleanup_removes_only_a_regular_recorded_file() {
        let (scratch, root) = root();
        let temp = Path::new(".settings.json.wenlan-key-test.tmp");
        fs::write(scratch.path().join(temp), b"partial settings").unwrap();
        root.cleanup_key_temp(Path::new("settings.json"), temp)
            .unwrap();
        assert!(!scratch.path().join(temp).exists());

        let directory = scratch.path().join(temp);
        fs::create_dir(&directory).unwrap();
        assert!(root
            .cleanup_key_temp(Path::new("settings.json"), temp)
            .is_err());
        assert!(directory.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_key_temp_cleanup_refuses_a_symlink() {
        let (scratch, root) = root();
        let temp = Path::new(".settings.json.wenlan-key-test.tmp");
        let target = scratch.path().join("user-file");
        fs::write(&target, b"leave this alone").unwrap();
        std::os::unix::fs::symlink("user-file", scratch.path().join(temp)).unwrap();

        assert!(root
            .cleanup_key_temp(Path::new("settings.json"), temp)
            .is_err());
        assert_eq!(fs::read(target).unwrap(), b"leave this alone");
        assert!(fs::symlink_metadata(scratch.path().join(temp))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(windows)]
    #[test]
    fn rulesync_key_file_windows_replaces_without_a_displaced_copy() {
        let (scratch, root) = root();
        let path = Path::new("settings.json");
        let temp = Path::new(".settings.json.wenlan-key-test.tmp");
        let seen = Snapshot::file(b"before".to_vec());
        let desired = Snapshot::file(b"after".to_vec());
        assert_eq!(
            root.apply(path, &Snapshot::Absent, &seen, false).unwrap(),
            ApplyOutcome::Applied
        );

        assert_eq!(
            root.apply_key_file(path, temp, &seen, &desired).unwrap(),
            ApplyOutcome::Applied
        );

        assert_eq!(fs::read(scratch.path().join(path)).unwrap(), b"after");
        assert_eq!(
            fs::read_dir(scratch.path()).unwrap().count(),
            1,
            "the settings replacement must not leave a displaced key-bearing copy"
        );
    }

    #[test]
    fn rulesync_step1g_windows_keeps_a_save_after_the_final_check() {
        let (scratch, root) = root();
        let name = Path::new("rules.md");
        let path = scratch.path().join(name);
        fs::write(&path, b"Checked rules\n").unwrap();
        let seen = root.read(name, false).unwrap();
        let incoming = scratch.path().join("editor-save.md");
        let saved = b"Editor saved after the last check\n";
        fs::write(&incoming, saved).unwrap();
        let recovery = Path::new("rules.previous");
        let desired = Snapshot::file(b"Generated rules\n".to_vec());
        let mut stage = Stage::new(&root.dir, None, false).unwrap();
        stage.write(&desired, None).unwrap();
        let result = commit_windows(
            &mut stage,
            name,
            &seen,
            &desired,
            Some(&(root.dir.try_clone().unwrap(), recovery)),
            false,
            || fs::rename(&incoming, &path).unwrap(),
            || {},
        )
        .unwrap();

        assert!(
            fs::read(&path).is_ok_and(|bytes| bytes == saved)
                || fs::read(scratch.path().join(recovery)).is_ok_and(|bytes| bytes == saved),
            "the editor's save vanished from the live path and the recovery copy"
        );
        assert_eq!(result, ApplyOutcome::ChangedSincePlan);

        // A second atomic save can displace the restored live link before
        // cleanup. The first save must remain reachable in recovery.
        let second = scratch.path().join("second-save.md");
        fs::write(&second, b"Second editor save\n").unwrap();
        fs::rename(&second, &path).unwrap();
        drop(stage);
        assert_eq!(fs::read(scratch.path().join(recovery)).unwrap(), saved);
        assert_eq!(fs::read(path).unwrap(), b"Second editor save\n");
    }

    #[cfg(windows)]
    fn icacls(path: &Path, args: &[&str]) -> Vec<u8> {
        let output = std::process::Command::new("icacls.exe")
            .arg(path)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        output.stdout
    }

    #[cfg(windows)]
    #[test]
    fn rulesync_step1j_windows_preserves_explicit_file_permissions() {
        for journaled in [false, true] {
            let (scratch, root) = root();
            let name = Path::new("rules.md");
            let path = scratch.path().join(name);
            fs::write(&path, b"Original rules\n").unwrap();
            icacls(&path, &["/grant", "*S-1-1-0:(R)"]);
            let permissions = icacls(&path, &[]);
            let seen = root.read(name, false).unwrap();
            let desired = Snapshot::file(b"Updated rules\n".to_vec());
            let previous = Path::new("previous");
            let options = ApplyOptions {
                displaced: journaled.then_some((&root, previous)),
                stage_id: journaled.then_some("00000000000000000000000000000000"),
                ..direct(false)
            };

            assert_eq!(
                root.apply_with(name, &seen, &desired, options).unwrap(),
                ApplyOutcome::Applied
            );
            assert_eq!(root.read(name, false).unwrap(), desired);
            assert_eq!(icacls(&path, &[]), permissions);
            if journaled {
                assert_eq!(root.read(previous, false).unwrap(), seen);
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn rulesync_step1j_windows_copies_directory_permissions_onto_its_replacement() {
        let (scratch, root) = root();
        let name = Path::new("skill");
        let path = scratch.path().join(name);
        fs::create_dir(&path).unwrap();
        icacls(&path, &["/grant", "*S-1-1-0:(R)"]);
        let seen = root.read(name, false).unwrap();
        let desired = Snapshot::file(b"Replacement\n".to_vec());

        assert_eq!(
            root.apply(name, &seen, &desired, false).unwrap(),
            ApplyOutcome::Applied
        );
        assert_eq!(root.read(name, false).unwrap(), desired);
        // The directory and file can inherit different rules. Granting R
        // again leaves the ACL unchanged only if that explicit ACE survived.
        let copied = icacls(&path, &[]);
        icacls(&path, &["/grant", "*S-1-1-0:(R)"]);
        assert_eq!(icacls(&path, &[]), copied);
    }

    #[test]
    fn rulesync_round2_unused_recovery_link_releases_only_a_still_live_inode() {
        let (temp, root) = root();
        let live = Path::new("live.md");
        let alias = Path::new("saved.md");
        let recovery = (root.dir.try_clone().unwrap(), alias);
        fs::write(temp.path().join(live), "Before.").unwrap();
        root.dir.hard_link(live, &recovery.0, alias).unwrap();
        fs::write(temp.path().join(live), "In-place edit.").unwrap();
        remove_unused_recovery_link(&root.dir, live, (&recovery.0, recovery.1)).unwrap();
        assert!(!temp.path().join(alias).exists());
        assert_eq!(
            fs::read_to_string(temp.path().join(live)).unwrap(),
            "In-place edit."
        );

        root.dir.hard_link(live, &recovery.0, alias).unwrap();
        fs::remove_file(temp.path().join(live)).unwrap();
        fs::write(temp.path().join(live), "New inode.").unwrap();
        remove_unused_recovery_link(&root.dir, live, (&recovery.0, recovery.1)).unwrap();
        assert_eq!(
            fs::read_to_string(temp.path().join(alias)).unwrap(),
            "In-place edit."
        );
        assert_eq!(
            fs::read_to_string(temp.path().join(live)).unwrap(),
            "New inode."
        );
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
            root.apply(name, &seen, &Snapshot::private_directory(), true)
                .unwrap(),
            ApplyOutcome::Applied
        );
        assert!(root.read(name, false).unwrap().is_directory());
        let late_child = path.join("personal-notes");
        let result = root.apply_inner(
            name,
            &Snapshot::private_directory(),
            &seen,
            direct(true),
            || {
                fs::write(&late_child, b"new skill notes").unwrap();
            },
        );
        assert_eq!(result.unwrap(), ApplyOutcome::ChangedSincePlan);
        assert_eq!(fs::read(&late_child).unwrap(), b"new skill notes");
        assert_eq!(
            fs::read_dir(scratch.path().join("source")).unwrap().count(),
            0
        );

        fs::remove_file(late_child).unwrap();
        assert_eq!(
            root.apply(name, &Snapshot::private_directory(), &seen, true)
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
