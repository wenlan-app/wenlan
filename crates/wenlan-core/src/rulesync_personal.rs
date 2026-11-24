// SPDX-License-Identifier: Apache-2.0
//! The personal master and the read-only side of adopting an existing setup.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use cap_fs_ext::{DirExt as _, MetadataExt as _};
use cap_std::fs::{Dir, Metadata};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wenlan_types::rulesync::{
    AdoptionChange, AdoptionNewFiles, AdoptionPlan, AdoptionSkill, AdoptionSkillOutcome,
    AdoptionTextDiff, AdoptionTool, KeptInPlace, RulesSetupCode as Code, RulesSetupNotice,
    RulesSetupTool,
};
use wenlan_types::rulesync_skill::SavedSkillTool;

use crate::rulesync_adopt::{
    self, AdoptedSkill, Adoption, AdoptionEntry, AdoptionLink, AdoptionSkip, AdoptionTarget,
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
const WINDOWS_INHERITED_ACE: u8 = 0x10;

#[cfg(any(windows, test))]
fn windows_replacement_problem(
    owner: &[u8],
    assignable_owners: &[Vec<u8>],
    has_security_policy: bool,
) -> Option<&'static str> {
    // An elevated administrator's files are owned by its default owner
    // (BUILTIN\Administrators), not its user SID. Accept any owner the
    // process may assign itself, so the owner it keeps is the one it can set.
    if !assignable_owners.iter().any(|sid| sid == owner) {
        return Some("the Windows owner is not the process user or one of its owner groups");
    }
    windows_policy_problem(has_security_policy)
}

#[cfg(any(windows, test))]
fn windows_policy_problem(has_security_policy: bool) -> Option<&'static str> {
    has_security_policy.then_some("the Windows entry has an integrity label or security policy")
}

#[cfg(any(windows, test))]
fn windows_sacl_problem(protected: bool, ace_flags: &[u8]) -> Option<&'static str> {
    if protected {
        return Some("the Windows system access list is protected");
    }
    ace_flags
        .iter()
        .any(|flags| flags & WINDOWS_INHERITED_ACE == 0)
        .then_some("the Windows system access list contains an explicit entry")
}

#[cfg(any(windows, test))]
struct WindowsDacl<'a> {
    protected: bool,
    revision: Option<u8>,
    aces: Vec<&'a [u8]>,
}

#[cfg(any(windows, test))]
impl WindowsDacl<'_> {
    fn matches(&self, other: &Self) -> bool {
        if self.protected != other.protected || self.revision.is_some() != other.revision.is_some()
        {
            return false;
        }
        if self.protected {
            return self.revision == other.revision && self.aces == other.aces;
        }

        // An unprotected set can re-merge ACEs inherited from the staging
        // folder. Verify the ordered bytes of the explicit ACEs and the
        // protection bit. A null DACL is distinct from an empty DACL.
        let explicit = |ace: &&[u8]| ace[1] & WINDOWS_INHERITED_ACE == 0;
        self.aces
            .iter()
            .copied()
            .filter(explicit)
            .eq(other.aces.iter().copied().filter(explicit))
    }
}

#[cfg(test)]
mod rulesync_windows_descriptor_tests {
    use super::{windows_replacement_problem, windows_sacl_problem, WindowsDacl};

    #[test]
    fn replacement_requires_an_assignable_owner_and_no_security_policy() {
        let owners = [b"user".to_vec(), b"admins".to_vec()];
        assert_eq!(windows_replacement_problem(b"user", &owners, false), None);
        // An elevated process's files are owned by its owner group.
        assert_eq!(windows_replacement_problem(b"admins", &owners, false), None);
        assert_eq!(
            windows_replacement_problem(b"other", &owners, false),
            Some("the Windows owner is not the process user or one of its owner groups"),
        );
        assert_eq!(
            windows_replacement_problem(b"group", &[b"user".to_vec()], false),
            Some("the Windows owner is not the process user or one of its owner groups"),
        );
        assert_eq!(
            windows_replacement_problem(b"user", &owners, true),
            Some("the Windows entry has an integrity label or security policy"),
        );
    }

    #[test]
    fn explicit_auditing_and_protected_sacls_are_refused() {
        assert_eq!(windows_sacl_problem(false, &[]), None);
        assert_eq!(windows_sacl_problem(false, &[0x50, 0x90]), None);
        assert_eq!(
            windows_sacl_problem(true, &[]),
            Some("the Windows system access list is protected"),
        );
        assert_eq!(
            windows_sacl_problem(false, &[0x50, 0x80]),
            Some("the Windows system access list contains an explicit entry"),
        );
    }

    #[test]
    fn readback_preserves_ace_bytes_order_and_protection() {
        let allow = &[0, 0, 8, 0, 1, 0, 0, 0][..];
        let deny = &[1, 0, 8, 0, 1, 0, 0, 0][..];
        let different_deny = &[1, 0, 8, 0, 2, 0, 0, 0][..];
        let inherited = &[0, 0x10, 8, 0, 2, 0, 0, 0][..];
        let remerged = &[0, 0x10, 8, 0, 4, 0, 0, 0][..];
        let read = |protected, aces| WindowsDacl {
            protected,
            revision: Some(2),
            aces,
        };
        let original = read(false, vec![deny, allow, inherited]);
        assert!(original.matches(&read(false, vec![deny, allow, remerged])));
        assert!(!original.matches(&read(false, vec![allow, deny, inherited])));
        assert!(!original.matches(&read(false, vec![deny, inherited])));
        assert!(!original.matches(&read(false, vec![different_deny, allow, inherited])));
        assert!(!original.matches(&read(true, vec![deny, allow, inherited])));
        let protected = read(true, vec![deny, allow, inherited]);
        assert!(protected.matches(&read(true, vec![deny, allow, inherited])));
        assert!(!protected.matches(&read(true, vec![deny, allow, remerged])));
    }

    #[test]
    fn readback_distinguishes_a_null_dacl_from_an_empty_dacl() {
        let null = WindowsDacl {
            protected: false,
            revision: None,
            aces: Vec::new(),
        };
        let empty = WindowsDacl {
            revision: Some(2),
            aces: Vec::new(),
            ..null
        };
        assert!(!null.matches(&empty));
    }
}

#[cfg(windows)]
pub use windows_acl::{
    check_windows_replacement, copy_windows_dacl, open_windows_tool_dir,
    prepare_windows_file_replacement, read_windows_owner, windows_access_error,
    windows_owner_is_assignable, windows_permissions_error,
};

#[cfg(windows)]
mod windows_acl {
    use super::{
        windows_policy_problem, windows_replacement_problem, windows_sacl_problem, WindowsDacl,
    };
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
    const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;
    const UNPROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x2000_0000;
    const SE_DACL_PROTECTED: u16 = 0x1000;
    const SE_SACL_PROTECTED: u16 = 0x2000;
    const READ_CONTROL: u32 = 0x0002_0000;
    const WRITE_DAC: u32 = 0x0004_0000;
    const WRITE_OWNER: u32 = 0x0008_0000;
    const ACCESS_SYSTEM_SECURITY: u32 = 0x0100_0000;
    const SHARE_READ_WRITE_DELETE: u32 = 0x7;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const TOKEN_QUERY: u32 = 0x0008;
    const TOKEN_USER: u32 = 1;
    const TOKEN_GROUPS: u32 = 2;
    const TOKEN_OWNER: u32 = 4;
    const SE_GROUP_ENABLED: u32 = 0x4;
    const SE_GROUP_OWNER: u32 = 0x8;
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_INSUFFICIENT_BUFFER: i32 = 122;
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

    // TOKEN_USER contains a single SID_AND_ATTRIBUTES.
    #[repr(C)]
    struct SidAndAttributes {
        sid: *mut c_void,
        attributes: u32,
    }

    // TOKEN_GROUPS is a count followed by that many SID_AND_ATTRIBUTES.
    #[repr(C)]
    struct TokenGroups {
        count: u32,
        groups: [SidAndAttributes; 1],
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
        fn SetSecurityInfo(
            handle: RawHandle,
            object_type: u32,
            security_info: u32,
            owner: *mut c_void,
            group: *mut c_void,
            dacl: *mut AclHeader,
            sacl: *mut AclHeader,
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
        fn GetAce(acl: *mut AclHeader, index: u32, ace: *mut *mut c_void) -> i32;
        fn GetLengthSid(sid: *mut c_void) -> u32;
        fn OpenProcessToken(process: RawHandle, access: u32, token: *mut RawHandle) -> i32;
        fn GetTokenInformation(
            token: RawHandle,
            class: u32,
            buffer: *mut c_void,
            length: u32,
            needed: *mut u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> RawHandle;
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
            let owner = if flags & OWNER_SECURITY_INFORMATION != 0 {
                &mut sd.owner
            } else {
                null_mut()
            };
            let dacl = if flags & DACL_SECURITY_INFORMATION != 0 {
                &mut sd.dacl
            } else {
                null_mut()
            };
            // SAFETY: the handle stays open and the output pointers are writable.
            // The owner and ACL are stored inside the owned descriptor.
            let status = unsafe {
                GetSecurityInfo(
                    entry.as_raw_handle(),
                    SE_FILE_OBJECT,
                    flags,
                    owner,
                    null_mut(),
                    dacl,
                    null_mut(),
                    &mut sd.ptr,
                )
            };
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            if sd.ptr.is_null() || (flags & OWNER_SECURITY_INFORMATION != 0 && sd.owner.is_null()) {
                return Err(io::Error::other(
                    "the Windows security descriptor is unavailable",
                ));
            }
            Ok(sd)
        }

        fn owner(&self) -> &[u8] {
            // SAFETY: the caller requested OWNER_SECURITY_INFORMATION and
            // GetSecurityInfo supplied a SID in the live descriptor.
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

        fn dacl(&self) -> io::Result<WindowsDacl<'_>> {
            let protected = self.control()? & SE_DACL_PROTECTED != 0;
            let mut dacl = WindowsDacl {
                protected,
                revision: None,
                aces: Vec::new(),
            };
            // SAFETY: any returned ACL is owned by this live descriptor.
            if let Some(list) = unsafe { self.dacl.as_ref() } {
                dacl.revision = Some(list.revision);
                for index in 0..u32::from(list.ace_count) {
                    let ace = ace_at(self.dacl, index)?;
                    // SAFETY: GetAce points to an ACE whose header gives the
                    // length within the live descriptor.
                    let bytes = unsafe {
                        slice::from_raw_parts(ace.cast::<u8>(), usize::from((*ace).size))
                    };
                    dacl.aces.push(bytes);
                }
            }
            Ok(dacl)
        }

        fn set_staged_owner(&self, staged: &impl AsRawHandle) -> io::Result<()> {
            if SecurityDescriptor::read(staged, OWNER_SECURITY_INFORMATION)?.owner() == self.owner()
            {
                return Ok(());
            }
            // The process token can have a default owner different from its
            // user SID. ReplaceFileW documents DACL preservation, so ensure
            // the replacement starts with the owner we intend to retain.
            let writable = reopen(staged, READ_CONTROL | WRITE_OWNER)?;
            // SAFETY: writable is live and self owns the owner SID.
            let status = unsafe {
                SetSecurityInfo(
                    writable.as_raw_handle(),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION,
                    self.owner,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                )
            };
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            Ok(())
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

    fn ace_at(list: *mut AclHeader, index: u32) -> io::Result<*const AceHeader> {
        let mut ace = null_mut();
        // SAFETY: the caller holds the descriptor with this ACL and bounds
        // index by its ACE count. The output pointer is writable.
        if unsafe { GetAce(list, index, &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let ace = ace.cast::<AceHeader>();
        // SAFETY: GetAce returned an entry starting with an ACE_HEADER.
        let header = unsafe { ace.as_ref() }
            .ok_or_else(|| io::Error::other("a Windows access rule is unavailable"))?;
        if usize::from(header.size) < std::mem::size_of::<AceHeader>() {
            return Err(io::Error::other("invalid Windows access rule length"));
        }
        Ok(ace)
    }

    pub fn windows_permissions_error(error: io::Error) -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!("left unmanaged: could not preserve Windows permissions ({error})"),
        )
    }

    pub fn windows_access_error(error: io::Error) -> io::Error {
        if error.kind() == io::ErrorKind::PermissionDenied {
            windows_permissions_error(error)
        } else {
            error
        }
    }

    pub fn open_windows_tool_dir(parent: &Dir, name: &Path) -> io::Result<Dir> {
        parent.open_dir_nofollow(name).map_err(windows_access_error)
    }

    /// Whether the process may keep `owner` as the owner of a file it writes.
    pub fn windows_owner_is_assignable(owner: &[u8]) -> io::Result<bool> {
        assignable_owners()
            .map(|owners| owners.iter().any(|sid| sid == owner))
            .map_err(windows_permissions_error)
    }

    fn token_information(
        token: &OwnedHandle,
        class: u32,
        minimum: usize,
    ) -> io::Result<Vec<usize>> {
        let mut length = 0;
        // SAFETY: a null buffer and zero size query the required byte count.
        if unsafe { GetTokenInformation(token.as_raw_handle(), class, null_mut(), 0, &mut length) }
            == 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER) {
                return Err(error);
            }
        }
        if (length as usize) < minimum {
            return Err(io::Error::other("the Windows process token is unavailable"));
        }
        let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
        // SAFETY: the buffer is pointer-aligned and has at least length bytes.
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                class,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(buffer)
    }

    fn copy_sid(sid: *mut c_void) -> Vec<u8> {
        // SAFETY: the SID stays valid while its token buffer lives and is copied here.
        unsafe { slice::from_raw_parts(sid.cast(), GetLengthSid(sid) as usize) }.to_vec()
    }

    /// The owners the process may assign: its user, its default owner, and
    /// enabled groups marked as owner groups.
    fn assignable_owners() -> io::Result<Vec<Vec<u8>>> {
        let mut token = null_mut();
        // SAFETY: GetCurrentProcess is a valid pseudo-handle and token is writable.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: OpenProcessToken returned a new owned handle.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let user = token_information(&token, TOKEN_USER, std::mem::size_of::<SidAndAttributes>())?;
        // SAFETY: GetTokenInformation wrote a TOKEN_USER whose SID is in user.
        let mut owners = vec![copy_sid(unsafe {
            (*user.as_ptr().cast::<SidAndAttributes>()).sid
        })];
        let owner = token_information(&token, TOKEN_OWNER, std::mem::size_of::<*mut c_void>())?;
        // SAFETY: GetTokenInformation wrote a TOKEN_OWNER whose SID is in owner.
        owners.push(copy_sid(unsafe { *owner.as_ptr().cast::<*mut c_void>() }));
        let groups = token_information(&token, TOKEN_GROUPS, std::mem::size_of::<u32>())?;
        let list = groups.as_ptr().cast::<TokenGroups>();
        // SAFETY: GetTokenInformation wrote a TOKEN_GROUPS with count entries in groups.
        let count = unsafe { (*list).count } as usize;
        // SAFETY: the entries follow the count inside the same buffer.
        let entries = unsafe {
            slice::from_raw_parts(
                std::ptr::addr_of!((*list).groups).cast::<SidAndAttributes>(),
                count,
            )
        };
        let owner_group = SE_GROUP_ENABLED | SE_GROUP_OWNER;
        owners.extend(
            entries
                .iter()
                .filter(|group| group.attributes & owner_group == owner_group)
                .map(|group| copy_sid(group.sid)),
        );
        Ok(owners)
    }

    fn reopen(entry: &impl AsRawHandle, access: u32) -> io::Result<OwnedHandle> {
        // SAFETY: reopen the same live object without a new pathname lookup.
        let handle = unsafe {
            ReOpenFile(
                entry.as_raw_handle(),
                access,
                SHARE_READ_WRITE_DELETE,
                FILE_FLAG_BACKUP_SEMANTICS,
            )
        };
        if handle as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: ReOpenFile returned a new owned handle.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    fn check_windows_audit(entry: &impl AsRawHandle) -> io::Result<()> {
        let handle = match reopen(entry, READ_CONTROL | ACCESS_SYSTEM_SECURITY) {
            Ok(handle) => handle,
            // Reading audit entries requires an enabled SeSecurityPrivilege.
            // The ordinary READ_CONTROL checks still apply when it is unavailable.
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(ERROR_ACCESS_DENIED | ERROR_PRIVILEGE_NOT_HELD)
                ) =>
            {
                return Ok(())
            }
            Err(error) => return Err(error),
        };
        let audit = SecurityDescriptor::read(&handle, SACL_SECURITY_INFORMATION)?;
        let protected = audit.control()? & SE_SACL_PROTECTED != 0;
        let sacl = audit.sacl()?;
        let mut flags = Vec::new();
        // SAFETY: any returned SACL is owned by the live descriptor.
        if let Some(list) = unsafe { sacl.as_ref() } {
            for index in 0..u32::from(list.ace_count) {
                // SAFETY: ace_at returned a header from the live descriptor.
                let ace = unsafe { &*ace_at(sacl, index)? };
                // Security-policy ACE types start with SYSTEM_MANDATORY_LABEL_ACE_TYPE.
                if let Some(problem) = windows_policy_problem(ace.kind >= 0x11) {
                    return Err(io::Error::other(problem));
                }
                flags.push(ace.flags);
            }
        }
        if let Some(problem) = windows_sacl_problem(protected, &flags) {
            return Err(io::Error::other(problem));
        }
        Ok(())
    }

    fn replacement_descriptor(
        entry: &impl AsRawHandle,
        extra: u32,
    ) -> io::Result<SecurityDescriptor> {
        // READ_CONTROL lets us inspect security policy entries without
        // requesting access to the audit portion of the SACL.
        let sd = SecurityDescriptor::read(
            entry,
            OWNER_SECURITY_INFORMATION
                | LABEL_SECURITY_INFORMATION
                | ATTRIBUTE_SECURITY_INFORMATION
                | SCOPE_SECURITY_INFORMATION
                | extra,
        )?;
        let policy = sd.sacl()?;
        // SAFETY: any returned SACL is owned by the live descriptor.
        let has_security_policy =
            unsafe { policy.as_ref() }.is_some_and(|list| list.ace_count != 0);
        if let Some(problem) =
            windows_replacement_problem(sd.owner(), &assignable_owners()?, has_security_policy)
        {
            return Err(io::Error::other(problem));
        }
        if let Some(problem) = windows_sacl_problem(sd.control()? & SE_SACL_PROTECTED != 0, &[]) {
            return Err(io::Error::other(problem));
        }
        check_windows_audit(entry)?;
        Ok(sd)
    }

    pub fn read_windows_owner(entry: &impl AsRawHandle) -> io::Result<Vec<u8>> {
        SecurityDescriptor::read(entry, OWNER_SECURITY_INFORMATION)
            .map(|sd| sd.owner().to_vec())
            .map_err(windows_permissions_error)
    }

    pub fn check_windows_replacement(entry: &impl AsRawHandle) -> io::Result<()> {
        replacement_descriptor(entry, 0)
            .map(|_| ())
            .map_err(windows_permissions_error)
    }

    pub fn prepare_windows_file_replacement(
        old: &impl AsRawHandle,
        staged: &impl AsRawHandle,
    ) -> io::Result<()> {
        replacement_descriptor(old, 0)
            .and_then(|sd| sd.set_staged_owner(staged))
            .map_err(windows_permissions_error)
    }

    pub fn copy_windows_dacl(old: &impl AsRawHandle, staged: &impl AsRawHandle) -> io::Result<()> {
        (|| {
            let original = replacement_descriptor(old, DACL_SECURITY_INFORMATION)?;
            let expected = original.dacl()?;
            let writable = reopen(staged, READ_CONTROL | WRITE_DAC)?;
            original.set_staged_owner(staged)?;
            let protection = if expected.protected {
                PROTECTED_DACL_SECURITY_INFORMATION
            } else {
                UNPROTECTED_DACL_SECURITY_INFORMATION
            };
            // SAFETY: writable is live and the ACL (or null) is owned by original.
            let status = unsafe {
                SetSecurityInfo(
                    writable.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | protection,
                    null_mut(),
                    null_mut(),
                    original.dacl,
                    null_mut(),
                )
            };
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            let copied = SecurityDescriptor::read(
                &writable,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            )?;
            if copied.owner() != original.owner() || !expected.matches(&copied.dacl()?) {
                return Err(io::Error::other(
                    "the copied Windows permissions differ on read-back",
                ));
            }
            Ok(())
        })()
        .map_err(windows_permissions_error)
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
    match std::env::var_os("WENLAN_RULESYNC") {
        Some(value) => value == "1",
        None => crate::config::data_root().join("rulesync.on").is_file(),
    }
}

/// Persist the switch in the data root shared by the CLI and daemon.
pub fn set_enabled(on: bool) -> Result<()> {
    let path = crate::config::data_root().join("rulesync.on");
    if on {
        std::fs::create_dir_all(path.parent().context("missing data root")?)?;
        if !path.is_file() {
            std::fs::write(path, b"on\n")?;
        }
    } else if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error.into());
        }
    }
    Ok(())
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
    let tools = installed.iter().map(|target| target.id);
    let files = rulesync_adopt::discover_personal_files(home, tools, &BTreeSet::new())?;
    let mut adoption = discover_targets(home, &installed, None, Some(files))?;
    adoption.settings = crate::rulesync_settings::personal::Adoption::discover(home)?;
    record_skipped_tools(&mut adoption, skipped);
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

pub fn discover_additional_adoption(
    home: &Path,
    master: &PersonalMaster,
    known_files: &BTreeSet<PathBuf>,
) -> Result<Adoption> {
    let installed = additional_targets(home, master)?;
    let tools = master
        .overlays
        .keys()
        .map(String::as_str)
        .chain(installed.iter().map(|target| target.id));
    let files = rulesync_adopt::discover_personal_files(home, tools, known_files)?;
    let mut adoption = discover_targets(home, &installed, Some(master), Some(files))?;
    adoption.settings = crate::rulesync_settings::personal::Adoption::discover(home)?;
    adoption.added_tools = installed
        .iter()
        .map(|target| target.tool.display_name().to_owned())
        .collect();
    Ok(adoption)
}

pub fn additional_personal_files(
    home: &Path,
    master: &PersonalMaster,
    known_files: &BTreeSet<PathBuf>,
) -> Result<Vec<PathBuf>> {
    let tools = master.overlays.keys().map(String::as_str);
    Ok(
        rulesync_adopt::discover_personal_files(home, tools, known_files)?
            .files
            .into_keys()
            .collect(),
    )
}

fn installed_targets<'a>(
    home: &Path,
    candidates: impl Iterator<Item = &'a PersonalTarget>,
) -> Result<(Vec<&'a PersonalTarget>, Vec<&'a PersonalTarget>)> {
    let root = Dir::open_ambient_dir(home, cap_std::ambient_authority())?;
    let mut targets = Vec::new();
    let mut skipped_tools = Vec::new();
    for target in candidates {
        match root.symlink_metadata(target.folder()) {
            Ok(_) => targets.push(target),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                skipped_tools.push(target);
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect {}", target.folder().display()));
            }
        }
    }
    Ok((targets, skipped_tools))
}

fn record_skipped_tools(adoption: &mut Adoption, tools: Vec<&PersonalTarget>) {
    for target in tools {
        adoption.skipped_tools.push(target.skipped_note());
        adoption.skipped_tool_codes.push(RulesSetupNotice::for_tool(
            Code::ToolMissing,
            target.tool.display_name(),
        ));
    }
}

/// The catalog name for a home-relative adoption destination.
pub fn adoption_tool(path: &Path) -> Option<RulesSetupTool> {
    let personal_file = crate::rulesync_personal_files::LOCATIONS
        .iter()
        .find(|spec| path.starts_with(spec.directory));
    PERSONAL_TARGETS
        .iter()
        .find(|target| {
            path == Path::new(target.rules.destination)
                || path.starts_with(target.skills.destination)
                || personal_file.is_some_and(|file| file.tool == target.id)
                || (target.id == "antigravity-ide"
                    && (path.starts_with(".gemini/config/workflows")
                        || path.starts_with(".gemini/antigravity/global_workflows")))
        })
        .map(|target| RulesSetupTool {
            id: target.agent.into(),
            name: target.tool.display_name().into(),
        })
}

/// The unit a preview counts for a new file: the skill folder for a file
/// inside a skills directory, otherwise the file itself.
fn new_file_unit(path: &Path) -> PathBuf {
    PERSONAL_TARGETS
        .iter()
        .flat_map(|target| [target.skills.destination, ".agents/skills"])
        .find_map(|root| {
            let rest = path.strip_prefix(root).ok()?;
            let folder = rest.components().next()?;
            Some(Path::new(root).join(folder))
        })
        .unwrap_or_else(|| path.to_path_buf())
}

fn discover_targets(
    home: &Path,
    installed: &[&PersonalTarget],
    master: Option<&PersonalMaster>,
    personal_files: Option<rulesync_adopt::PersonalFileDiscovery>,
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
    if let Some(files) = personal_files {
        files.add_to(&mut adoption)?;
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
                            AdoptionSkip::new(
                                Code::SkillPreviewFailed,
                                format!("cannot preview the delivered SKILL.md ({error:#})"),
                            ),
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

const ADOPTION_TOKEN_VERSION: &str = "adopt-7";
const ADOPTION_FIELDS: [&str; 8] = [
    "file entries",
    "skill locations and upstreams",
    "skipped paths",
    "personal rules",
    "personal skill master",
    "personal commands and subagents",
    "Git worktree destinations",
    "personal settings",
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
        fingerprint(&(
            adoption.includes_personal_files,
            &adoption.personal_files,
            &adoption.personal_file_notes,
            &adoption.skipped_personal_files,
        ))?,
        fingerprint(&adoption.worktree_writes)?,
        fingerprint(&adoption.settings)?,
    ])
}

pub fn adoption_plan_id(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))[..12].to_owned()
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
        plan.plan_id.clear();
        plan.note_codes.insert(0, Code::OfflineInventory.into());
        plan.notes.insert(0, "Offline inventory from the installed files. Run `wenlan rules adopt` with the local daemon to see the exact deliveries and additions to your setup.".into());
        Ok(plan)
    } else {
        let (installed, skipped) =
            installed_targets(home, [&CLAUDE_TARGET, &CODEX_TARGET].into_iter())?;
        let mut adoption = discover_targets(home, &installed, None, None)?;
        record_skipped_tools(&mut adoption, skipped);
        legacy_skill_previews(&mut adoption);
        let mut plan = adoption_plan(&adoption)?;
        plan.plan_id.clear();
        Ok(plan)
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

fn display_entry(entry: &AdoptionEntry) -> (Option<Code>, &str) {
    match entry {
        AdoptionEntry::File(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) if !text.contains('\0') => (None, text),
            _ => (Some(Code::EntryBinary), ""),
        },
        AdoptionEntry::Symlink { bytes, .. } => (
            Some(Code::EntryLink),
            bytes
                .as_deref()
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .filter(|text| !text.contains('\0'))
                .unwrap_or_default(),
        ),
        AdoptionEntry::Directory => (Some(Code::EntryDirectory), ""),
        AdoptionEntry::Missing => (None, ""),
    }
}

pub fn adoption_plan(adoption: &Adoption) -> Result<AdoptionPlan> {
    let mut files = Vec::new();
    for (path, file) in &adoption.files {
        if matches!(file.before, AdoptionEntry::Missing)
            && matches!(file.after, AdoptionEntry::Missing)
        {
            continue;
        }
        let personal_source = adoption.personal_files.contains_key(path);
        let personal_note = adoption.personal_file_notes.get(path);
        let tool = adoption_tool(path);
        let mut notice_codes = adoption
            .personal_file_codes
            .get(path)
            .copied()
            .into_iter()
            .collect::<Vec<_>>();
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
        let (added, removed, mut diff) = describe_change(&path, &before, &after);
        // The terminal diff also describes links, directories and binary
        // entries in English. Preview only the underlying text in the app.
        let (before_code, before_text) = display_entry(&file.before);
        let (after_code, after_text) = display_entry(&file.after);
        let text_diff = if before_code.is_some() || after_code.is_some() {
            notice_codes.extend(before_code);
            if after_code != before_code {
                notice_codes.extend(after_code);
            }
            AdoptionTextDiff::Extracted {
                diff: describe_change(&path, before_text, after_text).2,
            }
        } else {
            AdoptionTextDiff::FileText
        };
        if let Some(note) = personal_note {
            diff.push_str(note);
            diff.push('\n');
        } else if personal_source && diff.is_empty() {
            notice_codes.push(Code::PersonalImport);
            diff.push_str("Import the file as a personal source at this path.\n");
        }
        files.push(AdoptionChange {
            path,
            added,
            removed,
            diff,
            tool,
            notice_codes,
            text_diff: Some(text_diff),
        });
    }
    let mut skipped_files = Vec::new();
    for (path, reason) in adoption
        .skipped
        .iter()
        .chain(&adoption.skipped_rules)
        .chain(&adoption.skipped_personal_files)
    {
        let diff = if adoption.skipped_personal_files.contains_key(path) {
            format!("Skipped and left unmanaged: {}.\n", reason.reason)
        } else {
            format!(
                "Skipped ({}). Stays where it is, not managed.\n",
                reason.reason
            )
        };
        skipped_files.push(plan_path(path));
        files.push(AdoptionChange {
            path: plan_path(path),
            added: 0,
            removed: 0,
            diff,
            tool: adoption_tool(path),
            notice_codes: vec![reason.code],
            text_diff: None,
        });
    }
    files.extend(adoption.settings.preview()?);
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
            tool: adoption_tool(path),
            reason_code: None,
            byte_copy_reason: adoption.skill_byte_copies.get(path).cloned(),
            byte_copy_reason_code: adoption
                .skill_byte_copies
                .contains_key(path)
                .then_some(Code::SkillByteCopy),
        })
        .chain(adoption.skipped.iter().map(|(path, reason)| AdoptionSkill {
            path: plan_path(path),
            outcome: AdoptionSkillOutcome::Skipped {
                reason: reason.reason.clone(),
            },
            tool: adoption_tool(path),
            reason_code: Some(reason.code),
            byte_copy_reason: None,
            byte_copy_reason_code: None,
        }))
        .collect::<Vec<_>>();
    skills.sort_by(|first, second| first.path.cmp(&second.path));
    let mut notes = Vec::new();
    let mut note_codes = Vec::new();
    if !adoption.added_tools.is_empty() {
        note_codes.push(RulesSetupNotice {
            code: Code::AddTools,
            tools: adoption.added_tools.clone(),
        });
        notes.push(format!(
            "Add to the existing setup: {}.",
            adoption.added_tools.join(", ")
        ));
    }
    if adoption.codex_sync_replaced {
        note_codes.push(Code::CodexScript.into());
        notes.push("~/.agents/scripts/sync-codex-agents.sh becomes unused for Codex AGENTS.md after adoption. The script stays in place for you to keep or remove.".into());
    }
    note_codes.push(Code::SharedUntouched.into());
    notes.push("~/.agents is left untouched. Wenlan replaces adopted links at their tool paths and leaves the shared sources in place.".into());
    notes.extend(adoption.skipped_tools.iter().cloned());
    note_codes.extend(adoption.skipped_tool_codes.iter().cloned());
    notes.extend(adoption.worktree_notes.iter().cloned());
    note_codes.extend(adoption.worktree_note_codes.iter().cloned());
    if adoption.includes_personal_files {
        note_codes.extend([
            Code::PersonalImportNotes.into(),
            Code::IgnoreUnsupported.into(),
        ]);
        notes.push("Commands and subagents are adopted at their native tool paths. All-tools review shares compatible files with enrolled tools. Tool-specific metadata and Codex TOML subagents stay with their own tool.".into());
        notes.push("Personal ignore: none supported by the eight tools in this setup.".into());
    }
    // Settings notes carry no notice code; keep them after the coded notes so
    // `note_codes` stays aligned with `notes` by index.
    notes.extend(adoption.settings.notes.iter().cloned());
    let token = adoption_token(adoption)?;
    // A new skill copy counts once per folder, not once per file inside it.
    let mut created = BTreeMap::<String, BTreeSet<PathBuf>>::new();
    for (path, file) in &adoption.files {
        if matches!(file.before, AdoptionEntry::Missing)
            && !matches!(file.after, AdoptionEntry::Missing)
        {
            if let Some(tool) = adoption_tool(path) {
                created
                    .entry(tool.name)
                    .or_default()
                    .insert(new_file_unit(path));
            }
        }
    }
    Ok(AdoptionPlan {
        rule_lines: Some(
            std::iter::once(&adoption.shared)
                .chain(adoption.overlays.values())
                .flat_map(|text| text.lines())
                .filter(|line| !line.trim().is_empty())
                .count(),
        ),
        new_files: created
            .into_iter()
            .map(|(tool, units)| AdoptionNewFiles {
                tool,
                files: units.len(),
            })
            .collect(),
        settings: None,
        plan_id: adoption_plan_id(&token),
        token,
        files,
        skills,
        notes,
        kept_in_place: adoption
            .settings
            .kept_in_place
            .iter()
            .map(|(tool, count)| KeptInPlace {
                tool: tool.clone(),
                count: *count,
            })
            .collect(),
        note_codes,
        skipped_files,
        skipped_tools: adoption.skipped_tools.clone(),
        skipped_tool_codes: adoption.skipped_tool_codes.clone(),
        tools: PERSONAL_TARGETS
            .iter()
            .filter_map(|target| {
                let rules = usize::from(
                    adoption
                        .files
                        .get(Path::new(target.rules.destination))
                        .is_some_and(|file| !matches!(file.after, AdoptionEntry::Missing)),
                );
                let skills = adoption
                    .skill_locations
                    .keys()
                    .filter(|path| path.starts_with(target.skills.destination))
                    .count();
                let other_files = adoption
                    .personal_files
                    .keys()
                    .filter(|path| {
                        crate::rulesync_personal_files::location_of(path)
                            .is_some_and(|location| location.tool == target.id)
                    })
                    .count();
                (adoption.overlays.contains_key(target.id) || rules + skills + other_files > 0)
                    .then(|| AdoptionTool {
                        name: target.tool.display_name().into(),
                        rule_files: rules,
                        skills,
                        other_files,
                    })
            })
            .collect(),
    })
}

/// Wenlan keeps imported text as authored. Only Wenlan-generated additions
/// have already passed through the UX-SKILL-8 escaping in skill_grow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(deserialize = "Content: Deserialize<'de>"))]
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
    describe_line_changes(path, changes)
}

/// A command or subagent is approved as one file. Show a whole-file
/// replacement when computing a granular edit would exceed the diff budget.
pub fn describe_personal_file_change(path: &str, before: &str, after: &str) -> String {
    let crlf = "\r\n";
    if before.contains(crlf) && before.replace(crlf, "\n") == after {
        return format!(
            "Converted {} CRLF line endings to LF.\n",
            before.matches(crlf).count()
        );
    }
    if after.contains(crlf) && after.replace(crlf, "\n") == before {
        return format!(
            "Converted {} LF line endings to CRLF.\n",
            after.matches(crlf).count()
        );
    }
    let changes = line_changes(before, after).unwrap_or_else(|_| {
        vec![LineChange {
            start: 0,
            removed: before.split_inclusive('\n').map(str::to_owned).collect(),
            added: after.split_inclusive('\n').map(str::to_owned).collect(),
        }]
    });
    describe_line_changes(path, changes).2
}

fn describe_line_changes(path: &str, changes: Vec<LineChange>) -> (usize, usize, String) {
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
    fn rulesync_preview_names_tools_that_get_new_files() {
        let home = tempfile::tempdir().unwrap();
        for (path, text) in [(".agents/AGENTS.md", "Rule.\n"), (".gemini/note", "x")] {
            let path = home.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let plan = plan_adoption(home.path()).unwrap();
        assert_eq!(
            plan.new_files,
            vec![AdoptionNewFiles {
                tool: "Gemini CLI".into(),
                files: 1
            }]
        );
        fs::write(home.path().join(".gemini/GEMINI.md"), "Rule.\n").unwrap();
        assert!(plan_adoption(home.path()).unwrap().new_files.is_empty());
    }

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
        // The shared line is counted once, and each tool-only line once.
        assert_eq!(plan.rule_lines, Some(4));
        assert_eq!(plan.files.len(), 3);
        assert_eq!(plan.notes.len(), plan.note_codes.len());
        assert_eq!(plan.skipped_tools.len(), plan.skipped_tool_codes.len());
        assert!(plan
            .note_codes
            .iter()
            .any(|note| note.code == Code::SharedUntouched));
        assert!(plan
            .note_codes
            .iter()
            .any(|note| note.code == Code::PersonalImportNotes));
        assert!(plan
            .note_codes
            .iter()
            .any(|note| note.code == Code::IgnoreUnsupported));
        for tool in ["Claude Code", "Codex", "Cursor", "OpenCode", "Muse Code"] {
            assert!(plan
                .skipped_tool_codes
                .contains(&RulesSetupNotice::for_tool(Code::ToolMissing, tool)));
            assert!(plan
                .notes
                .iter()
                .any(|note| note.starts_with(&format!("Skipped {tool} "))));
        }
        assert!(!home.path().join(".cursor").exists());
        assert!(!home.path().join(".config").exists());
    }

    #[test]
    fn ux_cli_8_adoption_does_not_create_an_empty_rule_file_for_an_installed_tool() {
        for shared in ["", "\n \t\n", "\u{2003}\n"] {
            let home = tempfile::tempdir().unwrap();
            fs::create_dir_all(home.path().join(".claude")).unwrap();
            fs::create_dir_all(home.path().join(".agents")).unwrap();
            fs::write(home.path().join(".agents/AGENTS.md"), shared).unwrap();

            let adoption = discover_adoption(home.path()).unwrap();
            let no_op = &adoption.files[Path::new(".claude/CLAUDE.md")];
            assert!(matches!(no_op.before, AdoptionEntry::Missing));
            assert!(matches!(no_op.after, AdoptionEntry::Missing), "{shared:?}");
            assert!(adoption.overlays.contains_key("claudecode"));

            let plan = adoption_plan(&adoption).unwrap();
            assert_eq!(plan.rule_lines, Some(0));
            assert!(!plan
                .files
                .iter()
                .any(|file| file.path == ".claude/CLAUDE.md"));
            assert!(plan.new_files.is_empty());
            assert!(plan
                .tools
                .iter()
                .any(|tool| tool.name == "Claude Code" && tool.rule_files == 0));
        }
    }

    #[test]
    fn rulesync_adoption_plan_codes_keep_terminal_diagnostics_and_supply_tool_names() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        for (path, bytes) in [
            (".agents/skills/shared/SKILL.md", &b"A shared skill.\n"[..]),
            (".claude/skills/no-entry/local.txt", &b"Some file.\n"[..]),
            (".claude/commands/確認.md", &b"A native command.\n"[..]),
            (".gemini/config/workflows/old.md", &b"Old workflow.\n"[..]),
        ] {
            let path = home.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let plan = plan_adoption(home).unwrap();
        for (path, code, tool) in [
            (".agents/skills/shared", Code::SharedSkill, None),
            (
                ".claude/skills/no-entry",
                Code::SkillEntryMissing,
                Some("Claude Code"),
            ),
            (
                ".claude/commands/確認.md",
                Code::PersonalName,
                Some("Claude Code"),
            ),
            (
                ".gemini/config/workflows",
                Code::LegacyWorkflow,
                Some("Antigravity"),
            ),
        ] {
            assert!(
                plan.skipped_files.iter().any(|file| file == path),
                "{plan:?}"
            );
            let file = plan.files.iter().find(|file| file.path == path).unwrap();
            assert_eq!(file.notice_codes, [code]);
            assert!(file.diff.starts_with("Skipped"));
            assert_eq!(file.tool.as_ref().map(|tool| tool.name.as_str()), tool);
            if let Some(skill) = plan.skills.iter().find(|skill| skill.path == path) {
                assert_eq!(skill.reason_code, Some(code));
            }
        }
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
