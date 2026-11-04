// SPDX-License-Identifier: AGPL-3.0-only
//! Point `~/.wenlan/bin` at this app's bundled binaries when the copies there
//! are older. The previous file is renamed aside; it is not deleted.

use wenlan_core::home_bin::{
    backup_stamp_now, link_mode_for, may_rewrite_user_home_bin, read_binary_version, sync_home_bin,
    DesktopHost, EntryAction, SyncRequest, Version,
};

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
            mode: link_mode_for(host),
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
