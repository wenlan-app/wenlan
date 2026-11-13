// SPDX-License-Identifier: AGPL-3.0-only
//! Point `~/.wenlan/bin` at this app's bundled binaries when the copies there
//! are older. The previous file is renamed aside; it is not deleted.

use wenlan_core::home_bin::{
    backup_stamp_now, link_mode_for, may_rewrite_user_home_bin, read_binary_version, sync_home_bin,
    DesktopHost, EntryAction, LinkMode, SyncRequest, Version,
};

/// Whether this process runs from a mounted AppImage. The mount
/// (`/tmp/.mount_*`, or a `/tmp/appimage_extracted_*` copy without FUSE) is
/// made fresh on every launch and goes away when the app quits, so nothing
/// that must outlive the app (the `~/.wenlan/bin` entries, the path the AI
/// tool plugins read, an AI tool's MCP config) may point into it.
pub(crate) fn running_from_appimage() -> bool {
    if !cfg!(target_os = "linux") || std::env::var_os("APPIMAGE").is_none() {
        return false;
    }
    match (std::env::var_os("APPDIR"), std::env::current_exe()) {
        (Some(appdir), Ok(exe)) => exe_inside_appdir(&exe, std::path::Path::new(&appdir)),
        _ => false,
    }
}

fn exe_inside_appdir(exe: &std::path::Path, appdir: &std::path::Path) -> bool {
    !appdir.as_os_str().is_empty() && exe.starts_with(appdir)
}

/// The directory whose binaries outlive this launch: the app's own directory,
/// or `~/.wenlan/bin` when the app runs from an AppImage mount.
pub(crate) fn lasting_bin_dir(
    exe_dir: &std::path::Path,
    home: &std::path::Path,
) -> std::path::PathBuf {
    if running_from_appimage() {
        wenlan_core::home_bin::home_bin_dir(home)
    } else {
        exe_dir.to_path_buf()
    }
}

pub fn sync_bundled_home_bin() {
    if !may_rewrite_user_home_bin(
        cfg!(debug_assertions),
        crate::lifecycle::data_dir_env_overridden(),
    ) {
        return;
    }
    let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
    else {
        log::warn!("[home-bin] could not read the app's own path");
        return;
    };
    let Some(home) = dirs::home_dir() else {
        log::warn!("[home-bin] no home directory");
        return;
    };
    let Ok(version) = Version::parse(env!("CARGO_PKG_VERSION")) else {
        log::warn!("[home-bin] this app's version could not be parsed");
        return;
    };
    let host = DesktopHost::current();
    let actions = sync_home_bin(
        &SyncRequest {
            home: &home,
            bundle_dir: &exe_dir,
            bundle_version: &version,
            host,
            // A link into an AppImage mount dangles once the app quits, so
            // an AppImage copies the binaries out instead.
            mode: if running_from_appimage() {
                LinkMode::CopyAll
            } else {
                link_mode_for(host)
            },
            stamp: &backup_stamp_now(),
        },
        &mut read_binary_version,
    );
    for action in &actions {
        if action.should_report() {
            match action {
                EntryAction::Failed { .. } => log::warn!("[home-bin] {}", action.summary()),
                _ => log::info!("[home-bin] {}", action.summary()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::exe_inside_appdir;
    use std::path::Path;

    #[test]
    fn an_exe_counts_as_inside_the_appimage_only_under_its_appdir() {
        let appdir = Path::new("/tmp/.mount_WenlanAbc");
        assert!(exe_inside_appdir(
            Path::new("/tmp/.mount_WenlanAbc/usr/bin/wenlan-app"),
            appdir
        ));
        assert!(!exe_inside_appdir(Path::new("/usr/bin/wenlan-app"), appdir));
        assert!(!exe_inside_appdir(
            Path::new("/tmp/.mount_WenlanAbcd/usr/bin/wenlan-app"),
            appdir
        ));
        assert!(!exe_inside_appdir(
            Path::new("/usr/bin/wenlan-app"),
            Path::new("")
        ));
    }
}
