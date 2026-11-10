// SPDX-License-Identifier: Apache-2.0
//! Reading files a cloud folder has not downloaded yet.
//!
//! On macOS, Google Drive for desktop, iCloud Drive, Dropbox and OneDrive keep
//! a file's name on disk and download its bytes when a program reads it
//! (a "dataless" file). A background process such as this daemon can be
//! refused that download: the read fails with `EDEADLK` ("Resource deadlock
//! avoided") instead. A shared Space's folder is full of files another member
//! wrote, so the daemon asks macOS, once at start, to download a file when it
//! reads one, as an app the person opened does.
//!
//! Elsewhere nothing is needed: Windows cloud files download on read for any
//! process, and Linux sync tools keep whole files.

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_int;

    // <sys/resource.h>
    const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: c_int = 3;
    const IOPOL_SCOPE_PROCESS: c_int = 0;
    const IOPOL_MATERIALIZE_DATALESS_FILES_ON: c_int = 2;

    unsafe extern "C" {
        fn setiopolicy_np(iotype: c_int, scope: c_int, policy: c_int) -> c_int;
    }

    pub(super) fn allow() -> std::io::Result<()> {
        // SAFETY: setiopolicy_np takes three integers and keeps no pointers;
        // it only changes this process's I/O policy.
        let status = unsafe {
            setiopolicy_np(
                IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
                IOPOL_SCOPE_PROCESS,
                IOPOL_MATERIALIZE_DATALESS_FILES_ON,
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

/// Lets this process read a cloud file that is not downloaded yet, which
/// then downloads. A no-op outside macOS.
pub fn allow_downloading_cloud_files() -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::allow()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn allowing_cloud_downloads_succeeds_on_this_platform() {
        super::allow_downloading_cloud_files().expect("set the I/O policy");
    }
}
