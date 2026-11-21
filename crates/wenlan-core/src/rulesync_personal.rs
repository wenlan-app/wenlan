// SPDX-License-Identifier: Apache-2.0
//! The personal master and the read-only side of adopting an existing setup.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use cap_fs_ext::{DirExt as _, MetadataExt as _};
use cap_std::fs::{Dir, Metadata};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wenlan_types::rulesync::{AdoptionChange, AdoptionPlan, AdoptionSkill, AdoptionSkillOutcome};
use wenlan_types::rulesync_skill::SavedSkillTool;

use crate::rulesync_adopt::{
    self, AdoptedSkill, Adoption, AdoptionEntry, AdoptionLink, AdoptionTarget,
};

#[path = "rulesync_delivery_worktrees.rs"]
mod delivery_worktrees;
#[path = "rulesync_skill_preview.rs"]
mod skill_preview;
pub use delivery_worktrees::DeliveryWorktrees;

/// Identify a single directory entry. Hard links share a file identity but
/// represent distinct entries.
pub fn directory_entry_identity(metadata: &Metadata) -> Option<(u64, u64)> {
    let inode = metadata.ino();
    (inode != 0 && (metadata.is_dir() || metadata.nlink() == 1)).then(|| (metadata.dev(), inode))
}

/// Return the listed spelling when a lookup opens a differently spelled
/// entry. The volume determines which spellings resolve together.
pub fn case_alias_in(directory: &Dir, name: &OsStr) -> io::Result<Option<OsString>> {
    let path = Path::new(name);
    if path.file_name() != Some(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a directory entry name",
        ));
    }
    let requested = match directory.symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    for entry in directory.entries()? {
        if entry?.file_name() == name {
            return Ok(None);
        }
    }
    // No literal name was listed. Match the entry only when its identity
    // distinguishes it from every hard link, including links in this folder.
    let identity = directory_entry_identity(&requested)
        .ok_or_else(|| io::Error::other("cannot identify an alternate directory spelling"))?;
    for entry in directory.entries()? {
        let other = entry?.file_name();
        if directory_entry_identity(&directory.symlink_metadata(&other)?) == Some(identity) {
            return Ok((other != name).then_some(other));
        }
    }
    Err(io::Error::other(
        "directory entry changed while checking its spelling",
    ))
}

pub const RULE_PRECEDENCE: &str =
    "When instructions disagree, project rules come first, followed by team rules and personal rules.";

#[cfg(any(windows, test))]
const WINDOWS_FILE_ALL_ACCESS: u32 = 0x001F_01FF;

/// One allow rule of a Windows access list: its inheritance flags, access
/// mask and the SID it grants to.
#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WindowsAce<'a> {
    flags: u8,
    mask: u32,
    sid: &'a [u8],
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy)]
struct WindowsAcl<'a> {
    owner: &'a [u8],
    protected: bool,
    has_security_policy: bool,
    aces: &'a [WindowsAce<'a>],
}

#[cfg(any(windows, test))]
const WINDOWS_OBJECT_INHERIT: u8 = 0x01;
#[cfg(any(windows, test))]
const WINDOWS_CONTAINER_INHERIT: u8 = 0x02;
#[cfg(any(windows, test))]
const WINDOWS_NO_PROPAGATE: u8 = 0x04;
#[cfg(any(windows, test))]
const WINDOWS_INHERIT_ONLY: u8 = 0x08;
#[cfg(any(windows, test))]
const WINDOWS_INHERITED: u8 = 0x10;

/// The file-object meaning of a mask's generic bits (FILE_GENERIC_*).
#[cfg(any(windows, test))]
fn windows_file_mask(mask: u32) -> u32 {
    let mut mapped = mask & 0x0FFF_FFFF;
    for (generic, specific) in [
        (0x8000_0000, 0x0012_0089),
        (0x4000_0000, 0x0012_0116),
        (0x2000_0000, 0x0012_00A0),
        (0x1000_0000, WINDOWS_FILE_ALL_ACCESS),
    ] {
        if mask & generic != 0 {
            mapped |= specific;
        }
    }
    mapped
}

/// What an entry's own rules grant (`effective`) and what it hands to its
/// own children (`inheritable`), each sorted for comparison.
#[cfg(any(windows, test))]
type WindowsGrants<'a> = (Vec<(u32, &'a [u8])>, Vec<(u8, u32, &'a [u8])>);

#[cfg(any(windows, test))]
fn windows_entry_grants<'a>(aces: &[WindowsAce<'a>]) -> WindowsGrants<'a> {
    let mut effective = Vec::new();
    let mut inheritable = Vec::new();
    for ace in aces {
        if ace.flags & WINDOWS_INHERIT_ONLY == 0 {
            effective.push((windows_file_mask(ace.mask), ace.sid));
        }
        let passes = ace.flags & (WINDOWS_OBJECT_INHERIT | WINDOWS_CONTAINER_INHERIT);
        if passes != 0 {
            inheritable.push((
                ace.flags & (passes | WINDOWS_NO_PROPAGATE),
                ace.mask,
                ace.sid,
            ));
        }
    }
    effective.sort_unstable();
    inheritable.sort_unstable();
    (effective, inheritable)
}

/// What a new child of `parent` would get from inheritance, following the
/// Windows inheritance rules for allow rules. `creator_owner` is the
/// CREATOR OWNER SID, which an effective rule replaces with `owner`.
#[cfg(any(windows, test))]
fn windows_inherited_grants<'a>(
    parent: &[WindowsAce<'a>],
    directory: bool,
    owner: &'a [u8],
    creator_owner: &[u8],
) -> WindowsGrants<'a> {
    let mut effective = Vec::new();
    let mut inheritable = Vec::new();
    for ace in parent {
        let object = ace.flags & WINDOWS_OBJECT_INHERIT != 0;
        let container = ace.flags & WINDOWS_CONTAINER_INHERIT != 0;
        let applies = if directory { container } else { object };
        if applies {
            let sid = if ace.sid == creator_owner {
                owner
            } else {
                ace.sid
            };
            effective.push((windows_file_mask(ace.mask), sid));
        }
        if directory && (object || container) && ace.flags & WINDOWS_NO_PROPAGATE == 0 {
            let passes = ace.flags & (WINDOWS_OBJECT_INHERIT | WINDOWS_CONTAINER_INHERIT);
            inheritable.push((passes, ace.mask, ace.sid));
        }
    }
    effective.sort_unstable();
    inheritable.sort_unstable();
    (effective, inheritable)
}

/// An entry is safe to replace when a new entry created in the same parent
/// gets exactly the same access: the owner matches the parent's, there is
/// no integrity label or access policy, the access list is not protected,
/// every rule is inherited, and the rules equal what the parent passes down.
/// Explicit, protected, labelled or foreign-owned entries are refused.
#[cfg(any(windows, test))]
fn windows_acl_problem(
    acl: &WindowsAcl<'_>,
    directory: bool,
    parent_owner: &[u8],
    parent_aces: &[WindowsAce<'_>],
    creator_owner: &[u8],
) -> Option<&'static str> {
    if parent_owner != acl.owner {
        return Some("the Windows owner differs from the parent's owner");
    }
    if acl.has_security_policy {
        return Some("the Windows security descriptor has an integrity label or access policy");
    }
    if acl.protected {
        return Some("the Windows access list is protected from inheritance");
    }
    if acl.aces.is_empty() {
        return Some("the Windows access list is empty");
    }
    if acl
        .aces
        .iter()
        .any(|ace| ace.flags & WINDOWS_INHERITED == 0)
    {
        return Some("the Windows access list has an explicit entry");
    }
    (windows_entry_grants(acl.aces)
        != windows_inherited_grants(parent_aces, directory, acl.owner, creator_owner))
    .then_some("the Windows access list differs from what its folder passes down")
}

#[cfg(any(windows, test))]
fn windows_sacl_problem(protected: bool, ace_flags: &[u8]) -> Option<&'static str> {
    const INHERITED_ACE: u8 = 0x10;
    if protected {
        return Some("the Windows system access list is protected");
    }
    ace_flags
        .iter()
        .any(|flags| flags & INHERITED_ACE == 0)
        .then_some("the Windows system access list contains an explicit entry")
}

#[cfg(test)]
#[test]
fn rulesync_step1i_windows_rejects_explicit_auditing_and_protected_sacls() {
    assert_eq!(windows_sacl_problem(false, &[]), None);
    assert_eq!(windows_sacl_problem(false, &[0x50, 0x90]), None);
    assert_eq!(
        windows_sacl_problem(true, &[]),
        Some("the Windows system access list is protected")
    );
    assert_eq!(
        windows_sacl_problem(false, &[0x50, 0x80]),
        Some("the Windows system access list contains an explicit entry")
    );
}

#[cfg(test)]
mod windows_acl_rule_tests {
    use super::{windows_acl_problem, WindowsAce, WindowsAcl, WINDOWS_FILE_ALL_ACCESS};

    const OWNER: &[u8] = b"owner SID";
    const CREATOR_OWNER: &[u8] = b"CREATOR OWNER";
    const FULL: u32 = WINDOWS_FILE_ALL_ACCESS;
    const READ: u32 = 0x0012_0089;

    fn ace(flags: u8, mask: u32, sid: &'static [u8]) -> WindowsAce<'static> {
        WindowsAce { flags, mask, sid }
    }

    fn problem(
        aces: &[WindowsAce<'_>],
        directory: bool,
        parent: &[WindowsAce<'_>],
    ) -> Option<&'static str> {
        let acl = WindowsAcl {
            owner: OWNER,
            protected: false,
            has_security_policy: false,
            aces,
        };
        windows_acl_problem(&acl, directory, OWNER, parent, CREATOR_OWNER)
    }

    /// A user profile folder passes full control to SYSTEM, Administrators
    /// and the user; each file under it inherits those three.
    fn profile_parent() -> [WindowsAce<'static>; 3] {
        [
            ace(0x03, FULL, b"SYSTEM"),
            ace(0x03, FULL, b"Administrators"),
            ace(0x03, FULL, OWNER),
        ]
    }

    #[test]
    fn rulesync_windows_accepts_the_profile_layout() {
        let parent = profile_parent();
        let file = [
            ace(0x10, FULL, b"SYSTEM"),
            ace(0x10, FULL, b"Administrators"),
            ace(0x10, FULL, OWNER),
        ];
        assert_eq!(problem(&file, false, &parent), None);
        let directory = [
            ace(0x13, FULL, b"SYSTEM"),
            ace(0x13, FULL, b"Administrators"),
            ace(0x13, FULL, OWNER),
        ];
        assert_eq!(problem(&directory, true, &parent), None);
    }

    /// A temporary or second-drive folder (like a CI runner's D:\a\_temp)
    /// passes down a plain but different list, with a generic CREATOR OWNER
    /// rule and a read-only grant to Users. A file or folder that only
    /// inherits that list is accepted.
    #[test]
    fn rulesync_windows_accepts_a_plain_inherited_list_that_differs_from_the_profile_default() {
        let parent = [
            ace(0x03, FULL, b"SYSTEM"),
            ace(0x03, FULL, b"Administrators"),
            ace(0x0B, 0x1000_0000, CREATOR_OWNER),
            ace(0x03, READ, b"Users"),
        ];
        let file = [
            ace(0x10, FULL, b"SYSTEM"),
            ace(0x10, FULL, b"Administrators"),
            ace(0x10, FULL, OWNER),
            ace(0x10, READ, b"Users"),
        ];
        assert_eq!(problem(&file, false, &parent), None);
        let directory = [
            ace(0x13, FULL, b"SYSTEM"),
            ace(0x13, FULL, b"Administrators"),
            ace(0x10, FULL, OWNER),
            ace(0x1B, 0x1000_0000, CREATOR_OWNER),
            ace(0x13, READ, b"Users"),
        ];
        assert_eq!(problem(&directory, true, &parent), None);
    }

    #[test]
    fn rulesync_windows_refuses_entries_a_replacement_would_not_match() {
        let parent = profile_parent();
        for (directory, flags) in [(false, 0x10), (true, 0x13)] {
            let inherited = [
                ace(flags, FULL, b"SYSTEM"),
                ace(flags, FULL, b"Administrators"),
                ace(flags, FULL, OWNER),
            ];
            let base = WindowsAcl {
                owner: OWNER,
                protected: false,
                has_security_policy: false,
                aces: &inherited,
            };
            let explicit = [inherited[0], inherited[1], ace(flags & !0x10, FULL, OWNER)];
            let restrictive = [inherited[0], inherited[1], ace(flags, READ, OWNER)];
            let extra = [
                inherited[0],
                inherited[1],
                inherited[2],
                ace(flags, FULL, b"another user"),
            ];
            let missing = [inherited[0], inherited[2]];
            let cases = [
                (
                    "explicit rule",
                    WindowsAcl {
                        aces: &explicit,
                        ..base
                    },
                    OWNER,
                ),
                (
                    "restrictive inherited rule",
                    WindowsAcl {
                        aces: &restrictive,
                        ..base
                    },
                    OWNER,
                ),
                (
                    "rule the folder does not pass down",
                    WindowsAcl {
                        aces: &extra,
                        ..base
                    },
                    OWNER,
                ),
                (
                    "missing inherited rule",
                    WindowsAcl {
                        aces: &missing,
                        ..base
                    },
                    OWNER,
                ),
                ("empty list", WindowsAcl { aces: &[], ..base }, OWNER),
                (
                    "protected list",
                    WindowsAcl {
                        protected: true,
                        ..base
                    },
                    OWNER,
                ),
                (
                    "integrity label",
                    WindowsAcl {
                        has_security_policy: true,
                        ..base
                    },
                    OWNER,
                ),
                (
                    "foreign owner",
                    WindowsAcl {
                        owner: b"other SID",
                        ..base
                    },
                    OWNER,
                ),
            ];
            let mut accepted = Vec::new();
            for (case, acl, parent_owner) in cases {
                if windows_acl_problem(&acl, directory, parent_owner, &parent, CREATOR_OWNER)
                    .is_none()
                {
                    accepted.push(case);
                }
            }
            assert!(
                accepted.is_empty(),
                "accepted {accepted:?}, directory={directory}"
            );
        }
    }

    /// A folder whose own list passes nothing further down does not match a
    /// parent that passes its rules on, since a replacement would inherit them.
    #[test]
    fn rulesync_windows_compares_what_a_folder_passes_to_its_children() {
        let parent = profile_parent();
        let no_propagation = [
            ace(0x10, FULL, b"SYSTEM"),
            ace(0x10, FULL, b"Administrators"),
            ace(0x10, FULL, OWNER),
        ];
        assert!(problem(&no_propagation, true, &parent).is_some());
        let file_with_inheritance = [
            ace(0x13, FULL, b"SYSTEM"),
            ace(0x13, FULL, b"Administrators"),
            ace(0x13, FULL, OWNER),
        ];
        assert!(problem(&file_with_inheritance, false, &parent).is_some());
    }
}

#[cfg(windows)]
pub use windows_acl::{
    default_windows_child_acl, open_windows_tool_dir, windows_access_error, windows_owner,
};

#[cfg(windows)]
mod windows_acl {
    use super::{windows_acl_problem, windows_sacl_problem, WindowsAce, WindowsAcl};
    use cap_fs_ext::DirExt as _;
    use cap_std::fs::Dir;
    use std::{
        ffi::c_void,
        io,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
        path::Path,
        ptr::null_mut,
        slice,
    };

    const SE_FILE_OBJECT: u32 = 1;
    const OWNER_SECURITY_INFORMATION: u32 = 1;
    const DACL_SECURITY_INFORMATION: u32 = 4;
    const SACL_SECURITY_INFORMATION: u32 = 8;
    const LABEL_SECURITY_INFORMATION: u32 = 0x10;
    const ATTRIBUTE_SECURITY_INFORMATION: u32 = 0x20;
    const SCOPE_SECURITY_INFORMATION: u32 = 0x40;
    const SE_DACL_PROTECTED: u16 = 0x1000;
    const SE_SACL_PROTECTED: u16 = 0x2000;
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    // S-1-3-0, the CREATOR OWNER placeholder an inherited rule replaces with the owner.
    const CREATOR_OWNER_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0];
    const READ_CONTROL: u32 = 0x0002_0000;
    const ACCESS_SYSTEM_SECURITY: u32 = 0x0100_0000;
    const SHARE_READ_WRITE_DELETE: u32 = 0x7;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;

    #[repr(C)]
    struct AclHeader {
        revision: u8,
        reserved: u8,
        size: u16,
        ace_count: u16,
        reserved2: u16,
    }

    #[repr(C)]
    struct AceHeader {
        kind: u8,
        flags: u8,
        size: u16,
    }

    #[repr(C)]
    struct AllowAce {
        header: AceHeader,
        mask: u32,
        sid_start: u32,
    }

    struct AccessGrant<'a> {
        flags: u8,
        mask: u32,
        sid: &'a [u8],
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn GetSecurityInfo(
            handle: RawHandle,
            object_type: u32,
            security_info: u32,
            owner: *mut *mut c_void,
            group: *mut *mut c_void,
            dacl: *mut *mut AclHeader,
            sacl: *mut *mut AclHeader,
            descriptor: *mut *mut c_void,
        ) -> u32;
        fn GetSecurityDescriptorControl(
            descriptor: *mut c_void,
            control: *mut u16,
            revision: *mut u32,
        ) -> i32;
        fn GetSecurityDescriptorSacl(
            descriptor: *mut c_void,
            present: *mut i32,
            sacl: *mut *mut AclHeader,
            defaulted: *mut i32,
        ) -> i32;
        fn GetAce(dacl: *mut AclHeader, index: u32, ace: *mut *mut c_void) -> i32;
        fn GetLengthSid(sid: *mut c_void) -> u32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
        fn ReOpenFile(handle: RawHandle, access: u32, share: u32, flags: u32) -> RawHandle;
    }

    struct SecurityDescriptor {
        ptr: *mut c_void,
        owner: *mut c_void,
        dacl: *mut AclHeader,
    }

    impl SecurityDescriptor {
        fn read(entry: &impl AsRawHandle, flags: u32) -> io::Result<Self> {
            let mut sd = Self {
                ptr: null_mut(),
                owner: null_mut(),
                dacl: null_mut(),
            };
            let dacl = if flags & DACL_SECURITY_INFORMATION == 0 {
                null_mut()
            } else {
                &mut sd.dacl
            };
            // SAFETY: the handle stays open and the output pointers are writable.
            // The owner and ACL are stored inside the owned descriptor.
            let status = unsafe {
                GetSecurityInfo(
                    entry.as_raw_handle(),
                    SE_FILE_OBJECT,
                    flags | OWNER_SECURITY_INFORMATION,
                    &mut sd.owner,
                    null_mut(),
                    dacl,
                    null_mut(),
                    &mut sd.ptr,
                )
            };
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            if sd.ptr.is_null() || sd.owner.is_null() {
                return Err(io::Error::other("the Windows owner is unavailable"));
            }
            Ok(sd)
        }

        fn owner(&self) -> &[u8] {
            // SAFETY: GetSecurityInfo supplied a valid owner SID in the
            // descriptor. The slice is tied to the descriptor's lifetime.
            unsafe { slice::from_raw_parts(self.owner.cast(), GetLengthSid(self.owner) as usize) }
        }

        fn control(&self) -> io::Result<u16> {
            let mut control = 0;
            let mut revision = 0;
            // SAFETY: the descriptor is live and both outputs are writable.
            if unsafe { GetSecurityDescriptorControl(self.ptr, &mut control, &mut revision) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(control)
        }

        fn sacl(&self) -> io::Result<*mut AclHeader> {
            let mut present = 0;
            let mut list = null_mut();
            let mut defaulted = 0;
            // SAFETY: the descriptor is live and all outputs are writable.
            if unsafe {
                GetSecurityDescriptorSacl(self.ptr, &mut present, &mut list, &mut defaulted)
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(if present != 0 { list } else { null_mut() })
        }

        fn allow_aces(&self) -> io::Result<Vec<AccessGrant<'_>>> {
            // SAFETY: the DACL and each returned ACE belong to the live descriptor.
            let list = unsafe { self.dacl.as_ref() }
                .ok_or_else(|| io::Error::other("the Windows access list is unavailable"))?;
            (0..u32::from(list.ace_count))
                .map(|index| {
                    let mut ace = null_mut();
                    // SAFETY: index is within the ACL and the output is writable.
                    if unsafe { GetAce(self.dacl, index, &mut ace) } == 0 {
                        return Err(io::Error::last_os_error());
                    }
                    // SAFETY: GetAce supplies an entry that starts with ACE_HEADER.
                    let header = unsafe { ace.cast::<AceHeader>().as_ref() }
                        .ok_or_else(|| io::Error::other("a Windows access rule is unavailable"))?;
                    if header.kind != ACCESS_ALLOWED_ACE_TYPE
                        || usize::from(header.size) < std::mem::size_of::<AllowAce>()
                    {
                        return Err(io::Error::other(
                            "the Windows access list contains a nonstandard rule",
                        ));
                    }
                    let ace = ace.cast::<AllowAce>();
                    // SAFETY: this is a standard allow ACE with a variable-length
                    // SID starting at SidStart in the live descriptor buffer.
                    let grant = unsafe {
                        let sid = std::ptr::addr_of!((*ace).sid_start)
                            .cast_mut()
                            .cast::<c_void>();
                        AccessGrant {
                            flags: header.flags,
                            mask: (*ace).mask,
                            sid: slice::from_raw_parts(sid.cast(), GetLengthSid(sid) as usize),
                        }
                    };
                    Ok(grant)
                })
                .collect()
        }
    }

    impl Drop for SecurityDescriptor {
        fn drop(&mut self) {
            if !self.ptr.is_null() {
                // SAFETY: GetSecurityInfo allocates this buffer for LocalFree.
                unsafe { LocalFree(self.ptr) };
            }
        }
    }

    fn unmanaged(error: io::Error) -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!("left unmanaged: could not preserve Windows permissions ({error})"),
        )
    }

    pub fn windows_access_error(error: io::Error) -> io::Error {
        if error.kind() == io::ErrorKind::PermissionDenied {
            unmanaged(error)
        } else {
            error
        }
    }

    pub fn windows_owner(entry: &impl AsRawHandle) -> io::Result<Vec<u8>> {
        SecurityDescriptor::read(entry, 0)
            .map(|sd| sd.owner().to_vec())
            .map_err(unmanaged)
    }

    pub fn default_windows_child_acl(
        parent: &Dir,
        entry: &impl AsRawHandle,
        directory: bool,
    ) -> io::Result<Vec<u8>> {
        default_windows_acl(parent, entry, directory).map_err(unmanaged)
    }

    pub fn open_windows_tool_dir(parent: &Dir, name: &Path) -> io::Result<Dir> {
        let directory = parent
            .open_dir_nofollow(name)
            .map_err(windows_access_error)?;
        default_windows_child_acl(parent, &directory, true)?;
        Ok(directory)
    }

    fn check_windows_audit(entry: &impl AsRawHandle) -> io::Result<()> {
        // SAFETY: reopen the same live object without a new pathname lookup.
        let handle = unsafe {
            ReOpenFile(
                entry.as_raw_handle(),
                READ_CONTROL | ACCESS_SYSTEM_SECURITY,
                SHARE_READ_WRITE_DELETE,
                FILE_FLAG_BACKUP_SEMANTICS,
            )
        };
        if handle as isize == -1 {
            let error = io::Error::last_os_error();
            // Reading audit entries requires an enabled SeSecurityPrivilege.
            // The ordinary READ_CONTROL checks still apply when it is unavailable.
            return match error.raw_os_error() {
                Some(ERROR_ACCESS_DENIED | ERROR_PRIVILEGE_NOT_HELD) => Ok(()),
                _ => Err(error),
            };
        }
        // SAFETY: ReOpenFile returned a new valid handle and transfers ownership.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        let audit = SecurityDescriptor::read(&handle, SACL_SECURITY_INFORMATION)?;
        let protected = audit.control()? & SE_SACL_PROTECTED != 0;
        let sacl = audit.sacl()?;
        let mut flags = Vec::new();
        // SAFETY: any returned SACL is owned by the live descriptor.
        if let Some(list) = unsafe { sacl.as_ref() } {
            for index in 0..u32::from(list.ace_count) {
                let mut ace = null_mut();
                // SAFETY: index is within the ACL and ace is writable.
                if unsafe { GetAce(sacl, index, &mut ace) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                // SAFETY: GetAce returned an entry starting with an ACE_HEADER.
                let header = unsafe { ace.cast::<AceHeader>().as_ref() }
                    .ok_or_else(|| io::Error::other("a Windows audit entry is unavailable"))?;
                flags.push(header.flags);
            }
        }
        if let Some(problem) = windows_sacl_problem(protected, &flags) {
            return Err(io::Error::other(problem));
        }
        Ok(())
    }

    fn default_windows_acl(
        parent: &Dir,
        entry: &impl AsRawHandle,
        directory: bool,
    ) -> io::Result<Vec<u8>> {
        // These access-policy entries can be queried with READ_CONTROL.
        let sd = SecurityDescriptor::read(
            entry,
            DACL_SECURITY_INFORMATION
                | LABEL_SECURITY_INFORMATION
                | ATTRIBUTE_SECURITY_INFORMATION
                | SCOPE_SECURITY_INFORMATION,
        )?;
        let control = sd.control()?;
        if let Some(problem) = windows_sacl_problem(control & SE_SACL_PROTECTED != 0, &[]) {
            return Err(io::Error::other(problem));
        }
        let policy = sd.sacl()?;
        // SAFETY: any returned SACL is stored in the live descriptor.
        let has_security_policy =
            unsafe { policy.as_ref() }.is_some_and(|list| list.ace_count != 0);
        let parent = SecurityDescriptor::read(parent, DACL_SECURITY_INFORMATION)?;
        let grants = |sd: &SecurityDescriptor| -> io::Result<Vec<(u8, u32, Vec<u8>)>> {
            Ok(sd
                .allow_aces()?
                .into_iter()
                .map(|ace| (ace.flags, ace.mask, ace.sid.to_vec()))
                .collect())
        };
        let entry_grants = grants(&sd)?;
        let parent_grants = grants(&parent)?;
        let view = |grants: &[(u8, u32, Vec<u8>)]| -> Vec<WindowsAce<'_>> {
            grants
                .iter()
                .map(|(flags, mask, sid)| WindowsAce {
                    flags: *flags,
                    mask: *mask,
                    sid,
                })
                .collect()
        };
        let aces = view(&entry_grants);
        let parent_aces = view(&parent_grants);
        let acl = WindowsAcl {
            owner: sd.owner(),
            protected: control & SE_DACL_PROTECTED != 0,
            has_security_policy,
            aces: &aces,
        };
        if let Some(problem) = windows_acl_problem(
            &acl,
            directory,
            parent.owner(),
            &parent_aces,
            &CREATOR_OWNER_SID,
        ) {
            return Err(io::Error::other(problem));
        }
        check_windows_audit(entry)?;
        Ok(acl.owner.to_vec())
    }
}

/// The harness is a migration source. Personal delivery owns paths in the
/// tools' own directories and never owns any part of the harness checkout.
pub fn is_harness_path(path: &Path) -> bool {
    matches!(path.components().next(), Some(Component::Normal(name))
        if name.as_encoded_bytes().eq_ignore_ascii_case(b".agents"))
}

/// Compare directory identities so aliases and case differences cannot turn
/// the migration source into a write destination.
pub struct HarnessGuard(Option<(u64, u64)>);

impl HarnessGuard {
    pub fn new(home: &Path) -> std::io::Result<Self> {
        let harness = home.join(".agents");
        match Dir::open_ambient_dir(&harness, cap_std::ambient_authority()) {
            Ok(dir) => {
                let metadata = dir.dir_metadata()?;
                Ok(Self(Some((metadata.dev(), metadata.ino()))))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self(None)),
            Err(error) => Err(error),
        }
    }

    pub fn check(&self, metadata: &Metadata) -> std::io::Result<()> {
        if self.0 == Some((metadata.dev(), metadata.ino())) {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "inside the read-only ~/.agents harness. Choose a path outside it",
            ))
        } else {
            Ok(())
        }
    }

    /// Data and its staging directories can be placed outside the tool home.
    /// Check the canonical ancestors of their selected root.
    pub fn check_absolute(&self, path: &Path) -> std::io::Result<()> {
        for ancestor in path.canonicalize()?.ancestors() {
            let dir = Dir::open_ambient_dir(ancestor, cap_std::ambient_authority())?;
            self.check(&dir.dir_metadata()?)?;
        }
        Ok(())
    }
}

/// Preview only existing physical parents. A planned adoption may replace a
/// symlinked skill folder before applying its child entries.
pub fn check_delivery_paths<'a>(
    home: &Path,
    paths: impl IntoIterator<Item = &'a Path>,
) -> Result<()> {
    let guard = HarnessGuard::new(home).context("inspect the ~/.agents harness")?;
    let root = Dir::open_ambient_dir(home, cap_std::ambient_authority())?;
    for path in paths {
        ensure!(
            !is_harness_path(path),
            "{} is inside the read-only ~/.agents harness. Choose a tool path outside it.",
            path.display()
        );
        let mut directory = root.try_clone()?;
        guard.check(&directory.dir_metadata()?)?;
        for component in path.parent().into_iter().flat_map(Path::components) {
            let Component::Normal(name) = component else {
                bail!("delivery path must be relative to the tool home");
            };
            directory = match directory.open_dir_nofollow(name) {
                Ok(child) => child,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => {
                    if directory
                        .symlink_metadata(name)
                        .is_ok_and(|meta| meta.is_symlink())
                    {
                        break;
                    }
                    return Err(error).with_context(|| format!("inspect {}", path.display()));
                }
            };
            guard
                .check(&directory.dir_metadata()?)
                .map_err(|error| anyhow::anyhow!("{} is {error}", path.display()))?;
        }
    }
    Ok(())
}

/// One switch for every writer to the person's tool folders.
pub fn enabled() -> bool {
    std::env::var_os("WENLAN_RULESYNC").is_some_and(|value| value == "1")
}

/// The CLI and daemon resolve the same root, including the existing dev isolation.
pub fn system_home() -> Result<PathBuf> {
    if let Some(state) = std::env::var_os("WENLAN_DEV_STATE_DIR").filter(|value| !value.is_empty())
    {
        return Ok(PathBuf::from(state).join("ai-tools/home"));
    }
    dirs::home_dir().context("cannot find the home directory")
}

/// One v43.11.0 global output, with a home-relative delivery path.
#[derive(Clone, Copy)]
pub struct GlobalOutput {
    pub renderer: &'static str,
    pub staged: &'static str,
    pub destination: &'static str,
}

impl GlobalOutput {
    const fn native(renderer: &'static str, path: &'static str) -> Self {
        Self::mapped(renderer, path, path)
    }

    const fn mapped(
        renderer: &'static str,
        staged: &'static str,
        destination: &'static str,
    ) -> Self {
        Self {
            renderer,
            staged,
            destination,
        }
    }
}

pub struct PersonalTarget {
    /// Stable key in the adoption master and delivery manifest.
    pub id: &'static str,
    pub agent: &'static str,
    pub tool: SavedSkillTool,
    pub rules: GlobalOutput,
    pub skills: GlobalOutput,
}

impl PersonalTarget {
    pub fn folder(&self) -> &Path {
        Path::new(self.rules.destination)
            .parent()
            .expect("tool folder")
    }

    fn adoption_input(&self) -> AdoptionTarget {
        AdoptionTarget {
            tool: self.id.into(),
            rules_path: Some(self.rules.destination.into()),
            skills_path: Some(self.skills.destination.into()),
        }
    }

    fn skipped_note(&self) -> String {
        format!(
            "Skipped {} because ~/{} is absent.",
            self.tool.display_name(),
            plan_path(self.folder())
        )
    }
}

const CLAUDE_TARGET: PersonalTarget = PersonalTarget {
    id: "claudecode",
    agent: "claude-code",
    tool: SavedSkillTool::ClaudeCode,
    rules: GlobalOutput::native("claudecode", ".claude/CLAUDE.md"),
    skills: GlobalOutput::native("claudecode", ".claude/skills"),
};

const CODEX_TARGET: PersonalTarget = PersonalTarget {
    id: "codexcli",
    agent: "codex",
    tool: SavedSkillTool::Codex,
    rules: GlobalOutput::native("codexcli", ".codex/AGENTS.md"),
    skills: GlobalOutput::mapped("codexcli", ".agents/skills", ".codex/skills"),
};

pub const PERSONAL_TARGETS: &[PersonalTarget] = &[
    CLAUDE_TARGET,
    CODEX_TARGET,
    PersonalTarget {
        id: "geminicli",
        agent: "gemini-cli",
        tool: SavedSkillTool::GeminiCli,
        rules: GlobalOutput::native("antigravity-ide", ".gemini/GEMINI.md"),
        skills: GlobalOutput::mapped("opencode", ".config/opencode/skills", ".gemini/skills"),
    },
    PersonalTarget {
        id: "antigravity-ide",
        agent: "antigravity",
        tool: SavedSkillTool::Antigravity,
        rules: GlobalOutput::mapped(
            "antigravity-ide",
            ".gemini/GEMINI.md",
            ".gemini/config/GEMINI.md",
        ),
        skills: GlobalOutput::native("antigravity-ide", ".gemini/config/skills"),
    },
    PersonalTarget {
        id: "copilotcli",
        agent: "copilot-cli",
        tool: SavedSkillTool::CopilotCli,
        rules: GlobalOutput::native("copilotcli", ".copilot/copilot-instructions.md"),
        skills: GlobalOutput::native("copilotcli", ".copilot/skills"),
    },
    PersonalTarget {
        id: "cursor",
        agent: "cursor",
        tool: SavedSkillTool::Cursor,
        rules: GlobalOutput::mapped("codexcli", ".codex/AGENTS.md", ".cursor/AGENTS.md"),
        skills: GlobalOutput::native("cursor", ".cursor/skills"),
    },
    PersonalTarget {
        id: "opencode",
        agent: "opencode",
        tool: SavedSkillTool::OpenCode,
        rules: GlobalOutput::native("opencode", ".config/opencode/AGENTS.md"),
        skills: GlobalOutput::native("opencode", ".config/opencode/skills"),
    },
    PersonalTarget {
        id: "musecode",
        agent: "muse-code",
        tool: SavedSkillTool::MuseCode,
        rules: GlobalOutput::mapped("codexcli", ".codex/AGENTS.md", ".config/muse/AGENTS.md"),
        skills: GlobalOutput::native("musecode", ".config/muse/skills"),
    },
];

pub fn personal_target(id: &str) -> Option<&'static PersonalTarget> {
    PERSONAL_TARGETS.iter().find(|target| target.id == id)
}

pub fn discover_adoption(home: &Path) -> Result<Adoption> {
    let (installed, skipped) = installed_targets(home, PERSONAL_TARGETS.iter())?;
    let mut adoption = discover_targets(home, &installed, None)?;
    adoption.skipped_tools = skipped;
    Ok(adoption)
}

pub fn additional_targets(
    home: &Path,
    master: &PersonalMaster,
) -> Result<Vec<&'static PersonalTarget>> {
    let candidates = PERSONAL_TARGETS
        .iter()
        .filter(|target| !master.enrolls_target(target.id));
    Ok(installed_targets(home, candidates)?.0)
}

pub fn discover_additional_adoption(home: &Path, master: &PersonalMaster) -> Result<Adoption> {
    let installed = additional_targets(home, master)?;
    let mut adoption = discover_targets(home, &installed, Some(master))?;
    adoption.added_tools = installed
        .iter()
        .map(|target| target.tool.display_name().to_owned())
        .collect();
    Ok(adoption)
}

fn installed_targets<'a>(
    home: &Path,
    candidates: impl Iterator<Item = &'a PersonalTarget>,
) -> Result<(Vec<&'a PersonalTarget>, Vec<String>)> {
    let root = Dir::open_ambient_dir(home, cap_std::ambient_authority())?;
    let mut targets = Vec::new();
    let mut skipped_tools = Vec::new();
    for target in candidates {
        match root.symlink_metadata(target.folder()) {
            Ok(_) => targets.push(target),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                skipped_tools.push(target.skipped_note());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect {}", target.folder().display()));
            }
        }
    }
    Ok((targets, skipped_tools))
}

fn discover_targets(
    home: &Path,
    installed: &[&PersonalTarget],
    master: Option<&PersonalMaster>,
) -> Result<Adoption> {
    let targets: Vec<_> = installed
        .iter()
        .map(|target| target.adoption_input())
        .collect();
    let mut adoption = if let Some(master) = master {
        rulesync_adopt::discover_with_shared(home, &targets, &master.shared)?
    } else {
        rulesync_adopt::discover(home, &targets)?
    };
    if master.is_some() {
        adoption.skills.retain(|skill| !skill.locations.is_empty());
        adoption.skipped.retain(|path, _| !is_harness_path(path));
    }
    check_delivery_paths(home, adoption.files.keys().map(PathBuf::as_path))?;
    delivery_worktrees::review_destinations(home, &mut adoption, &targets)?;
    Ok(adoption)
}

fn legacy_skill_previews(adoption: &mut Adoption) {
    for skill in &mut adoption.skills {
        let entry = Path::new("SKILL.md");
        skill.locations.retain(|target, folders| {
            match skill_preview::preview(target, &skill.files[entry]) {
                Ok(rendered) => {
                    for folder in folders.iter() {
                        if let Some(file) = adoption.files.get_mut(&folder.join(entry)) {
                            file.after = AdoptionEntry::File(rendered.clone());
                        }
                    }
                    true
                }
                Err(error) => {
                    for folder in folders.iter() {
                        adoption.files.retain(|path, _| !path.starts_with(folder));
                        adoption.links.remove(folder);
                        adoption.skill_locations.remove(folder);
                        adoption.skipped.insert(
                            folder.clone(),
                            format!("cannot preview the delivered SKILL.md ({error:#})"),
                        );
                    }
                    false
                }
            }
        });
        skill
            .targets
            .retain(|target| skill.locations.contains_key(target));
    }
}

/// Convert the installed metadata to rulesync's source representation when
/// staging an adopted skill for this tool. The master retains the original.
pub fn stage_adopted_skill(target: &str, skill_md: &[u8]) -> Result<Vec<u8>> {
    skill_preview::stage(target, skill_md)
}

pub fn check_skill_frontmatter(source: &[u8], rendered: &[u8]) -> Result<()> {
    skill_preview::check_frontmatter(source, rendered)
}

pub fn fingerprint(value: &impl Serialize) -> Result<String> {
    let mut hash = Sha256::new();
    {
        let mut buffered = BufWriter::new(&mut hash);
        serde_json::to_writer(&mut buffered, value)?;
        buffered.flush()?;
    }
    Ok(format!("{:x}", hash.finalize()))
}

const ADOPTION_TOKEN_VERSION: &str = "adopt-5";
const ADOPTION_FIELDS: [&str; 6] = [
    "file entries",
    "skill locations and upstreams",
    "skipped paths",
    "personal rules",
    "personal skill master",
    "Git worktree destinations",
];

fn adoption_digests(adoption: &Adoption) -> Result<[String; ADOPTION_FIELDS.len()]> {
    #[derive(Serialize)]
    enum ApprovedEntry<'a> {
        Entry(&'a AdoptionEntry),
        Link(&'a AdoptionLink),
    }

    let files = adoption
        .files
        .iter()
        .map(|(path, file)| {
            let before = adoption
                .links
                .get(path)
                .map_or(ApprovedEntry::Entry(&file.before), ApprovedEntry::Link);
            (path, (before, &file.after))
        })
        .collect::<BTreeMap<_, _>>();
    // Bind approval to the file operations and the master they create.
    // Link spelling is saved from the live entry at apply time for restore.
    // A worktree's friendly Git name is display-only. Its actual location
    // matters because adoption authorizes future writes there.
    Ok([
        fingerprint(&files)?,
        fingerprint(&(&adoption.skill_locations, &adoption.skill_byte_copies))?,
        fingerprint(&(
            &adoption.skipped,
            &adoption.skipped_rules,
            &adoption.skipped_tools,
            &adoption.added_tools,
        ))?,
        fingerprint(&(
            &adoption.shared,
            &adoption.overlays,
            adoption.codex_sync_replaced,
        ))?,
        fingerprint(&adoption.skills)?,
        fingerprint(&adoption.worktree_writes)?,
    ])
}

pub fn adoption_token(adoption: &Adoption) -> Result<String> {
    Ok(format!(
        "{ADOPTION_TOKEN_VERSION}:{}",
        adoption_digests(adoption)?.join(":")
    ))
}

pub fn verify_adoption_token(adoption: &Adoption, token: &str) -> Result<()> {
    let mut parts = token.split(':');
    ensure!(
        parts.next() == Some(ADOPTION_TOKEN_VERSION)
            && parts.clone().count() == ADOPTION_FIELDS.len(),
        "Unsupported adoption approval token. Review the plan with the current Wenlan CLI."
    );
    for ((field, actual), expected) in ADOPTION_FIELDS
        .into_iter()
        .zip(adoption_digests(adoption)?)
        .zip(parts)
    {
        ensure!(
            actual == expected,
            "The adoption plan changed in {field}. No tool files were written. Review it again."
        );
    }
    Ok(())
}

/// No staging, renderer, daemon, database, or data-root lookup on this path.
pub fn plan_adoption(home: &Path) -> Result<AdoptionPlan> {
    adoption_plan(&discover_adoption(home)?)
}

/// The standalone read-only plan also works before the opt-in writer is enabled.
pub fn plan_adoption_offline(home: &Path) -> Result<AdoptionPlan> {
    if enabled() {
        let mut plan = plan_adoption(home)?;
        plan.token.clear();
        plan.notes.insert(0, "Offline inventory from the installed files. Run `wenlan rules adopt` with the local daemon to see exact skill deliveries and added tools.".into());
        Ok(plan)
    } else {
        let (installed, skipped) =
            installed_targets(home, [&CLAUDE_TARGET, &CODEX_TARGET].into_iter())?;
        let mut adoption = discover_targets(home, &installed, None)?;
        adoption.skipped_tools = skipped;
        legacy_skill_previews(&mut adoption);
        adoption_plan(&adoption)
    }
}

/// The home-relative path a person reads in the plan, with `/` between parts on
/// every platform so the same plan reads the same on Windows.
fn plan_path(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub fn adoption_plan(adoption: &Adoption) -> Result<AdoptionPlan> {
    let mut files = Vec::new();
    for (path, file) in &adoption.files {
        let path = plan_path(path);
        let entry_text = |entry: &AdoptionEntry| match entry {
            AdoptionEntry::File(bytes) => match std::str::from_utf8(bytes) {
                Ok(text) if !text.contains('\0') => text.to_owned(),
                _ => format!(
                    "{} binary bytes, SHA-256 {:x}\n",
                    bytes.len(),
                    Sha256::digest(bytes)
                ),
            },
            AdoptionEntry::Symlink { target, bytes } => {
                let body = bytes.as_deref().map(String::from_utf8_lossy);
                format!(
                    "symlink -> {}\n{}",
                    target.display(),
                    body.as_deref().unwrap_or("")
                )
            }
            AdoptionEntry::Directory => String::from("directory\n"),
            AdoptionEntry::Missing => String::new(),
        };
        let before = entry_text(&file.before);
        let after = entry_text(&file.after);
        line_changes(&before, &after).with_context(|| format!("cannot fully preview {path}"))?;
        let (added, removed, diff) = describe_change(&path, &before, &after);
        files.push(AdoptionChange {
            path,
            added,
            removed,
            diff,
        });
    }
    for (path, reason) in adoption.skipped.iter().chain(&adoption.skipped_rules) {
        files.push(AdoptionChange {
            path: plan_path(path),
            added: 0,
            removed: 0,
            diff: format!("Skipped ({reason}). Stays where it is, not managed.\n"),
        });
    }
    let mut skills = adoption
        .skill_locations
        .iter()
        .map(|(path, upstream)| AdoptionSkill {
            path: plan_path(path),
            outcome: upstream
                .clone()
                .map_or(AdoptionSkillOutcome::PlainCopy, |upstream| {
                    AdoptionSkillOutcome::TrackedUpstream { upstream }
                }),
            byte_copy_reason: adoption.skill_byte_copies.get(path).cloned(),
        })
        .chain(adoption.skipped.iter().map(|(path, reason)| AdoptionSkill {
            path: plan_path(path),
            outcome: AdoptionSkillOutcome::Skipped {
                reason: reason.clone(),
            },
            byte_copy_reason: None,
        }))
        .collect::<Vec<_>>();
    skills.sort_by(|first, second| first.path.cmp(&second.path));
    let mut notes = Vec::new();
    if !adoption.added_tools.is_empty() {
        notes.push(format!(
            "Add to the existing setup: {}.",
            adoption.added_tools.join(", ")
        ));
    }
    if adoption.codex_sync_replaced {
        notes.push("~/.agents/scripts/sync-codex-agents.sh becomes unused for Codex AGENTS.md after adoption. The script stays in place for you to keep or remove.".into());
    }
    notes.push("~/.agents is left untouched. Wenlan replaces adopted links at their tool paths and leaves the shared sources in place.".into());
    notes.extend(adoption.skipped_tools.iter().cloned());
    notes.extend(adoption.worktree_notes.iter().cloned());
    Ok(AdoptionPlan {
        token: adoption_token(adoption)?,
        files,
        skills,
        notes,
    })
}

/// Wenlan keeps imported text as authored. Only Wenlan-generated additions
/// have already passed through the UX-SKILL-8 escaping in skill_grow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalMaster<Content = Vec<u8>> {
    pub shared: String,
    pub overlays: BTreeMap<String, String>,
    pub codex_sync_replaced: bool,
    pub skills: Vec<AdoptedSkill<Content>>,
    pub overrides: BTreeMap<String, LocalText>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalText {
    pub base: String,
    pub edited: String,
}

impl PersonalMaster {
    pub fn enrolls_target(&self, target: &str) -> bool {
        self.overlays.contains_key(target)
            || self
                .skills
                .iter()
                .any(|skill| skill.targets.contains(target))
    }

    pub fn new(adoption: &Adoption) -> Self {
        Self {
            shared: adoption.shared.clone(),
            overlays: adoption.overlays.clone(),
            codex_sync_replaced: adoption.codex_sync_replaced,
            skills: adoption.skills.clone(),
            overrides: BTreeMap::new(),
        }
    }

    pub fn base_rule_text(&self, target: &str, generated: &[String]) -> String {
        let overlay = self.overlays.get(target).map(String::as_str).unwrap_or("");
        let mut base = rulesync_adopt::personal_rule_body(
            &self.shared,
            overlay,
            target,
            self.codex_sync_replaced,
        );
        if !generated.is_empty() {
            let mut addition = String::from("\n## How I work\n\n");
            for line in generated {
                addition.push_str("- ");
                addition.push_str(line);
                addition.push('\n');
            }
            base = join_text(&base, &addition);
        }
        base
    }

    pub fn rule_text(&self, target: &str, generated: &[String]) -> Result<String> {
        let base = self.base_rule_text(target, generated);
        match self.overrides.get(target) {
            Some(local) => merge_rule_text(&local.base, &local.edited, &base),
            None => Ok(base),
        }
    }

    /// Promote a shared edit in place. For a tool overlay, move just the
    /// edited lines into the shared master and leave its other rules local.
    pub fn promote_tool_edit(&mut self, target: &str, before: &str, edited: &str) -> Result<bool> {
        let original = self.overlays.get(target).map(String::as_str).unwrap_or("");
        let (shared, overlay) = promote_changes(
            target,
            self.codex_sync_replaced,
            &self.shared,
            original,
            before,
            edited,
        )?;
        let moved_from_overlay = overlay != original;
        self.shared = shared;
        if moved_from_overlay {
            self.overlays.insert(target.to_owned(), overlay);
        }
        Ok(moved_from_overlay)
    }
}

pub fn join_text(first: &str, second: &str) -> String {
    if first.is_empty() || second.is_empty() || first.ends_with('\n') {
        format!("{first}{second}")
    } else {
        format!("{first}\n{second}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineChange {
    pub start: usize,
    pub removed: Vec<String>,
    pub added: Vec<String>,
}

fn bounded_lines(before: &[&str], after: &[&str]) -> bool {
    // Prevent a local tool's accidental huge file from allocating an unbounded
    // LCS table. Large files stay untouched and can be reviewed separately.
    before.len().saturating_mul(after.len()) <= 4_000_000
}

pub fn line_changes(before: &str, after: &str) -> Result<Vec<LineChange>> {
    let before: Vec<_> = before.split_inclusive('\n').collect();
    let after: Vec<_> = after.split_inclusive('\n').collect();
    let prefix = before
        .iter()
        .zip(&after)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = before[prefix..]
        .iter()
        .rev()
        .zip(after[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old = &before[prefix..before.len() - suffix];
    let new = &after[prefix..after.len() - suffix];
    ensure!(
        bounded_lines(old, new),
        "too many changed lines to compare safely"
    );
    let pairs = crate::authorship::lcs_pairs(old, new);
    let mut changes = Vec::new();
    let (mut previous_old, mut previous_new) = (0, 0);
    for (i, j) in pairs
        .into_iter()
        .chain(std::iter::once((old.len(), new.len())))
    {
        if (i, j) != (previous_old, previous_new) {
            changes.push(LineChange {
                start: prefix + previous_old,
                removed: old[previous_old..i]
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                added: new[previous_new..j]
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            });
        }
        previous_old = i + 1;
        previous_new = j + 1;
    }
    Ok(changes)
}

pub fn describe_change(path: &str, before: &str, after: &str) -> (usize, usize, String) {
    let Ok(changes) = line_changes(before, after) else {
        return (0, 0, "Too many changed lines for a safe preview.\n".into());
    };
    if changes.is_empty() {
        return (0, 0, String::new());
    }
    let mut diff = format!("--- {path}\n+++ {path}\n");
    let (mut added, mut removed) = (0, 0);
    for change in changes {
        let old_line = change.start + usize::from(!change.removed.is_empty());
        let new_line = change.start + added - removed + usize::from(!change.added.is_empty());
        diff.push_str(&format!(
            "@@ -{old_line},{} +{new_line},{} @@\n",
            change.removed.len(),
            change.added.len()
        ));
        removed += change.removed.len();
        added += change.added.len();
        for (prefix, lines) in [('-', change.removed), ('+', change.added)] {
            for line in lines {
                diff.push(prefix);
                diff.push_str(&line);
                if !line.ends_with('\n') {
                    diff.push_str("\n\\ No newline at end of file\n");
                }
            }
        }
    }
    (added, removed, diff)
}

/// Apply both sets of changes to a common ancestor. Overlapping changes
/// require consent when they disagree rather than guessing how to combine them.
pub fn merge_rule_text(base: &str, left: &str, right: &str) -> Result<String> {
    if left == base || left == right {
        return Ok(right.into());
    }
    if right == base {
        return Ok(left.into());
    }
    let mut changes = line_changes(base, left)?;
    for change in line_changes(base, right)? {
        if changes.contains(&change) {
            continue;
        }
        let end = change.start + change.removed.len();
        if changes.iter().any(|earlier| {
            let earlier_end = earlier.start + earlier.removed.len();
            earlier.start < end && change.start < earlier_end || earlier.start == change.start
        }) {
            bail!("the master and this tool changed the same lines");
        }
        changes.push(change);
    }
    changes.sort_by_key(|change| change.start);
    let lines: Vec<_> = base.split_inclusive('\n').collect();
    let (mut result, mut at) = (String::new(), 0);
    for change in changes {
        result.extend(lines[at..change.start].iter().copied());
        result.extend(change.added);
        at = change.start + change.removed.len();
    }
    result.extend(lines[at..].iter().copied());
    Ok(result)
}

fn promote_changes(
    target: &str,
    codex_sync_replaced: bool,
    shared: &str,
    overlay: &str,
    before: &str,
    edited: &str,
) -> Result<(String, String)> {
    let base = rulesync_adopt::personal_rule_body(shared, overlay, target, codex_sync_replaced);
    let header_line_count = rulesync_adopt::personal_rule_body("", "", target, codex_sync_replaced)
        .split_inclusive('\n')
        .count();
    let shared_end = rulesync_adopt::personal_rule_body(shared, "", target, codex_sync_replaced)
        .split_inclusive('\n')
        .count();
    let base_lines: Vec<_> = base.split_inclusive('\n').collect();
    let before_lines: Vec<_> = before.split_inclusive('\n').collect();
    let mut before_to_base = vec![None; before_lines.len()];
    let mut base_at = 0;
    let mut before_at = 0;
    for change in line_changes(&base, before)? {
        let unchanged = change.start - base_at;
        for offset in 0..unchanged {
            before_to_base[before_at + offset] = Some(base_at + offset);
        }
        base_at = change.start + change.removed.len();
        before_at += unchanged + change.added.len();
    }
    for offset in 0..base_lines.len() - base_at {
        before_to_base[before_at + offset] = Some(base_at + offset);
    }
    ensure!(before_at + base_lines.len() - base_at == before_lines.len());
    let mut shared: Vec<String> = shared.split_inclusive('\n').map(str::to_owned).collect();
    let mut overlay: Vec<String> = overlay.split_inclusive('\n').map(str::to_owned).collect();
    let mut changes_to_shared = Vec::new();
    for change in line_changes(before, edited)?.into_iter().rev() {
        if !change.removed.is_empty() {
            let mapped: Vec<_> = (change.start..change.start + change.removed.len())
                .map(|before_at| before_to_base.get(before_at).copied().flatten())
                .collect();
            let base_at = mapped
                .first()
                .copied()
                .flatten()
                .context("this change includes text outside the authored rules")?;
            ensure!(
                mapped
                    .iter()
                    .enumerate()
                    .all(|(offset, at)| *at == Some(base_at + offset)),
                "this change does not map to one authored rule section"
            );
            let end = base_at + change.removed.len();
            ensure!(
                base_at >= header_line_count,
                "this change targets the managed tool header"
            );
            if base_at < shared_end {
                ensure!(
                    end <= shared_end,
                    "this change crosses the shared and tool overlay boundary"
                );
                let at = base_at - header_line_count;
                ensure!(at + change.removed.len() <= shared.len());
                shared.splice(at..at + change.removed.len(), change.added);
                continue;
            }
            let at = base_at - shared_end;
            ensure!(at + change.removed.len() <= overlay.len());
            overlay.drain(at..at + change.removed.len());
        }
        if !change.added.is_empty() {
            changes_to_shared.push(change.added);
        }
    }
    for addition in changes_to_shared.into_iter().rev() {
        if let Some(last) = shared.last_mut() {
            if !last.ends_with('\n') {
                last.push('\n');
            }
        }
        shared.extend(addition);
    }
    Ok((shared.concat(), overlay.concat()))
}

pub fn write_root_rule(input: &Path, text: &str) -> Result<()> {
    write_named_root_rule(input, "wenlan", text)
}

pub fn write_named_root_rule(input: &Path, name: &str, text: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid staged rule name"
    );
    let rules = input.join("rules");
    fs::create_dir_all(&rules)?;
    fs::write(
        rules.join(format!("{name}.md")),
        format!("---\nroot: true\n---\n{text}"),
    )?;
    Ok(())
}

/// Shared setup must never carry credential-like text. Error messages
/// identify only the position so the suspected value is never echoed.
pub fn reject_secret_lines(text: &str) -> Result<()> {
    static TOKEN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(concat!(
            r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----",
            r"|\b(?:sk-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{20,}",
            r"|github_pat_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{16,}",
            r"|(?:AKIA|ASIA)[A-Z0-9]{16}|AIza[A-Za-z0-9_-]{35}|wlt_[a-f0-9]{64})\b",
            r"|(?i)\bbearer\s+[A-Za-z0-9_.~+/-]{20,}",
            r#"|(?i)\b(?:[a-z0-9_]*[_-])?(?:api[_-]?key|secret|token|password|access[_-]?key)\b["']?"#,
            r#"\s*[:=]\s*["']?[A-Za-z0-9_+/.-]{8,}"#,
        )).expect("credential detection pattern")
    });
    for (index, line) in text.lines().enumerate() {
        ensure!(
            !TOKEN.is_match(line),
            "possible credential on line {}",
            index + 1
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rulesync_adoption_imports_installed_overlays_and_skips_missing_tools() {
        let home = tempfile::tempdir().unwrap();
        for (path, text) in [
            (".agents/AGENTS.md", "Shared rule.\n"),
            (".gemini/GEMINI.md", "Shared rule.\nGemini only.\n"),
            (
                ".gemini/config/GEMINI.md",
                "Shared rule.\nAntigravity only.\n",
            ),
            (
                ".copilot/copilot-instructions.md",
                "Shared rule.\nCopilot only.\n",
            ),
        ] {
            let path = home.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let adoption = discover_adoption(home.path()).unwrap();
        assert_eq!(
            adoption.overlays,
            BTreeMap::from([
                ("geminicli".into(), "Gemini only.\n".into()),
                ("antigravity-ide".into(), "Antigravity only.\n".into()),
                ("copilotcli".into(), "Copilot only.\n".into()),
            ])
        );
        let plan = adoption_plan(&adoption).unwrap();
        assert_eq!(plan.files.len(), 3);
        for tool in ["Claude Code", "Codex", "Cursor", "OpenCode", "Muse Code"] {
            assert!(plan
                .notes
                .iter()
                .any(|note| note.starts_with(&format!("Skipped {tool} "))));
        }
        assert!(!home.path().join(".cursor").exists());
        assert!(!home.path().join(".config").exists());
    }

    #[test]
    fn rulesync_personal_destinations_are_portable_and_have_one_owner() {
        let expected = [
            ("claudecode", ".claude/CLAUDE.md", ".claude/skills"),
            ("codexcli", ".codex/AGENTS.md", ".codex/skills"),
            ("geminicli", ".gemini/GEMINI.md", ".gemini/skills"),
            (
                "antigravity-ide",
                ".gemini/config/GEMINI.md",
                ".gemini/config/skills",
            ),
            (
                "copilotcli",
                ".copilot/copilot-instructions.md",
                ".copilot/skills",
            ),
            ("cursor", ".cursor/AGENTS.md", ".cursor/skills"),
            (
                "opencode",
                ".config/opencode/AGENTS.md",
                ".config/opencode/skills",
            ),
            ("musecode", ".config/muse/AGENTS.md", ".config/muse/skills"),
        ];
        assert_eq!(PERSONAL_TARGETS.len(), expected.len());
        let mut destinations = std::collections::BTreeSet::new();
        for (id, rules, skills) in expected {
            let target = personal_target(id).unwrap();
            for (actual, expected) in [
                (target.rules.destination, rules),
                (target.skills.destination, skills),
            ] {
                let path = Path::new(actual);
                assert!(path.starts_with(target.folder()));
                assert!(!is_harness_path(path));
                assert!(destinations.insert(plan_path(path).to_ascii_lowercase()));
                assert_eq!(plan_path(path), expected);
                for home in ["/Users/Test User", "/home/test-user", "C:/Users/Test User"] {
                    let home = Path::new(home);
                    assert_eq!(home.join(path), home.join(expected));
                    let resolved = home.join(path);
                    let relative = resolved.strip_prefix(home).unwrap();
                    assert_eq!(plan_path(relative), expected);
                }
            }
        }
        let gemini = personal_target("geminicli").unwrap();
        let windows_relative = plan_path(Path::new(gemini.rules.destination)).replace('/', "\\");
        assert_eq!(
            format!("%USERPROFILE%\\{windows_relative}"),
            r"%USERPROFILE%\.gemini\GEMINI.md"
        );
    }

    #[test]
    fn line_changes_include_deletions_and_final_newline_changes() {
        let (added, removed, rendered) = describe_change("AGENTS.md", "a\nb\nc", "a\nc\n");
        assert_eq!((added, removed), (1, 2));
        assert!(rendered.contains("-b\n-c\n\\ No newline at end of file\n+c\n"));
    }

    #[test]
    fn tool_only_deletion_survives_an_unrelated_master_edit() {
        let base = "one\ntwo\nthree\n";
        let local = "one\nthree\n";
        assert_eq!(
            merge_rule_text(base, local, "one\ntwo\nnew\n").unwrap(),
            "one\nnew\n"
        );
        assert!(merge_rule_text(base, local, "one\nsecond\nthree\n").is_err());
    }

    #[test]
    fn promotion_uses_edit_position_to_disambiguate_repeated_rules() {
        assert_eq!(
            promote_changes("cursor", false, "x\nx\n", "", "x\nx\n", "y\nx\n").unwrap(),
            ("y\nx\n".into(), String::new())
        );
        assert_eq!(
            promote_changes("cursor", false, "x\ny\n", "", "x\ny\n", "z\ny\n").unwrap(),
            ("z\ny\n".into(), String::new())
        );
        assert_eq!(
            promote_changes("cursor", false, "x\n", "y\ny\n", "x\ny\ny\n", "x\nz\ny\n").unwrap(),
            ("x\nz\n".into(), "y\n".into())
        );
    }

    #[test]
    fn all_tools_promotion_targets_the_edited_overlay_occurrence() {
        let mut master = PersonalMaster {
            shared: "Repeated rule.\n".into(),
            overlays: BTreeMap::from([("cursor".into(), "Repeated rule.\n".into())]),
            codex_sync_replaced: false,
            skills: Vec::new(),
            overrides: BTreeMap::new(),
        };
        let before = master.base_rule_text("cursor", &[]);

        assert!(master
            .promote_tool_edit("cursor", &before, "Repeated rule.\nEdited tool rule.\n")
            .unwrap());
        assert_eq!(master.shared, "Repeated rule.\nEdited tool rule.\n");
        assert_eq!(master.overlays.get("cursor").unwrap(), "");
    }

    #[test]
    fn promotion_maps_codex_overlay_edits_past_the_managed_header() {
        let mut master = PersonalMaster {
            shared: "Shared rule.\n".into(),
            overlays: BTreeMap::from([("codexcli".into(), "Codex rule.\n".into())]),
            codex_sync_replaced: true,
            skills: Vec::new(),
            overrides: BTreeMap::new(),
        };
        let before = master.base_rule_text("codexcli", &[]);
        let edited = before.replace("Codex rule.", "Edited Codex rule.");

        assert!(master
            .promote_tool_edit("codexcli", &before, &edited)
            .unwrap());
        assert_eq!(master.shared, "Shared rule.\nEdited Codex rule.\n");
        assert_eq!(master.overlays.get("codexcli").unwrap(), "");
    }

    #[test]
    fn promotion_maps_edits_after_an_accepted_local_insertion() {
        let mut master = PersonalMaster {
            shared: "Shared rule.\n".into(),
            overlays: BTreeMap::from([("cursor".into(), "Cursor rule.\n".into())]),
            codex_sync_replaced: false,
            skills: Vec::new(),
            overrides: BTreeMap::new(),
        };
        let base = master.base_rule_text("cursor", &[]);
        let local = "Shared rule.\nAccepted local insertion.\nCursor rule.\n";
        master.overrides.insert(
            "cursor".into(),
            LocalText {
                base,
                edited: local.into(),
            },
        );
        let before = master.rule_text("cursor", &[]).unwrap();
        let edited = before.replace("Cursor rule.", "Edited Cursor rule.");

        assert!(master
            .promote_tool_edit("cursor", &before, &edited)
            .unwrap());
        assert_eq!(master.shared, "Shared rule.\nEdited Cursor rule.\n");
        assert_eq!(master.overlays.get("cursor").unwrap(), "");
    }

    #[test]
    fn promotion_keeps_multiple_appended_overlay_edits_in_file_order() {
        let mut master = PersonalMaster {
            shared: "Shared rule.\n".into(),
            overlays: BTreeMap::from([(
                "cursor".into(),
                "First rule.\nMiddle rule.\nLast rule.\n".into(),
            )]),
            codex_sync_replaced: false,
            skills: Vec::new(),
            overrides: BTreeMap::new(),
        };
        let before = master.base_rule_text("cursor", &[]);
        let edited = before
            .replace("First rule.", "Edited first rule.")
            .replace("Last rule.", "Edited last rule.");

        assert!(master
            .promote_tool_edit("cursor", &before, &edited)
            .unwrap());
        assert_eq!(
            master.shared,
            "Shared rule.\nEdited first rule.\nEdited last rule.\n"
        );
        assert_eq!(master.overlays.get("cursor").unwrap(), "Middle rule.\n");
    }

    #[test]
    fn promotion_refuses_edits_inside_unmanaged_text_but_keeps_new_additions() {
        assert!(promote_changes(
            "cursor",
            false,
            "Shared rule.\n",
            "",
            "Shared rule.\nGenerated or local text.\n",
            "Shared rule.\nEdited generated or local text.\n",
        )
        .is_err());

        assert_eq!(
            promote_changes(
                "cursor",
                false,
                "Shared rule.\n",
                "",
                "Shared rule.\n## How I work\n- generated rule\n",
                "Shared rule.\n## How I work\n- generated rule\nNew rule\n",
            )
            .unwrap(),
            ("Shared rule.\nNew rule\n".into(), String::new())
        );
        assert_eq!(
            promote_changes(
                "cursor",
                false,
                "",
                "",
                "## How I work\n- generated rule\n",
                "## How I work\n- generated rule\nNew rule\n",
            )
            .unwrap(),
            ("New rule\n".into(), String::new())
        );
    }

    #[test]
    fn shared_masters_refuse_credentials_without_echoing_them() {
        let fixture = ["sk-", &"x".repeat(24)].concat();
        let error = reject_secret_lines(&format!("ordinary rule\nBearer {fixture}")).unwrap_err();
        assert_eq!(error.to_string(), "possible credential on line 2");
        assert!(reject_secret_lines("Read API keys from the system keychain.").is_ok());
        assert!(reject_secret_lines("SERVICE_TOKEN=fixture-value").is_err());
    }
}
