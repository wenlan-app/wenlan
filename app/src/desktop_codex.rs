//! Locate the CLI shipped with the desktop app; never launch the GUI executable.
use std::path::{Path, PathBuf};

/// MSIX registration is per-user and points at the installed version. Do not
/// scan WindowsApps (normally unreadable), guess a version, or run PowerShell.
#[cfg(windows)]
fn registered_package_roots(prefix: &str, publisher: &str) -> Vec<PathBuf> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let Ok(packages) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(
        r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages",
    ) else {
        return Vec::new();
    };
    packages
        .enum_keys()
        .filter_map(Result::ok)
        .filter(|name| name.starts_with(prefix) && name.ends_with(publisher))
        .filter_map(|name| packages.open_subkey(name).ok())
        .filter_map(|key| key.get_value::<String, _>("PackageRootFolder").ok())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect()
}

#[cfg(not(windows))]
fn registered_package_roots(_prefix: &str, _publisher: &str) -> Vec<PathBuf> {
    Vec::new()
}

pub(crate) fn registered_windows_roots() -> Vec<PathBuf> {
    registered_package_roots("OpenAI.Codex_", "__2p2nqsd0c76g0")
}

/// The Windows desktop runtime supplies Git too. A new machine need not have
/// a standalone installation. Append it only to this CLI's child environment.
pub(crate) fn path_with_bundled_git(
    inherited: Option<std::ffi::OsString>,
) -> Option<std::ffi::OsString> {
    let roots = registered_package_roots("OpenAI.CodexPrimaryRuntime.", "__3k8sg7r9htsxt");
    append_bundled_git(inherited, &roots, |path| path.is_file())
}

fn append_bundled_git(
    inherited: Option<std::ffi::OsString>,
    roots: &[PathBuf],
    exists: impl Fn(&Path) -> bool,
) -> Option<std::ffi::OsString> {
    let mut paths = inherited
        .as_ref()
        .map(|p| std::env::split_paths(p).collect::<Vec<_>>())
        .unwrap_or_default();
    let original_len = paths.len();
    for root in roots {
        let dir = root.join("dependencies/native/git/cmd");
        if exists(&dir.join("git.exe")) && !paths.contains(&dir) {
            paths.push(dir);
        }
    }
    if paths.len() == original_len {
        return inherited;
    }
    std::env::join_paths(paths).ok().or(inherited)
}

pub(crate) fn candidates(
    windows: bool,
    macos: bool,
    home: Option<&Path>,
    local_appdata: Option<&Path>,
    registered_roots: &[PathBuf],
) -> Vec<PathBuf> {
    if windows {
        let mut paths: Vec<_> = registered_roots
            .iter()
            .map(|root| root.join("app/resources/codex.exe"))
            .collect();
        if let Some(local) = local_appdata {
            let app = local.join("Programs/OpenAI/Codex");
            paths.push(app.join("resources/codex.exe"));
            paths.push(app.join("bin/codex.exe"));
        }
        return paths;
    }
    if macos {
        let mut apps = vec![PathBuf::from("/Applications/ChatGPT.app")];
        if let Some(home) = home {
            apps.push(home.join("Applications/ChatGPT.app"));
        }
        return apps
            .into_iter()
            .map(|app| app.join("Contents/Resources/codex-cli/bin/codex"))
            .collect();
    }
    // Linux uses the existing PATH and version-manager resolution. There is
    // no supported desktop bundle to invent a location for.
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_only_windows_finds_the_cli_with_no_path_or_admin_scan() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("OpenAI.Codex_26_x64__2p2nqsd0c76g0");
        let cli = root.join("app/resources/codex.exe");
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(&cli, b"fixture").unwrap();
        let paths = candidates(true, false, None, None, std::slice::from_ref(&root));
        assert_eq!(paths.iter().find(|p| p.is_file()), Some(&cli));
        assert!(!paths.contains(&root.join("app/Codex.exe")));
        std::fs::remove_file(&cli).unwrap();
        assert!(paths.iter().all(|p| !p.is_file()));
    }

    #[test]
    fn bundled_git_is_child_local_and_preserves_existing_path_priority() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("runtime");
        let git_dir = root.join("dependencies/native/git/cmd");
        let cli_dir = dir.path().join("existing-tools");
        let inherited = std::env::join_paths([&cli_dir]).unwrap();
        let roots = std::slice::from_ref(&root);
        let added = append_bundled_git(Some(inherited.clone()), roots, |p| {
            p == git_dir.join("git.exe")
        })
        .unwrap();
        assert_eq!(
            std::env::split_paths(&added).collect::<Vec<_>>(),
            vec![cli_dir, git_dir.clone()]
        );
        assert_eq!(
            append_bundled_git(Some(inherited.clone()), roots, |_| false),
            Some(inherited)
        );
        assert_eq!(
            append_bundled_git(None, roots, |_| true),
            Some(git_dir.into_os_string())
        );
        assert!(append_bundled_git(None, &[], |_| true).is_none());
    }

    #[test]
    fn platform_candidates_keep_mac_windows_and_linux_separate() {
        let home = Path::new("/users/u");
        let local = home.join("AppData/Local");
        let mac = candidates(false, true, Some(home), Some(&local), &[]);
        assert_eq!(mac.len(), 2);
        assert!(mac[1].ends_with("Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex"));
        let windows = candidates(true, false, Some(home), Some(&local), &[]);
        assert_eq!(
            windows,
            vec![
                local.join("Programs/OpenAI/Codex/resources/codex.exe"),
                local.join("Programs/OpenAI/Codex/bin/codex.exe")
            ]
        );
        assert!(candidates(false, false, Some(home), Some(&local), &[]).is_empty());
        assert!(candidates(true, false, None, None, &[]).is_empty());
    }
}
