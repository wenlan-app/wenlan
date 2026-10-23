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
//! Paths come from [`crate::identity_paths::ai_tool_dirs`], the same home the
//! MCP config writer uses, so an isolated dev run installs into its stand-in
//! home and never into the user's real tool folders.

use crate::mcp_config::Reading;
use std::fmt;
use std::path::{Component, Path, PathBuf};

/// The marker file in each skill folder Wenlan wrote. Holds the bundle version.
pub(crate) const MANAGED_MARKER: &str = ".wenlan-managed";

/// The text in a skill file that stands for the absolute skills folder, so a
/// skill can name its own scripts (`{{WENLAN_SKILLS_DIR}}/wenlan/scripts/..`).
pub(crate) const SKILLS_DIR_PLACEHOLDER: &str = "{{WENLAN_SKILLS_DIR}}";

/// The skills folder `client_type` reads, under `home`. `None` for a client
/// whose skills arrive another way (the Claude Code and Codex plugins) or that
/// has no skills surface (Claude Desktop, Muse Code).
///
/// Several clients share a folder: Antigravity reads Gemini CLI's
/// `~/.gemini/skills`, and VS Code reads Copilot's `~/.copilot/skills`.
pub(crate) fn skills_dir_for(client_type: &str, home: &Path) -> Option<PathBuf> {
    let tail: &[&str] = match client_type {
        "cursor" => &[".cursor", "skills"],
        "gemini_cli" | "antigravity" => &[".gemini", "skills"],
        "copilot_cli" | "vscode" => &[".copilot", "skills"],
        "windsurf" => &[".codeium", "windsurf", "skills"],
        "kiro" => &[".kiro", "skills"],
        "opencode" => &[".config", "opencode", "skills"],
        _ => return None,
    };
    Some(
        tail.iter()
            .fold(home.to_path_buf(), |acc, part| acc.join(part)),
    )
}

/// Why an install did not happen. Every variant is raised before anything is
/// written, except `Io`, which names the path that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillsInstallError {
    /// Not a client with a skills folder.
    NoSkillsFolder {
        client_type: String,
    },
    /// The platform would not report a home directory.
    HomeUndetermined,
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
            SkillsInstallError::HomeUndetermined => write!(
                f,
                "Could not determine the home directory, so there is no skills folder to \
                 install into. Nothing was written."
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
    exists: &dyn Fn(&Path) -> Reading,
) -> Option<Reading> {
    skills_reading_for(client_type, home, exists, crate::agent_skills_bundle::FILES)
}

fn skills_reading_for(
    client_type: &str,
    home: Option<&Path>,
    exists: &dyn Fn(&Path) -> Reading,
    files: &[(&str, &str)],
) -> Option<Reading> {
    // Answer "has a skills folder" from a stand-in home first, so a client's
    // `Some`/`None` never depends on whether the home could be found.
    skills_dir_for(client_type, Path::new(""))?;
    let Some(home) = home else {
        return Some(Reading::Unreadable {
            error: "could not determine the home directory".to_string(),
        });
    };
    let dir = skills_dir_for(client_type, home)?;
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
/// AI-tool home (the dev-state stand-in in an isolated dev run). Returns the
/// skills folder.
pub fn install_skills(client_type: &str) -> Result<PathBuf, SkillsInstallError> {
    let (home, _) = crate::identity_paths::ai_tool_dirs();
    let Some(home) = home else {
        // Not-a-skills-client first, so that error is not masked by this one.
        return Err(match skills_dir_for(client_type, Path::new("")) {
            Some(_) => SkillsInstallError::HomeUndetermined,
            None => SkillsInstallError::NoSkillsFolder {
                client_type: client_type.to_string(),
            },
        });
    };
    let dir =
        skills_dir_for(client_type, &home).ok_or_else(|| SkillsInstallError::NoSkillsFolder {
            client_type: client_type.to_string(),
        })?;
    install_skills_into(
        &dir,
        crate::agent_skills_bundle::FILES,
        crate::agent_skills_bundle::BUNDLE_VERSION,
    )?;
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
pub(crate) fn install_skills_into(
    dir: &Path,
    files: &[(&str, &str)],
    version: &str,
) -> Result<(), SkillsInstallError> {
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
        for (client_type, tail) in [
            ("cursor", [".cursor", "skills"].as_slice()),
            ("gemini_cli", &[".gemini", "skills"]),
            ("antigravity", &[".gemini", "skills"]),
            ("copilot_cli", &[".copilot", "skills"]),
            ("vscode", &[".copilot", "skills"]),
            ("windsurf", &[".codeium", "windsurf", "skills"]),
            ("kiro", &[".kiro", "skills"]),
            ("opencode", &[".config", "opencode", "skills"]),
        ] {
            let expected = tail.iter().fold(home.clone(), |acc, part| acc.join(part));
            assert_eq!(
                skills_dir_for(client_type, &home),
                Some(expected),
                "{client_type}"
            );
        }
        for client_type in [
            "claude_code",
            "codex_cli",
            "claude_desktop",
            "muse_code",
            "unknown",
        ] {
            assert_eq!(skills_dir_for(client_type, &home), None, "{client_type}");
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
        let reading = |client: &str| skills_reading_for(client, Some(home), &real_exists, BUNDLE);

        assert_eq!(reading("kiro"), Some(Reading::No));
        let dir = skills_dir_for("kiro", home).unwrap();
        install_skills_into(&dir, BUNDLE, "v1").unwrap();
        assert_eq!(reading("kiro"), Some(Reading::Yes));

        // One skill without its marker is not installed.
        std::fs::remove_file(dir.join("wenlan-brief").join(MANAGED_MARKER)).unwrap();
        assert_eq!(reading("kiro"), Some(Reading::No));

        // Shared folders answer for both clients that read them.
        let copilot = skills_dir_for("copilot_cli", home).unwrap();
        install_skills_into(&copilot, BUNDLE, "v1").unwrap();
        assert_eq!(reading("copilot_cli"), Some(Reading::Yes));
        assert_eq!(reading("vscode"), Some(Reading::Yes));

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
            skills_reading_for("cursor", Some(&home), &refused, BUNDLE),
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
            skills_reading_for("cursor", Some(&home), &mixed, BUNDLE),
            Some(Reading::No)
        );
        // No home: unknown, but still a skills client.
        assert!(matches!(
            skills_reading_for("cursor", None, &refused, BUNDLE),
            Some(Reading::Unreadable { .. })
        ));
        assert_eq!(
            skills_reading_for("claude_desktop", None, &refused, BUNDLE),
            None
        );
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
        assert_eq!(
            install_skills("claude_desktop"),
            Err(SkillsInstallError::NoSkillsFolder {
                client_type: "claude_desktop".to_string()
            })
        );
    }
}
