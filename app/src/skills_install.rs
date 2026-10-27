// SPDX-License-Identifier: AGPL-3.0-only
//! Installs Wenlan's agent skills (`crate::agent_skills_bundle`) into an AI
//! tool's own skills folder, and reports whether they are there.
//!
//! Claude Code and Codex get their skills through the Wenlan plugin; the tools
//! here read a plain skills folder (`<folder>/<skill>/SKILL.md`), so the app
//! writes the bundle there itself. Every skill folder Wenlan writes carries a
//! `.wenlan-managed` marker holding the bundle version, and a same-named folder
//! WITHOUT the marker is the user's: it is never overwritten.
//!
//! Paths come from [`crate::identity_paths::ai_tool_dirs`], the same home and
//! config dir the MCP config writer uses, so an isolated dev run installs into
//! its stand-ins and never into the user's real tool folders.

use crate::mcp_config::Reading;
use std::fmt;
use std::path::{Component, Path, PathBuf};

/// The marker file in each skill folder Wenlan wrote. Holds the bundle version.
pub(crate) const MANAGED_MARKER: &str = ".wenlan-managed";

/// The text in a skill file that stands for the absolute skills folder, so a
/// skill can name its own scripts (`{{WENLAN_SKILLS_DIR}}/wenlan/scripts/..`).
pub(crate) const SKILLS_DIR_PLACEHOLDER: &str = "{{WENLAN_SKILLS_DIR}}";

/// The platform folder a client's skills folder hangs off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkillsBase {
    Home,
    /// The application-configuration folder (`dirs::config_dir()`).
    ConfigDir,
}

/// Where `client_type`'s skills folder is, as a base and a relative tail.
/// `None` for a client whose skills arrive another way (the Claude Code and
/// Codex plugins) or that has no skills surface (Claude Desktop, Muse Code).
///
/// Several clients share a folder: Antigravity reads Gemini CLI's
/// `~/.gemini/skills`, and VS Code reads Copilot's `~/.copilot/skills`.
fn skills_location(client_type: &str) -> Option<(SkillsBase, PathBuf)> {
    let tail: &[&str] = match client_type {
        "cursor" => &[".cursor", "skills"],
        "gemini_cli" | "antigravity" => &[".gemini", "skills"],
        "copilot_cli" | "vscode" => &[".copilot", "skills"],
        "windsurf" => &[".codeium", "windsurf", "skills"],
        "kiro" => &[".kiro", "skills"],
        "opencode" => &[".config", "opencode", "skills"],
        // Kimi Code also reads the shared `~/.agents/skills`; its own folder
        // keeps Wenlan's copy next to the MCP entry it pairs with.
        "kimi_code" => &[".kimi-code", "skills"],
        // Kimi, the desktop app: its daemon hands its embedded Kimi Code one
        // skills folder, `<config dir>/kimi-desktop/daimon-share/daimon/skills`
        // (measured on macOS, Kimi 3.2.21), and reads neither `~/.agents/skills`
        // nor a `skills` folder in its Kimi Code home. The app's own built-in
        // skills live there too, listed in its
        // `.daimon-managed-builtin-skills.json`; Wenlan's carry
        // [`MANAGED_MARKER`] like everywhere else, so neither is mistaken for
        // the other.
        "kimi_desktop" => {
            let tail = crate::mcp_config::KIMI_DESKTOP_DAIMON
                .iter()
                .chain(&["skills"])
                .collect::<PathBuf>();
            return Some((SkillsBase::ConfigDir, tail));
        }
        _ => return None,
    };
    Some((SkillsBase::Home, tail.iter().collect()))
}

/// The skills folder `client_type` reads, under `home` or the platform
/// `config_dir` (whichever that client's folder hangs off). `None` for a
/// client with no skills folder; `Some(Err(what))` when the directory it
/// needs could not be determined, naming that directory.
pub(crate) fn skills_dir_for(
    client_type: &str,
    home: Option<&Path>,
    config_dir: Option<&Path>,
) -> Option<Result<PathBuf, &'static str>> {
    let (base, tail) = skills_location(client_type)?;
    Some(match base {
        SkillsBase::Home => home
            .map(|home| home.join(&tail))
            .ok_or("the home directory"),
        SkillsBase::ConfigDir => config_dir
            .map(|config_dir| config_dir.join(&tail))
            .ok_or("the application-configuration directory"),
    })
}

/// Why an install did not happen. Every variant is raised before anything is
/// written, except `Io`, which names the path that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillsInstallError {
    /// Not a client with a skills folder.
    NoSkillsFolder {
        client_type: String,
    },
    /// The platform would not report the directory the skills folder hangs
    /// off (the home directory, or the application-configuration directory).
    DirUndetermined {
        what: &'static str,
    },
    /// A skill folder of the same name is there and Wenlan did not write it.
    Unmanaged {
        dir: PathBuf,
    },
    /// The skills folder's path cannot be written into a skill file as text.
    UnrepresentablePath {
        dir: PathBuf,
    },
    /// A bundle entry whose path is not a plain relative path.
    BadBundlePath {
        path: String,
    },
    Io {
        path: PathBuf,
        error: String,
    },
}

impl fmt::Display for SkillsInstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkillsInstallError::NoSkillsFolder { client_type } => {
                write!(f, "{client_type} has no skills folder Wenlan installs into")
            }
            SkillsInstallError::DirUndetermined { what } => write!(
                f,
                "Could not determine {what}, so there is no skills folder to install into. \
                 Nothing was written."
            ),
            SkillsInstallError::Unmanaged { dir } => write!(
                f,
                "{} already exists and was not installed by Wenlan, so it was left alone. \
                 Rename or remove it to install Wenlan's skills. Nothing was written.",
                dir.display()
            ),
            SkillsInstallError::UnrepresentablePath { dir } => write!(
                f,
                "{} is not a path Wenlan can write into a skill file as text. Nothing was \
                 written.",
                dir.display()
            ),
            SkillsInstallError::BadBundlePath { path } => write!(
                f,
                "The skills bundle holds a file at {path:?}, which is not a plain relative \
                 path. Nothing was written."
            ),
            SkillsInstallError::Io { path, error } => {
                write!(f, "Could not write {}: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for SkillsInstallError {}

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> SkillsInstallError + '_ {
    move |error| SkillsInstallError::Io {
        path: path.to_path_buf(),
        error: error.to_string(),
    }
}

/// The bundle's top-level skill folder names, in first-seen order, deduplicated.
fn top_level_dirs(files: &[(&str, &str)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (path, _) in files {
        if let Some(first) = path.split('/').next().filter(|first| !first.is_empty()) {
            if !out.iter().any(|seen| seen == first) {
                out.push(first.to_string());
            }
        }
    }
    out
}

/// Whether Wenlan's skills are installed for `client_type`: `Yes` when every
/// skill folder in the bundle carries [`MANAGED_MARKER`], `No` when one is
/// measured absent, `Unreadable` when a look failed or the home directory
/// could not be determined. `None` for a client with no skills folder.
///
/// `exists` is detection's three-valued probe, so a test decides what is on
/// the machine. A marker with an older bundle version still counts as
/// installed; the version is what an update would compare.
pub(crate) fn skills_reading(
    client_type: &str,
    home: Option<&Path>,
    config_dir: Option<&Path>,
    exists: &dyn Fn(&Path) -> Reading,
) -> Option<Reading> {
    skills_reading_for(
        client_type,
        home,
        config_dir,
        exists,
        crate::agent_skills_bundle::FILES,
    )
}

fn skills_reading_for(
    client_type: &str,
    home: Option<&Path>,
    config_dir: Option<&Path>,
    exists: &dyn Fn(&Path) -> Reading,
    files: &[(&str, &str)],
) -> Option<Reading> {
    // A client's `Some`/`None` never depends on whether its directory could
    // be found: that is `Some(Unreadable)`.
    let dir = match skills_dir_for(client_type, home, config_dir)? {
        Ok(dir) => dir,
        Err(what) => {
            return Some(Reading::Unreadable {
                error: format!("could not determine {what}"),
            })
        }
    };
    let skills = top_level_dirs(files);
    if skills.is_empty() {
        return Some(Reading::No);
    }
    Some(
        skills
            .iter()
            .map(|skill| exists(&dir.join(skill).join(MANAGED_MARKER)))
            .fold(Reading::Yes, Reading::and),
    )
}

/// Install the bundle for `client_type` into its skills folder under the
/// AI-tool home or config dir (the dev-state stand-ins in an isolated dev
/// run). Returns the skills folder.
///
/// For a tool that needs an agent file to start a helper on its cheap model
/// (Cursor, Antigravity), also writes `wenlan-helper` there
/// ([`crate::helper_agent`]). That is best effort: it never fails the skills.
pub fn install_skills(client_type: &str) -> Result<PathBuf, SkillsInstallError> {
    let (home, config_dir) = crate::identity_paths::ai_tool_dirs();
    let dir = match skills_dir_for(client_type, home.as_deref(), config_dir.as_deref()) {
        Some(Ok(dir)) => dir,
        Some(Err(what)) => return Err(SkillsInstallError::DirUndetermined { what }),
        None => {
            return Err(SkillsInstallError::NoSkillsFolder {
                client_type: client_type.to_string(),
            })
        }
    };
    install_skills_into(
        &dir,
        crate::agent_skills_bundle::FILES,
        crate::agent_skills_bundle::BUNDLE_VERSION,
    )?;
    if let Some(home) = home.as_deref() {
        match crate::helper_agent::install_helper_agent(client_type, home) {
            Ok(crate::helper_agent::HelperInstall::LeftAlone(path)) => log::info!(
                "[skills] {} is not Wenlan's, so the helper agent was left alone",
                path.display()
            ),
            Ok(_) => {}
            Err(error) => {
                log::warn!("[skills] could not write the {client_type} helper agent: {error}")
            }
        }
    }
    Ok(dir)
}

/// The skills folder as the text that replaces [`SKILLS_DIR_PLACEHOLDER`].
/// Forward slashes on Windows: the resolver script runs under a POSIX shell
/// (Git Bash, WSL), where `C:/Users/..` works and `C:\Users\..` does not.
fn placeholder_text(dir: &Path) -> Result<String, SkillsInstallError> {
    let text = dir
        .to_str()
        .ok_or_else(|| SkillsInstallError::UnrepresentablePath {
            dir: dir.to_path_buf(),
        })?;
    Ok(if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.to_string()
    })
}

/// A bundle path as components under the skills folder, refusing anything
/// that could land outside it (`..`, an absolute path, an empty segment).
fn bundle_relative_path(path: &str) -> Result<PathBuf, SkillsInstallError> {
    let bad = || SkillsInstallError::BadBundlePath {
        path: path.to_string(),
    };
    if path.is_empty() || path.split('/').any(|part| part.is_empty()) {
        return Err(bad());
    }
    let relative = PathBuf::from(path);
    if relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        Ok(relative)
    } else {
        Err(bad())
    }
}

/// Whether the skill folder at `target` may be replaced: absent, or a real
/// folder holding the marker. Anything else is the user's.
fn check_replaceable(target: &Path) -> Result<bool, SkillsInstallError> {
    match std::fs::symlink_metadata(target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(target)(error)),
        // A file or a symlink of the skill's name is not a folder Wenlan wrote.
        Ok(meta) if !meta.is_dir() => Err(SkillsInstallError::Unmanaged {
            dir: target.to_path_buf(),
        }),
        Ok(_) => {
            let marker = target.join(MANAGED_MARKER);
            match std::fs::symlink_metadata(&marker) {
                Ok(meta) if meta.is_file() => Ok(true),
                Ok(_) => Err(SkillsInstallError::Unmanaged {
                    dir: target.to_path_buf(),
                }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Err(SkillsInstallError::Unmanaged {
                        dir: target.to_path_buf(),
                    })
                }
                Err(error) => Err(io_error(&marker)(error)),
            }
        }
    }
}

/// A sibling name under `dir` for staging `skill`, unique to this process and
/// moment, hidden so a tool listing the folder mid-install skips it.
fn sibling(dir: &Path, skill: &str, kind: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    dir.join(format!(
        ".{skill}.wenlan-{kind}-{}-{nanos}",
        std::process::id()
    ))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), SkillsInstallError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(io_error(path))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), SkillsInstallError> {
    Ok(())
}

/// Write one skill's files into `staged` (a fresh folder), with the marker.
fn stage_skill(
    staged: &Path,
    skill: &str,
    files: &[(PathBuf, &str)],
    placeholder: &str,
    version: &str,
) -> Result<(), SkillsInstallError> {
    std::fs::create_dir_all(staged).map_err(io_error(staged))?;
    for (relative, contents) in files {
        // `relative` starts with the skill's own folder name.
        let inside = relative.strip_prefix(skill).unwrap_or(relative);
        let path = staged.join(inside);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error(parent))?;
        }
        std::fs::write(&path, contents.replace(SKILLS_DIR_PLACEHOLDER, placeholder))
            .map_err(io_error(&path))?;
        if path.extension().is_some_and(|ext| ext == "sh") {
            make_executable(&path)?;
        }
    }
    let marker = staged.join(MANAGED_MARKER);
    std::fs::write(&marker, version).map_err(io_error(&marker))?;
    Ok(())
}

/// The installer body with the folder and bundle handed in.
///
/// Order: every check first (paths, then each top-level skill folder — absent
/// or marked as Wenlan's), so a refusal writes nothing. Then every skill is
/// staged in a hidden sibling folder; a failure there removes the staging and
/// leaves the installed skills as they were. Only then is each skill swapped
/// in: the old folder renamed aside, the staged one renamed into place, the old
/// one removed. A rename onto an existing folder is not portable (Windows
/// refuses it), hence the aside step; if the second rename fails the old
/// folder is put back.
///
/// Installs run one at a time. Setup connects several tools at once, and tools
/// that share a folder (VS Code and Copilot, Antigravity and Gemini CLI) would
/// otherwise both see a skill absent and the second rename into place would
/// fail with "Directory not empty".
pub(crate) fn install_skills_into(
    dir: &Path,
    files: &[(&str, &str)],
    version: &str,
) -> Result<(), SkillsInstallError> {
    static INSTALL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _serial = INSTALL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let placeholder = placeholder_text(dir)?;
    let mut by_skill: Vec<(String, Vec<(PathBuf, &str)>)> = Vec::new();
    for (path, contents) in files {
        let relative = bundle_relative_path(path)?;
        // A file at the top level has no skill folder to carry a marker.
        let skill = match path.split_once('/') {
            Some((skill, _)) => skill.to_string(),
            None => {
                return Err(SkillsInstallError::BadBundlePath {
                    path: path.to_string(),
                })
            }
        };
        match by_skill.iter_mut().find(|(name, _)| *name == skill) {
            Some((_, entries)) => entries.push((relative, contents)),
            None => by_skill.push((skill, vec![(relative, contents)])),
        }
    }

    let mut replace = Vec::with_capacity(by_skill.len());
    for (skill, _) in &by_skill {
        replace.push(check_replaceable(&dir.join(skill))?);
    }

    std::fs::create_dir_all(dir).map_err(io_error(dir))?;
    let mut staged: Vec<PathBuf> = Vec::with_capacity(by_skill.len());
    for (skill, entries) in &by_skill {
        let staging = sibling(dir, skill, "new");
        staged.push(staging.clone());
        if let Err(error) = stage_skill(&staging, skill, entries, &placeholder, version) {
            for path in &staged {
                let _ = std::fs::remove_dir_all(path);
            }
            return Err(error);
        }
    }

    for (((skill, _), staging), replace) in by_skill.iter().zip(&staged).zip(replace) {
        let target = dir.join(skill);
        let aside = sibling(dir, skill, "old");
        if replace {
            if let Err(error) = std::fs::rename(&target, &aside) {
                let _ = std::fs::remove_dir_all(staging);
                return Err(io_error(&target)(error));
            }
        }
        if let Err(error) = std::fs::rename(staging, &target) {
            if replace {
                let _ = std::fs::rename(&aside, &target);
            }
            let _ = std::fs::remove_dir_all(staging);
            return Err(io_error(&target)(error));
        }
        if replace {
            if let Err(error) = std::fs::remove_dir_all(&aside) {
                log::warn!(
                    "[skills] installed {} but could not remove the old copy at {}: {error}",
                    target.display(),
                    aside.display()
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE: &[(&str, &str)] = &[
        (
            "wenlan/SKILL.md",
            "---\nname: wenlan\n---\nRun {{WENLAN_SKILLS_DIR}}/wenlan/scripts/resolve-space.sh\n",
        ),
        ("wenlan/references/wiki.md", "# wiki\n"),
        (
            "wenlan/scripts/resolve-space.sh",
            "#!/bin/sh\necho {{WENLAN_SKILLS_DIR}}\n",
        ),
        ("wenlan-brief/SKILL.md", "---\nname: wenlan-brief\n---\n"),
    ];

    fn real_exists(path: &Path) -> Reading {
        match std::fs::metadata(path) {
            Ok(_) => Reading::Yes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Reading::No,
            Err(e) => Reading::Unreadable {
                error: e.to_string(),
            },
        }
    }

    #[test]
    fn skills_dir_for_every_skills_client() {
        let home = PathBuf::from("/fixture/home");
        let config = PathBuf::from("/fixture/config");
        for (client_type, tail) in [
            ("cursor", [".cursor", "skills"].as_slice()),
            ("gemini_cli", &[".gemini", "skills"]),
            ("antigravity", &[".gemini", "skills"]),
            ("copilot_cli", &[".copilot", "skills"]),
            ("vscode", &[".copilot", "skills"]),
            ("windsurf", &[".codeium", "windsurf", "skills"]),
            ("kiro", &[".kiro", "skills"]),
            ("opencode", &[".config", "opencode", "skills"]),
            ("kimi_code", &[".kimi-code", "skills"]),
        ] {
            let expected = tail.iter().fold(home.clone(), |acc, part| acc.join(part));
            assert_eq!(
                skills_dir_for(client_type, Some(&home), Some(&config)),
                Some(Ok(expected)),
                "{client_type}"
            );
            // A home-based folder never needs the config dir.
            assert!(
                matches!(skills_dir_for(client_type, Some(&home), None), Some(Ok(_))),
                "{client_type}"
            );
            assert_eq!(
                skills_dir_for(client_type, None, Some(&config)),
                Some(Err("the home directory")),
                "{client_type}"
            );
        }
        // Kimi (the desktop app) is under the config dir, and only that.
        let kimi = ["kimi-desktop", "daimon-share", "daimon", "skills"]
            .iter()
            .fold(config.clone(), |acc, part| acc.join(part));
        assert_eq!(
            skills_dir_for("kimi_desktop", Some(&home), Some(&config)),
            Some(Ok(kimi.clone()))
        );
        assert_eq!(
            skills_dir_for("kimi_desktop", None, Some(&config)),
            Some(Ok(kimi))
        );
        assert_eq!(
            skills_dir_for("kimi_desktop", Some(&home), None),
            Some(Err("the application-configuration directory"))
        );
        for client_type in [
            "claude_code",
            "codex_cli",
            "claude_desktop",
            "muse_code",
            "unknown",
        ] {
            assert_eq!(
                skills_dir_for(client_type, Some(&home), Some(&config)),
                None,
                "{client_type}"
            );
            assert_eq!(
                skills_dir_for(client_type, None, None),
                None,
                "{client_type}"
            );
        }
    }

    #[test]
    fn install_writes_every_file_replaces_the_placeholder_and_marks_each_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".cursor").join("skills");
        install_skills_into(&dir, BUNDLE, "v1").unwrap();

        let dir_text = dir.to_str().unwrap();
        let skill = std::fs::read_to_string(dir.join("wenlan").join("SKILL.md")).unwrap();
        assert!(!skill.contains(SKILLS_DIR_PLACEHOLDER), "{skill}");
        assert!(
            skill.contains(&format!("{dir_text}/wenlan/scripts/resolve-space.sh")),
            "{skill}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("wenlan").join("references").join("wiki.md")).unwrap(),
            "# wiki\n"
        );
        let script = dir.join("wenlan").join("scripts").join("resolve-space.sh");
        assert_eq!(
            std::fs::read_to_string(&script).unwrap(),
            format!("#!/bin/sh\necho {dir_text}\n")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&script).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        for skill in ["wenlan", "wenlan-brief"] {
            assert_eq!(
                std::fs::read_to_string(dir.join(skill).join(MANAGED_MARKER)).unwrap(),
                "v1",
                "{skill}"
            );
        }
        // No staging or aside folders are left behind.
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["wenlan", "wenlan-brief"]);
    }

    #[test]
    fn an_unmanaged_skill_folder_of_the_same_name_is_refused_and_nothing_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skills");
        let users = dir.join("wenlan-brief");
        std::fs::create_dir_all(&users).unwrap();
        std::fs::write(users.join("SKILL.md"), "mine").unwrap();

        let error = install_skills_into(&dir, BUNDLE, "v1").unwrap_err();
        assert_eq!(error, SkillsInstallError::Unmanaged { dir: users.clone() });
        assert!(error.to_string().contains("Nothing was written"), "{error}");
        assert_eq!(
            std::fs::read_to_string(users.join("SKILL.md")).unwrap(),
            "mine"
        );
        // `wenlan` comes first in the bundle and was still not written.
        assert!(!dir.join("wenlan").exists());

        // A plain file of a skill's name is the user's too.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skills");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("wenlan"), "a file").unwrap();
        assert!(matches!(
            install_skills_into(&dir, BUNDLE, "v1"),
            Err(SkillsInstallError::Unmanaged { .. })
        ));
        assert!(!dir.join("wenlan-brief").exists());
    }

    #[test]
    fn reinstall_replaces_a_managed_folder_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skills");
        install_skills_into(&dir, BUNDLE, "v1").unwrap();
        // A file the old bundle had and the new one does not, and a file the
        // user dropped into a managed folder: both go with the old version.
        std::fs::write(dir.join("wenlan").join("stale.md"), "old").unwrap();

        let next: &[(&str, &str)] = &[
            ("wenlan/SKILL.md", "---\nname: wenlan\n---\nv2\n"),
            (
                "wenlan-brief/SKILL.md",
                "---\nname: wenlan-brief\n---\nv2\n",
            ),
        ];
        install_skills_into(&dir, next, "v2").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("wenlan").join("SKILL.md")).unwrap(),
            "---\nname: wenlan\n---\nv2\n"
        );
        assert!(!dir.join("wenlan").join("stale.md").exists());
        assert!(!dir.join("wenlan").join("references").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("wenlan").join(MANAGED_MARKER)).unwrap(),
            "v2"
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    }

    #[test]
    fn other_skills_in_a_shared_folder_are_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skills");
        let other = dir.join("someone-elses-skill");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("SKILL.md"), "theirs").unwrap();
        install_skills_into(&dir, BUNDLE, "v1").unwrap();
        assert_eq!(
            std::fs::read_to_string(other.join("SKILL.md")).unwrap(),
            "theirs"
        );
    }

    #[test]
    fn a_bundle_path_that_could_leave_the_folder_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skills");
        for path in [
            "../escape/SKILL.md",
            "/abs/SKILL.md",
            "wenlan//x",
            "loose.md",
            "",
        ] {
            let files: &[(&str, &str)] = &[(path, "x")];
            assert!(
                matches!(
                    install_skills_into(&dir, files, "v1"),
                    Err(SkillsInstallError::BadBundlePath { .. })
                ),
                "{path:?}"
            );
        }
        assert!(!dir.exists(), "a refused bundle writes nothing");
    }

    #[test]
    fn has_skills_is_yes_only_when_every_skill_is_marked() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let reading =
            |client: &str| skills_reading_for(client, Some(home), Some(home), &real_exists, BUNDLE);

        assert_eq!(reading("kiro"), Some(Reading::No));
        let dir = skills_dir_for("kiro", Some(home), None).unwrap().unwrap();
        install_skills_into(&dir, BUNDLE, "v1").unwrap();
        assert_eq!(reading("kiro"), Some(Reading::Yes));

        // One skill without its marker is not installed.
        std::fs::remove_file(dir.join("wenlan-brief").join(MANAGED_MARKER)).unwrap();
        assert_eq!(reading("kiro"), Some(Reading::No));

        // Shared folders answer for both clients that read them.
        let copilot = skills_dir_for("copilot_cli", Some(home), None)
            .unwrap()
            .unwrap();
        install_skills_into(&copilot, BUNDLE, "v1").unwrap();
        assert_eq!(reading("copilot_cli"), Some(Reading::Yes));
        assert_eq!(reading("vscode"), Some(Reading::Yes));

        // Kimi's folder is its own, under the config dir.
        assert_eq!(reading("kimi_desktop"), Some(Reading::No));
        let kimi = skills_dir_for("kimi_desktop", None, Some(home))
            .unwrap()
            .unwrap();
        install_skills_into(&kimi, BUNDLE, "v1").unwrap();
        assert_eq!(reading("kimi_desktop"), Some(Reading::Yes));
        assert_eq!(reading("kimi_code"), Some(Reading::No));

        // Clients with no skills folder have no reading at all.
        assert_eq!(reading("claude_code"), None);
        assert_eq!(reading("muse_code"), None);
    }

    #[test]
    fn has_skills_never_claims_no_from_a_failed_look() {
        let home = PathBuf::from("/fixture/home");
        let refused = |_: &Path| Reading::Unreadable {
            error: "Access is denied. (os error 5)".to_string(),
        };
        assert!(matches!(
            skills_reading_for("cursor", Some(&home), None, &refused, BUNDLE),
            Some(Reading::Unreadable { .. })
        ));
        // A measured absence beside an unread one is a measured no: "every
        // skill is installed" is false either way.
        let brief_marker = home
            .join(".cursor")
            .join("skills")
            .join("wenlan-brief")
            .join(MANAGED_MARKER);
        let mixed = move |p: &Path| {
            if p == brief_marker {
                Reading::No
            } else {
                Reading::Unreadable {
                    error: "denied".to_string(),
                }
            }
        };
        assert_eq!(
            skills_reading_for("cursor", Some(&home), None, &mixed, BUNDLE),
            Some(Reading::No)
        );
        // No home: unknown, but still a skills client.
        assert!(matches!(
            skills_reading_for("cursor", None, Some(&home), &refused, BUNDLE),
            Some(Reading::Unreadable { .. })
        ));
        // No config dir: unknown for Kimi, whose folder hangs off it.
        assert!(matches!(
            skills_reading_for("kimi_desktop", Some(&home), None, &real_exists, BUNDLE),
            Some(Reading::Unreadable { .. })
        ));
        assert_eq!(
            skills_reading_for("claude_desktop", None, None, &refused, BUNDLE),
            None
        );
    }

    /// Tools that share a skills folder are set up at the same moment; both
    /// installs must succeed rather than one failing on "Directory not empty".
    #[test]
    fn concurrent_installs_into_a_shared_folder_both_succeed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".copilot").join("skills");
        for _ in 0..20 {
            let _ = std::fs::remove_dir_all(&dir);
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let dir = dir.clone();
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        install_skills_into(&dir, BUNDLE, "v1")
                    })
                })
                .collect();
            for handle in handles {
                assert_eq!(handle.join().unwrap(), Ok(()));
            }
            for (path, _) in BUNDLE {
                assert!(dir.join(path).is_file(), "{path}");
            }
        }
    }

    /// The real entry point honors the dev-state redirect, like the MCP writer.
    #[test]
    #[serial_test::serial]
    fn install_skills_lands_under_the_dev_state_home() {
        let _env = crate::test_env::EnvGuard::capture(&["WENLAN_DEV_STATE_DIR"]);
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("WENLAN_DEV_STATE_DIR", tmp.path());
        let dir = install_skills("opencode").unwrap();
        let expected = tmp
            .path()
            .join("ai-tools")
            .join("home")
            .join(".config")
            .join("opencode")
            .join("skills");
        assert_eq!(dir, expected);
        for (path, _) in crate::agent_skills_bundle::FILES {
            assert!(dir.join(path).is_file(), "{path}");
        }
        // OpenCode gets no helper agent; Cursor gets one next to its skills.
        assert!(!tmp
            .path()
            .join("ai-tools")
            .join("home")
            .join(".cursor")
            .exists());
        install_skills("cursor").unwrap();
        let helper = tmp
            .path()
            .join("ai-tools")
            .join("home")
            .join(".cursor")
            .join("agents")
            .join("wenlan-helper.md");
        assert_eq!(
            std::fs::read_to_string(&helper).unwrap(),
            crate::helper_agent::helper_agent_text("cursor").unwrap()
        );
        // Kimi (the desktop app) lands under the dev-state config dir.
        let kimi = install_skills("kimi_desktop").unwrap();
        assert_eq!(
            kimi,
            tmp.path()
                .join("ai-tools")
                .join("config")
                .join("kimi-desktop")
                .join("daimon-share")
                .join("daimon")
                .join("skills")
        );
        assert!(kimi.join("wenlan").join("SKILL.md").is_file());
        assert_eq!(
            install_skills("claude_desktop"),
            Err(SkillsInstallError::NoSkillsFolder {
                client_type: "claude_desktop".to_string()
            })
        );
    }
}
