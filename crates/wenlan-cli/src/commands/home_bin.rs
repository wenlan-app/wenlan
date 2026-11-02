// SPDX-License-Identifier: Apache-2.0
//! Doctor output and setup refresh for `~/.wenlan/bin`.

use std::path::PathBuf;

use wenlan_core::home_bin::{
    backup_stamp_now, default_bundle_candidates, discover_bundle_dir, executable_name,
    inspect_home_bin, link_mode_for, may_rewrite_user_home_bin, read_binary_version, relaunch_hint,
    render_actions, render_binary_report, sync_home_bin, DesktopHost, DiscoverRequest,
    InspectRequest, SyncRequest, Version,
};

pub fn print_doctor_binaries(daemon_version: Option<Version>, repair: bool) {
    let home = match super::mcp::home_dir_path() {
        Ok(home) => home,
        Err(error) => {
            println!("Binaries: {error}");
            return;
        }
    };
    let cli_version = match Version::parse(env!("CARGO_PKG_VERSION")) {
        Ok(version) => version,
        Err(error) => {
            println!("Binaries: this wenlan's version is unreadable ({error}).");
            return;
        }
    };
    let located = locate(&home);
    let report = inspect_home_bin(
        &InspectRequest {
            home: &home,
            host: located.host,
            bundle_dir: located.bundle_dir.as_deref(),
            bundle_version: located.bundle_version.as_ref(),
            bundle_version_error: located.bundle_version_error.as_deref(),
            cli_version: &cli_version,
            daemon_version: daemon_version.as_ref(),
        },
        &mut read_binary_version,
    );
    println!("{}", render_binary_report(&report));
    if !repair {
        return;
    }
    match repair_with(&home, &located) {
        Repair::NoBundle => {
            println!("No app bundle was found, so ~/.wenlan/bin was not changed.");
        }
        Repair::UnreadableBundle => {
            println!(
                "The app bundle's version could not be read, so ~/.wenlan/bin was not changed."
            );
        }
        Repair::Done { text, relaunch } => {
            match text {
                Some(text) => println!("{text}"),
                None => println!("~/.wenlan/bin already matches the app bundle."),
            }
            if let Some(hint) = relaunch {
                println!("{hint}");
            }
        }
    }
}

/// Release installs refresh stale home-bin copies during `wenlan setup`.
/// A debug build or a scratch `WENLAN_DATA_DIR` does not.
pub fn refresh_for_setup() -> Option<String> {
    if !may_rewrite_user_home_bin(
        cfg!(debug_assertions),
        std::env::var_os("WENLAN_DATA_DIR").is_some()
            || std::env::var_os("ORIGIN_DATA_DIR").is_some(),
    ) {
        return None;
    }
    let home = super::mcp::home_dir_path().ok()?;
    match repair_with(&home, &locate(&home)) {
        Repair::Done { text, relaunch } => {
            let mut lines = Vec::new();
            if let Some(text) = text {
                lines.push(text);
            }
            if let Some(hint) = relaunch {
                lines.push(hint.to_string());
            }
            if lines.is_empty() {
                None
            } else {
                Some(lines.join("\n"))
            }
        }
        Repair::NoBundle | Repair::UnreadableBundle => None,
    }
}

struct Located {
    host: DesktopHost,
    bundle_dir: Option<PathBuf>,
    bundle_version: Option<Version>,
    bundle_version_error: Option<String>,
}

fn locate(home: &std::path::Path) -> Located {
    let host = DesktopHost::current();
    let candidates = default_bundle_candidates(
        home,
        host,
        dirs::data_local_dir().as_deref(),
        program_files().as_deref(),
    );
    let override_dir = std::env::var_os("WENLAN_BUNDLE_DIR")
        .map(PathBuf::from)
        .filter(|dir| !dir.as_os_str().is_empty());
    let current_exe = std::env::current_exe().ok();
    let bundle_dir = discover_bundle_dir(&DiscoverRequest {
        home,
        host,
        current_exe: current_exe.as_deref(),
        override_dir: override_dir.as_deref(),
        candidates: &candidates,
    });
    let (bundle_version, bundle_version_error) = match &bundle_dir {
        Some(dir) => match read_binary_version(&dir.join(executable_name("wenlan", host))) {
            Ok(version) => (Some(version), None),
            Err(error) => (None, Some(error)),
        },
        None => (None, None),
    };
    Located {
        host,
        bundle_dir,
        bundle_version,
        bundle_version_error,
    }
}

enum Repair {
    NoBundle,
    UnreadableBundle,
    Done {
        text: Option<String>,
        relaunch: Option<&'static str>,
    },
}

fn repair_with(home: &std::path::Path, located: &Located) -> Repair {
    let Some(dir) = located.bundle_dir.as_deref() else {
        return Repair::NoBundle;
    };
    let Some(version) = located.bundle_version.as_ref() else {
        return Repair::UnreadableBundle;
    };
    let actions = sync_home_bin(
        &SyncRequest {
            home,
            bundle_dir: dir,
            bundle_version: version,
            host: located.host,
            mode: link_mode_for(located.host),
            stamp: &backup_stamp_now(),
        },
        &mut read_binary_version,
    );
    let relaunch = relaunch_hint(&actions);
    Repair::Done {
        text: render_actions(&actions),
        relaunch,
    }
}

fn program_files() -> Option<PathBuf> {
    std::env::var_os("ProgramFiles").map(PathBuf::from)
}
