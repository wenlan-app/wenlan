// SPDX-License-Identifier: Apache-2.0
//! Creating the person key the daemon checks (see `wenlan_types::person_key`).
//!
//! The key is made once, on the daemon's first start, and kept: the app and
//! the `wenlan` command read the same file. It is created in a temporary file
//! that only this account can read and then linked into place, so no reader
//! ever sees half a key and two starts never hand out two keys.
//!
//! On Unix the file is `0600`; a key file that anyone else could read is
//! replaced with a new one. On Windows the file takes the access of its
//! folder: the default data root is under the user's own profile
//! (`%LOCALAPPDATA%`), which other accounts cannot read. A `WENLAN_DATA_DIR`
//! elsewhere on Windows keeps that folder's access, and
//! [`readable_by_others`] says so for the daemon to log. (Setting an
//! explicit owner-only ACL would need new Windows security bindings; not
//! done.)

use std::io::Write;
use std::path::Path;

pub use wenlan_types::person_key::{path_in, read, well_formed, FILE_NAME};

/// The person key in `data_root`, made now when there is none (or the one
/// there is not a key, or on Unix could be read by another account).
pub fn ensure(data_root: &Path) -> std::io::Result<String> {
    std::fs::create_dir_all(data_root)?;
    let path = path_in(data_root);
    // Twice at most: a file another start linked in first is checked like
    // any other, and replaced if others could read it.
    for _ in 0..2 {
        if let Some(key) = read(data_root) {
            if only_owner_can_read(&path)? {
                return Ok(key);
            }
        }
        write_new(data_root, &path)?;
    }
    read(data_root).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the person key file holds no key after writing it",
        )
    })
}

/// Write a new key at `path`: replacing what is there, or linking it in
/// when nothing is.
fn write_new(data_root: &Path, path: &Path) -> std::io::Result<()> {
    let replace = path.exists();
    let key = new_key()?;
    let temp = data_root.join(format!(
        ".{FILE_NAME}.{}.{}.tmp",
        std::process::id(),
        &new_key()?[..12]
    ));
    let written = write_owner_only(&temp, &key).and_then(|()| {
        if replace {
            return std::fs::rename(&temp, path);
        }
        match std::fs::hard_link(&temp, path) {
            Ok(()) => Ok(()),
            // Another start made it first: `ensure` checks that one.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            // A file system without hard links: the file did not exist a
            // moment ago, and the daemon's data lock keeps a second daemon out.
            Err(_) => std::fs::rename(&temp, path),
        }
    });
    let _ = std::fs::remove_file(&temp);
    written
}

/// A plain warning, never the key, when other accounts on this computer may
/// be able to read the key file anyway:
///
/// - on Unix, when the file still is not owner-only after it was written
///   (a folder on a file system without Unix permissions, such as exFAT or
///   a network share);
/// - on Windows, when the data root is outside the user's own profile, so
///   the file takes that folder's access instead of the profile's.
pub fn readable_by_others(data_root: &Path) -> Option<String> {
    let path = path_in(data_root);
    #[cfg(unix)]
    {
        if !only_owner_can_read(&path).unwrap_or(true) {
            return Some(format!(
                "the person key file in {} can be read by other accounts: this folder does \
                 not keep Unix permissions. Keep Wenlan's data in a folder that does",
                data_root.display()
            ));
        }
        None
    }
    #[cfg(not(unix))]
    {
        let _ = &path;
        let profile = [dirs::data_local_dir(), dirs::home_dir()];
        let inside = profile
            .iter()
            .flatten()
            .any(|base| data_root.starts_with(base));
        (!inside).then(|| {
            format!(
                "the person key file in {} takes that folder's access, which other accounts \
                 may be able to read: it is outside your user profile. Keep Wenlan's data under \
                 your profile (the default) so only your account can read it",
                data_root.display()
            )
        })
    }
}

/// Whether `given` is `expected`, compared in constant time.
pub fn matches(expected: &str, given: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    expected.len() == given.len() && bool::from(expected.as_bytes().ct_eq(given))
}

/// 32 random bytes as 64 lowercase hex digits: a key, or a one-time pass.
pub fn new_key() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| std::io::Error::other(format!("no random bytes for a key: {error}")))?;
    Ok(hex::encode(bytes))
}

fn write_owner_only(path: &Path, key: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(key.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()
}

#[cfg(unix)]
fn only_owner_can_read(path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    Ok(std::fs::metadata(path)?.permissions().mode() & 0o077 == 0)
}

/// Windows: the folder's access decides (see the module note).
#[cfg(not(unix))]
fn only_owner_can_read(_path: &Path) -> std::io::Result<bool> {
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ux_skill_13_person_key_is_made_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let first = ensure(dir.path()).unwrap();
        assert!(well_formed(&first));
        assert_eq!(ensure(dir.path()).unwrap(), first, "kept across starts");
        assert_eq!(read(dir.path()).as_deref(), Some(first.as_str()));
        // Only the key file is left: the temporary file is gone.
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec![FILE_NAME.to_string()]);
        // A file that holds no key is replaced.
        std::fs::write(path_in(dir.path()), "not a key").unwrap();
        let replaced = ensure(dir.path()).unwrap();
        assert!(well_formed(&replaced));
        assert_ne!(replaced, first);
    }

    #[test]
    fn ux_skill_13_person_key_matches_only_itself() {
        let key = "ab".repeat(32);
        assert!(matches(&key, key.as_bytes()));
        assert!(!matches(&key, "ac".repeat(32).as_bytes()));
        assert!(!matches(&key, &key.as_bytes()[..63]));
        assert!(!matches(&key, b""));
        assert!(!matches(&key, key.to_uppercase().as_bytes()));
    }

    #[cfg(unix)]
    #[test]
    fn ux_skill_13_person_key_file_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let key = ensure(dir.path()).unwrap();
        let mode = |dir: &Path| {
            std::fs::metadata(path_in(dir))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode(dir.path()), 0o600);
        // A key someone else could have read is not kept.
        std::fs::set_permissions(path_in(dir.path()), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        let fresh = ensure(dir.path()).unwrap();
        assert_ne!(fresh, key, "a key others could read is replaced");
        assert_eq!(mode(dir.path()), 0o600);
        assert_eq!(readable_by_others(dir.path()), None);
        // A folder that loses the mode (exFAT, a share) is said plainly,
        // without the key.
        std::fs::set_permissions(path_in(dir.path()), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        let warning = readable_by_others(dir.path()).unwrap();
        assert!(warning.contains("other accounts"), "{warning}");
        assert!(!warning.contains(&fresh));
    }
}
