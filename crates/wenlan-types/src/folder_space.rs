// SPDX-License-Identifier: Apache-2.0
//! What a working folder says about the Space, before the registry is asked.
//!
//! The terminal, the plugin and the stdio MCP server walk the same chain
//! (`plugin/skills/README.md`, "Choosing the active space"): a
//! `~/.wenlan/spaces.toml` folder mapping first, then a Space named like the
//! git repository's folder. This module holds the folder half, so every door
//! reads it the same way; whether a name is a registered Space is the
//! caller's check.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The Space names a folder points at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderSpaceHint {
    /// The Space of the longest `~/.wenlan/spaces.toml` prefix that holds the
    /// folder.
    pub mapped: Option<String>,
    /// The folder name of the git repository the folder is in. A linked
    /// worktree answers with its main repository's name.
    pub repository: Option<String>,
}

impl FolderSpaceHint {
    /// The Space this folder uses, given the registered Space names: the
    /// mapping when it names a registered Space, else the repository's name
    /// when a Space is called that.
    pub fn registered(&self, is_registered: impl Fn(&str) -> bool) -> Option<&str> {
        [self.mapped.as_deref(), self.repository.as_deref()]
            .into_iter()
            .flatten()
            .find(|name| is_registered(name))
    }
}

/// Read both hints for `cwd`. `home` expands `~` in mapping prefixes.
pub fn folder_space_hint(
    cwd: &Path,
    spaces_file: Option<&Path>,
    home: Option<&Path>,
) -> FolderSpaceHint {
    FolderSpaceHint {
        mapped: spaces_file.and_then(|file| mapped_space(cwd, file, home)),
        repository: repository_folder_name(cwd),
    }
}

/// The Space of the longest `[[mapping]]` prefix in `config` that holds
/// `cwd`. Equal-length prefixes go to the first one defined.
pub fn mapped_space(cwd: &Path, config: &Path, home: Option<&Path>) -> Option<String> {
    let body = std::fs::read_to_string(config).ok()?;
    let mut mappings: Vec<(String, String)> = Vec::new();
    let mut prefix: Option<String> = None;
    let mut space: Option<String> = None;

    let mut finish = |prefix: &mut Option<String>, space: &mut Option<String>| {
        if let (Some(prefix), Some(space)) = (prefix.take(), space.take()) {
            mappings.push((prefix, space));
        }
    };
    for line in body.lines() {
        let line = line.trim();
        if line == "[[mapping]]" {
            finish(&mut prefix, &mut space);
        } else if let Some(value) = parse_toml_string(line, "prefix") {
            prefix = Some(value);
        } else if let Some(value) = parse_toml_string(line, "space") {
            space = Some(value);
        }
    }
    finish(&mut prefix, &mut space);

    mappings
        .into_iter()
        // `.rev()` makes `max_by_key` below (which returns the LAST maximum)
        // return the FIRST-defined mapping among equal-length prefix matches,
        // matching the documented tie-break (plugin/skills/README.md,
        // plugin/examples/spaces.toml) and resolve-space.sh's strict `>` check.
        .rev()
        .filter_map(|(prefix, space)| {
            let expanded = if prefix == "~" {
                home.map(Path::to_path_buf)
            } else if let Some(suffix) = prefix.strip_prefix("~/") {
                home.map(|home| home.join(suffix))
            } else {
                Some(PathBuf::from(prefix))
            }?;
            cwd.starts_with(&expanded)
                .then_some((expanded.as_os_str().len(), space))
        })
        .max_by_key(|(length, _)| *length)
        .map(|(_, space)| space)
}

fn parse_toml_string(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?.trim_start();
    let rest = rest.strip_prefix('=')?.trim();
    let value = rest.strip_prefix('"')?.strip_suffix('"')?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// The folder name of the git repository `cwd` is in.
pub fn repository_folder_name(cwd: &Path) -> Option<String> {
    let common = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !common.status.success() {
        return None;
    }
    let common = PathBuf::from(String::from_utf8(common.stdout).ok()?.trim());
    if common.file_name().and_then(|name| name.to_str()) == Some(".git") {
        return common
            .parent()?
            .file_name()?
            .to_str()
            .map(ToOwned::to_owned);
    }

    let root = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !root.status.success() {
        return None;
    }
    PathBuf::from(String::from_utf8(root.stdout).ok()?.trim())
        .file_name()?
        .to_str()
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(mapped: Option<&str>, repository: Option<&str>) -> FolderSpaceHint {
        FolderSpaceHint {
            mapped: mapped.map(str::to_string),
            repository: repository.map(str::to_string),
        }
    }

    #[test]
    fn a_registered_mapping_wins_over_the_repository_name() {
        let both = hint(Some("mapped"), Some("repo"));
        assert_eq!(both.registered(|_| true), Some("mapped"));
        assert_eq!(both.registered(|name| name == "repo"), Some("repo"));
        assert_eq!(both.registered(|_| false), None);
    }

    #[test]
    fn a_folder_outside_any_repository_and_mapping_names_no_space() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("spaces.toml");
        std::fs::write(
            &config,
            "[[mapping]]\nprefix = \"/nowhere/else\"\nspace = \"x\"\n",
        )
        .unwrap();
        assert_eq!(mapped_space(tmp.path(), &config, None), None);
    }

    #[test]
    fn a_tilde_prefix_is_read_against_the_home_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work/app");
        std::fs::create_dir_all(&work).unwrap();
        let config = tmp.path().join("spaces.toml");
        std::fs::write(
            &config,
            "[[mapping]]\nprefix = \"~/work\"\nspace = \"job\"\n",
        )
        .unwrap();
        assert_eq!(
            mapped_space(&work, &config, Some(tmp.path())).as_deref(),
            Some("job")
        );
        assert_eq!(mapped_space(&work, &config, None), None);
    }
}
