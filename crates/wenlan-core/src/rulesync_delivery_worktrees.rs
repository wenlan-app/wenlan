// SPDX-License-Identifier: Apache-2.0
//! Read-only discovery of Git worktrees containing personal delivery paths.

use wenlan_types::rulesync::{RulesSetupCode as Code, RulesSetupNotice};

fn worktree_notice(code: Code, tool: &str) -> RulesSetupNotice {
    super::personal_target(tool).map_or_else(
        || code.into(),
        |target| RulesSetupNotice::for_tool(code, target.tool.display_name()),
    )
}

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::{ensure, Context, Result};

use crate::rulesync_adopt::{git_repository_name, Adoption, AdoptionTarget, SHARED_SKILLS};

#[derive(Clone, Default)]
struct Location {
    root: Option<PathBuf>,
    // Never probe paths behind a symlink in the destination. The applier
    // either refuses that path or operates on the link itself at adoption.
    beneath_link: bool,
}

pub struct DeliveryWorktrees {
    home: PathBuf,
    directories: BTreeMap<PathBuf, Location>,
}

impl DeliveryWorktrees {
    pub fn new(home: &Path) -> Result<Self> {
        Ok(Self {
            home: home.canonicalize().context("resolve delivery home")?,
            directories: BTreeMap::new(),
        })
    }

    /// The nearest repository that could receive writes to a file or folder.
    /// A directory containing `.git` also counts before its first commit.
    pub fn at(&mut self, destination: &Path) -> Result<Option<PathBuf>> {
        ensure!(
            !destination.as_os_str().is_empty()
                && destination
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "expected a home-relative delivery path"
        );
        Ok(self.location(&self.home.join(destination))?.root)
    }

    pub fn display(&self, path: &Path) -> String {
        path.strip_prefix(&self.home).map_or_else(
            |_| path.display().to_string(),
            |path| {
                if path.as_os_str().is_empty() {
                    "~".into()
                } else {
                    format!("~/{}", path.display())
                }
            },
        )
    }

    pub fn unreviewed(
        &mut self,
        destination: &Path,
        reviewed: &BTreeMap<PathBuf, PathBuf>,
    ) -> Result<Option<String>> {
        let Some(root) = self.at(destination)? else {
            return Ok(None);
        };
        if destination
            .ancestors()
            .any(|path| reviewed.get(path) == Some(&root))
        {
            return Ok(None);
        }
        Ok(Some(format!(
            "This destination is inside the Git worktree at {}. Choose a tool folder outside that repository to sync it without changing the checkout.",
            self.display(&root)
        )))
    }

    fn location(&mut self, path: &Path) -> Result<Location> {
        if let Some(found) = self.directories.get(path) {
            return Ok(found.clone());
        }
        let mut location = path
            .parent()
            .map_or(Ok(Location::default()), |parent| self.location(parent))?;
        if !location.beneath_link {
            match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_dir() => {
                    let marker = path.join(".git");
                    match fs::symlink_metadata(&marker) {
                        Ok(_) => location.root = Some(path.to_owned()),
                        Err(error) if error.kind() == ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(error).with_context(|| {
                                format!("check {} for a Git worktree", path.display())
                            })
                        }
                    }
                }
                Ok(metadata) => location.beneath_link = metadata.is_symlink(),
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("check {} for a Git worktree", path.display()))
                }
            }
        }
        self.directories.insert(path.to_owned(), location.clone());
        Ok(location)
    }
}

pub(super) fn review_destinations(
    home: &Path,
    adoption: &mut Adoption,
    targets: &[AdoptionTarget],
) -> Result<()> {
    let mut worktrees = DeliveryWorktrees::new(home)?;
    for path in adoption.files.keys() {
        if let Some(root) = worktrees.at(path)? {
            adoption.worktree_writes.insert(path.clone(), root);
        }
    }

    let mut names = BTreeMap::new();
    let mut repository = |root: &Path| {
        names
            .entry(root.to_owned())
            .or_insert_with(|| git_repository_name(root).unwrap_or_else(|| worktrees.display(root)))
            .clone()
    };
    for target in targets {
        if let Some(path) = &target.rules_path {
            if let Some(root) = adoption.worktree_writes.get(path) {
                let name = repository(root);
                adoption
                    .worktree_note_codes
                    .push(worktree_notice(Code::GitRule, &target.tool));
                adoption.worktree_notes.push(format!(
                    "~/{} is inside the Git repository {name}. Wenlan will manage this file there, and its writes can appear as changes in that repository.",
                    path.display()
                ));
            }
        }
        let Some(scope) = target.skills_path.as_deref() else {
            continue;
        };
        let mut counts: BTreeMap<&Path, BTreeSet<&std::ffi::OsStr>> = BTreeMap::new();
        for (path, root) in &adoption.worktree_writes {
            if let Some(folder) = path
                .strip_prefix(scope)
                .ok()
                .and_then(|relative| relative.iter().next())
            {
                counts.entry(root).or_default().insert(folder);
            }
        }
        for (root, folders) in counts {
            let name = repository(root);
            let count = folders.len();
            let location = if let Ok(relative) = root.strip_prefix(worktrees.home.join(scope)) {
                format!("~/{}", scope.join(relative).display())
            } else {
                format!("~/{}", scope.display())
            };
            let noun = if count == 1 { "folder" } else { "folders" };
            adoption
                .worktree_note_codes
                .push(worktree_notice(Code::GitSkills, &target.tool));
            adoption.worktree_notes.push(format!(
                "{location} is inside the Git repository {name}. Wenlan will manage {count} skill {noun} there, and its writes can appear as changes in that repository."
            ));
        }
    }
    for path in adoption.personal_files.keys() {
        if let Some(root) = adoption.worktree_writes.get(path) {
            let name = repository(root);
            adoption.worktree_note_codes.push(
                crate::rulesync_personal_files::location_of(path).map_or_else(
                    || Code::GitPersonalFile.into(),
                    |location| worktree_notice(Code::GitPersonalFile, location.tool),
                ),
            );
            adoption.worktree_notes.push(format!(
                "~/{} is inside the Git repository {name}. Delivery can change this checkout.",
                path.display()
            ));
        }
    }
    let shared = Path::new(SHARED_SKILLS);
    if adoption.skipped.keys().any(|path| path.starts_with(shared)) {
        if let Some(root) = worktrees.at(shared)? {
            let name = git_repository_name(&root).unwrap_or_else(|| worktrees.display(&root));
            adoption
                .worktree_note_codes
                .push(Code::GitSharedSkills.into());
            adoption.worktree_notes.push(format!(
                "~/{SHARED_SKILLS} is inside the Git repository {name}. Wenlan reads the shared skills there and leaves that directory in place."
            ));
        }
    }
    Ok(())
}
