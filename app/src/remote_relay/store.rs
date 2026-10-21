// SPDX-License-Identifier: AGPL-3.0-only
//! Native-only relay configuration. No legacy ID import and no implicit consent.
use super::{valid_id, valid_space, DeviceCredential, RELAY_ORIGIN};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_BYTES: usize = 16 * 1024;
const MAX_ENCODED_BYTES: usize = 64 * 1024;
const FILE_NAME: &str = "connection.dat";
const BUSY_RETRIES: u32 = 20;
const BUSY_WAIT: std::time::Duration = std::time::Duration::from_millis(25);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StoreError {
    #[error("Remote access is not configured")]
    NotConfigured,
    #[error("Remote access settings changed; reload before continuing")]
    Stale,
    #[error("Disconnect the existing remote device before changing this setting")]
    DisconnectPending,
    #[error("Remote access storage is busy; retry shortly")]
    Busy,
    #[error("Remote access credentials could not be stored safely")]
    Storage,
    #[error("Remote access credential durability could not be confirmed; reload settings")]
    Durability,
    #[error("Remote access credentials are invalid; connection remains disabled")]
    Invalid,
    #[error("Remote access credential protection is unavailable")]
    Protection,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    version: u32,
    revision: String,
    relay_origin: String,
    space: String,
    backend_token: String,
    enabled: bool,
    device: Option<DeviceCredential>,
    /// An app was allowed to write, so the web-write process runs beside the
    /// search-only one. Missing in a profile saved before the choice existed;
    /// not written while false, so an older build can still read the file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    write_enabled: bool,
}

impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RemoteProfile([redacted])")
    }
}

/// The only profile shape suitable for the frontend. Revision is a concurrency
/// marker, not an authorization credential.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileView {
    pub revision: String,
    pub space: String,
    pub enabled: bool,
    pub disconnect_pending: bool,
    pub credential_expires_at: Option<u64>,
}

impl Profile {
    pub fn view(&self) -> ProfileView {
        ProfileView {
            revision: self.revision.clone(),
            space: self.space.clone(),
            enabled: self.enabled,
            disconnect_pending: !self.enabled && self.device.is_some(),
            credential_expires_at: self.device.as_ref().map(|device| device.expires_at),
        }
    }
    pub fn revision(&self) -> &str {
        &self.revision
    }
    pub fn space(&self) -> &str {
        &self.space
    }
    /// Every Space plus everything not in a Space, rather than one Space.
    pub fn whole_library(&self) -> bool {
        self.space == super::WHOLE_LIBRARY_SPACE
    }
    pub fn backend_token(&self) -> &str {
        &self.backend_token
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn device(&self) -> Option<&DeviceCredential> {
        self.device.as_ref()
    }
    /// Whether the local web-write process should run for this profile.
    pub fn write_enabled(&self) -> bool {
        self.write_enabled
    }

    fn validate(&self) -> Result<(), StoreError> {
        let valid_device = self.device.as_ref().is_none_or(|device| {
            valid_id(&device.id, 32)
                && valid_id(&device.management_token, 32)
                && device.expires_at > 0
        });
        if self.version == 1
            && self.relay_origin == RELAY_ORIGIN
            && valid_id(&self.revision, 32)
            && valid_space(&self.space)
            && valid_id(&self.backend_token, 32)
            && valid_device
        {
            Ok(())
        } else {
            Err(StoreError::Invalid)
        }
    }
}

#[derive(Clone)]
pub struct Store {
    directory: PathBuf,
}

impl Store {
    #[cfg(test)]
    pub(super) fn in_directory(directory: PathBuf) -> Self {
        Self { directory }
    }
    pub fn current() -> Self {
        Self {
            directory: crate::identity_paths::mcp_config_dir().join("relay-v1"),
        }
    }

    /// Missing data is not consent. Callers must not create a profile based only
    /// on the legacy remote_access_enabled setting.
    pub fn load(&self) -> Result<Option<Profile>, StoreError> {
        match std::fs::symlink_metadata(&self.directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(StoreError::Storage),
            Ok(_) => {}
        }
        self.with_lock(read_profile)
    }

    /// Select an explicit scope without enabling exposure. A connected or
    /// pending-revocation profile cannot silently be replaced by a new scope.
    pub fn configure(&self, expected: Option<&str>, space: &str) -> Result<Profile, StoreError> {
        if !valid_space(space) {
            return Err(StoreError::Invalid);
        }
        self.with_lock(|path| {
            let previous = read_profile(path)?;
            if previous.as_ref().map(|profile| profile.revision.as_str()) != expected {
                return Err(StoreError::Stale);
            }
            if previous
                .as_ref()
                .is_some_and(|profile| profile.enabled || profile.device.is_some())
            {
                return Err(StoreError::DisconnectPending);
            }
            let profile = Profile {
                version: 1,
                revision: secret()?,
                relay_origin: RELAY_ORIGIN.into(),
                space: space.into(),
                backend_token: secret()?,
                enabled: false,
                device: None,
                write_enabled: false,
            };
            write_profile(path, &profile)?;
            Ok(profile)
        })
    }

    pub fn enable(&self, expected: &str) -> Result<Profile, StoreError> {
        self.change(expected, |profile| {
            if profile.enabled || profile.device.is_some() {
                return Err(StoreError::DisconnectPending);
            }
            profile.backend_token = secret()?;
            profile.enabled = true;
            profile.write_enabled = false;
            Ok(())
        })
    }

    /// Save an enrollment or a same-device rotation. An old request cannot
    /// publish credentials into a newer or disabled profile.
    pub fn attach_device(
        &self,
        expected: &str,
        device: DeviceCredential,
    ) -> Result<Profile, StoreError> {
        device.validate().map_err(|_| StoreError::Invalid)?;
        self.change(expected, |profile| {
            if !profile.enabled {
                return Err(StoreError::Stale);
            }
            if profile
                .device
                .as_ref()
                .is_some_and(|current| current.id != device.id)
            {
                return Err(StoreError::Stale);
            }
            // A new or rotated key ends every grant the relay holds for this
            // computer, so no write approval survives it.
            profile.device = Some(device);
            profile.write_enabled = false;
            Ok(())
        })
    }

    /// Record that an app was allowed to write. Unlike every other change this
    /// keeps the revision: it only adds a process beside the running one, so a
    /// connection captured at this revision stays current. It applies only to
    /// the enabled, enrolled profile the person approved against, and a stale
    /// caller cannot switch writing on for a newer profile.
    pub fn enable_write(&self, expected: &str) -> Result<Profile, StoreError> {
        self.with_lock(|path| {
            let mut profile = read_profile(path)?.ok_or(StoreError::NotConfigured)?;
            if profile.revision != expected {
                return Err(StoreError::Stale);
            }
            if !profile.enabled || profile.device.is_none() {
                return Err(StoreError::Stale);
            }
            if !profile.write_enabled {
                profile.write_enabled = true;
                write_profile(path, &profile)?;
            }
            Ok(profile)
        })
    }

    /// Persist the person's stop intent before network revocation. Keep the
    /// management credential until the matching remote device is disconnected.
    pub fn disable(&self, expected: &str) -> Result<Profile, StoreError> {
        self.change(expected, |profile| {
            profile.enabled = false;
            profile.write_enabled = false;
            Ok(())
        })
    }

    pub fn finish_disconnect(
        &self,
        expected: &str,
        device_id: &str,
    ) -> Result<Profile, StoreError> {
        self.change(expected, |profile| {
            if profile.enabled
                || profile.device.as_ref().map(|device| device.id.as_str()) != Some(device_id)
            {
                return Err(StoreError::Stale);
            }
            profile.device = None;
            profile.backend_token = secret()?;
            profile.write_enabled = false;
            Ok(())
        })
    }

    fn change(
        &self,
        expected: &str,
        operation: impl FnOnce(&mut Profile) -> Result<(), StoreError>,
    ) -> Result<Profile, StoreError> {
        self.with_lock(|path| {
            let mut profile = read_profile(path)?.ok_or(StoreError::NotConfigured)?;
            if profile.revision != expected {
                return Err(StoreError::Stale);
            }
            operation(&mut profile)?;
            profile.revision = secret()?;
            write_profile(path, &profile)?;
            Ok(profile)
        })
    }

    fn with_lock<T>(
        &self,
        operation: impl FnOnce(&Path) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        prepare_directory(&self.directory)?;
        let lock_path = self.directory.join("connection.lock");
        regular_or_missing(&lock_path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(&lock_path).map_err(|_| StoreError::Storage)?;
        private_file(&lock)?;
        // Another task holds the lock only while it reads or saves, so wait
        // that out instead of failing: a Busy read in the connection monitor
        // turns Web access off, and one in the settings panel shows an error.
        let mut attempts = 0;
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) if attempts < BUSY_RETRIES => {
                    attempts += 1;
                    std::thread::sleep(BUSY_WAIT);
                }
                Err(std::fs::TryLockError::WouldBlock) => return Err(StoreError::Busy),
                Err(std::fs::TryLockError::Error(_)) => return Err(StoreError::Storage),
            }
        }
        // Arm only after acquisition; a failed contender must never unlock.
        let _guard = LockGuard::hold(lock);
        operation(&self.directory.join(FILE_NAME))
    }
}

/// Closing alone can retain the lock through a fork-inherited descriptor.
/// Explicitly release our successful acquisition, including during unwind.
struct LockGuard {
    file: File,
}

impl LockGuard {
    fn hold(file: File) -> Self {
        Self { file }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn secret() -> Result<String, StoreError> {
    let mut bytes = [0; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| StoreError::Protection)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn regular_or_missing(path: &Path) -> Result<(), StoreError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(StoreError::Storage),
    }
}

fn prepare_directory(path: &Path) -> Result<(), StoreError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(StoreError::Storage)
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(StoreError::Storage)
        }
        _ => {}
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|_| StoreError::Storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| StoreError::Storage)?;
    }
    Ok(())
}

fn private_file(file: &File) -> Result<(), StoreError> {
    if !file.metadata().map_err(|_| StoreError::Storage)?.is_file() {
        return Err(StoreError::Storage);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| StoreError::Storage)?;
    }
    Ok(())
}

fn read_profile(path: &Path) -> Result<Option<Profile>, StoreError> {
    regular_or_missing(path)?;
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(StoreError::Storage),
    };
    private_file(&file)?;
    let mut bytes = Vec::new();
    file.take(MAX_ENCODED_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| StoreError::Storage)?;
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err(StoreError::Invalid);
    }
    let plaintext = unprotect(&bytes)?;
    if plaintext.len() > MAX_BYTES {
        return Err(StoreError::Invalid);
    }
    let profile: Profile = serde_json::from_slice(&plaintext).map_err(|_| StoreError::Invalid)?;
    profile.validate()?;
    Ok(Some(profile))
}

fn write_profile(path: &Path, profile: &Profile) -> Result<(), StoreError> {
    profile.validate()?;
    regular_or_missing(path)?;
    let plaintext = serde_json::to_vec(profile).map_err(|_| StoreError::Invalid)?;
    if plaintext.len() > MAX_BYTES {
        return Err(StoreError::Invalid);
    }
    let encoded = protect(&plaintext)?;
    if encoded.len() > MAX_ENCODED_BYTES {
        return Err(StoreError::Protection);
    }
    let parent = path.parent().ok_or(StoreError::Storage)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|_| StoreError::Storage)?;
    private_file(staged.as_file())?;
    staged
        .write_all(&encoded)
        .map_err(|_| StoreError::Storage)?;
    staged
        .as_file()
        .sync_all()
        .map_err(|_| StoreError::Storage)?;
    staged.persist(path).map_err(|_| StoreError::Storage)?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| StoreError::Durability)?;
    Ok(())
}

#[cfg(unix)]
fn protect(bytes: &[u8]) -> Result<Vec<u8>, StoreError> {
    Ok(bytes.to_vec())
}
#[cfg(unix)]
fn unprotect(bytes: &[u8]) -> Result<Vec<u8>, StoreError> {
    Ok(bytes.to_vec())
}

#[cfg(windows)]
fn protect(bytes: &[u8]) -> Result<Vec<u8>, StoreError> {
    let mut output = b"WENLAN-DPAPI-1\n".to_vec();
    output.extend(dpapi(bytes, true)?);
    Ok(output)
}
#[cfg(windows)]
fn unprotect(bytes: &[u8]) -> Result<Vec<u8>, StoreError> {
    dpapi(
        bytes
            .strip_prefix(b"WENLAN-DPAPI-1\n")
            .ok_or(StoreError::Protection)?,
        false,
    )
}

#[cfg(windows)]
fn dpapi(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>, StoreError> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    if bytes.is_empty() || bytes.len() > MAX_ENCODED_BYTES {
        return Err(StoreError::Protection);
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    // User-scoped DPAPI only: no LOCAL_MACHINE flag, prompts, custom crypto or
    // plaintext fallback. Win32 owns the returned buffer until LocalFree.
    let success = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    let result = if success != 0
        && !output.pbData.is_null()
        && output.cbData > 0
        && output.cbData as usize <= MAX_ENCODED_BYTES
    {
        Ok(unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec())
    } else {
        Err(StoreError::Protection)
    };
    if !output.pbData.is_null() {
        unsafe {
            LocalFree(output.pbData.cast());
        }
    }
    result
}

#[cfg(test)]
mod tests;
