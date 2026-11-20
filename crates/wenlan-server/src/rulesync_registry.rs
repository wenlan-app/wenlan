// SPDX-License-Identifier: Apache-2.0
//! Feed confirmed registry sources to the personal tool-file applier.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use wenlan_core::{rulesync_personal, skill_registry as registry};
use wenlan_types::{rulesync::RulesSyncReport, skill_registry::LocalSkill};

#[path = "rulesync_skill_routes.rs"]
mod saved_skills;
pub(crate) use saved_skills::{
    approved_local_folders, approved_skill_sources, authored_rows, collect_tool_proposals,
    register, remove_saved_source, ManagedSkill,
};

// The registry swaps source directories in place. A snapshot taken during
// the swap could mistake an in-progress update for a removal. Release this
// lock before entering the applier, which may take another snapshot.
static SOURCES_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn with_sources_lock<T>(work: impl FnOnce() -> T) -> Result<T> {
    let _lock = SOURCES_LOCK
        .lock()
        .map_err(|_| anyhow!("the skill registry was interrupted"))?;
    Ok(work())
}

pub(crate) fn installed_sources() -> Result<BTreeMap<String, BTreeMap<PathBuf, Vec<u8>>>> {
    with_sources_lock(|| registry::personal_sources(&registry::default_wenlan_root()))?
}

/// A person can also take skills in from each of the delivery folders.
pub(crate) fn scan_sources(root: &Path, home: &Path) -> Vec<LocalSkill> {
    let mut dirs: Vec<_> = rulesync_personal::PERSONAL_TARGETS
        .iter()
        .map(|target| home.join(target.skills.destination))
        .collect();
    dirs.push(home.join(".agents/skills"));
    registry::scan_local_in_home(root, &dirs, home)
}

pub(crate) fn invoke_delivery() -> Result<RulesSyncReport> {
    crate::rulesync_routes::sync_registry_now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use wenlan_types::skill_registry::Scope;

    fn skill(home: &Path, folder: &str, markdown: &str) -> PathBuf {
        let path = home.join(folder);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("SKILL.md"), markdown).unwrap();
        path
    }

    #[test]
    fn rulesync_registry_snapshot_uses_author_bytes_and_only_all_spaces() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("wenlan");
        let home = scratch.path().join("home");
        let markdown = "\u{feff}---\nname: example\n---\nAuthor content.\n";
        let source = skill(&home, ".claude/skills/example", markdown);
        fs::create_dir(source.join("assets")).unwrap();
        fs::write(source.join("assets/data.bin"), [0, 255]).unwrap();
        registry::import_local_fully_shown(&root, &source, 1).unwrap();
        registry::set_additions(&root, "example", "My addition.").unwrap();
        skill(&root, "skills/handmade", "Unauthorised registry source.\n");
        fs::write(root.join("skills/example/SKILL.md"), "Edited ready copy.\n").unwrap();

        let sources = registry::personal_sources(&root).unwrap();
        let files = &sources["example"];
        assert_eq!(sources.len(), 1);
        assert_eq!(files.len(), 2);
        assert_eq!(
            files[Path::new("SKILL.md")],
            format!("{markdown}\n## My additions\n\nMy addition.\n").into_bytes()
        );
        assert_eq!(files[Path::new("assets/data.bin")], vec![0, 255]);

        registry::set_scope(
            &root,
            "example",
            Scope::Space {
                name: "Work".into(),
            },
        )
        .unwrap();
        assert!(registry::personal_sources(&root).unwrap().is_empty());
    }

    #[test]
    fn rulesync_registry_snapshot_rejects_corruption_instead_of_reporting_a_removal() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("wenlan");
        let source = skill(scratch.path(), "example", "Author content.\n");
        registry::import_local_fully_shown(&root, &source, 1).unwrap();
        let control = root.join("skills/example/.wenlan");
        fs::write(control.join("author/SKILL.md"), "Unreviewed replacement.\n").unwrap();
        assert!(registry::personal_sources(&root)
            .unwrap_err()
            .to_string()
            .contains("differ"));

        fs::write(control.join("skill.json"), "incomplete registry record").unwrap();
        assert!(registry::personal_sources(&root)
            .unwrap_err()
            .to_string()
            .contains("example"));
    }

    #[cfg(unix)]
    #[test]
    fn rulesync_registry_scan_finds_known_paths_without_reading_outside_home() {
        let scratch = tempfile::tempdir().unwrap();
        let home = scratch.path().join("home");
        let root = scratch.path().join("wenlan");
        let claude = skill(&home, ".claude/skills/claude", "Claude copy.\n");
        let codex = skill(&home, ".codex/skills/codex", "Codex copy.\n");
        let agents = skill(&home, ".agents/skills/shared", "Shared copy.\n");
        let outside = skill(scratch.path(), "outside", "Outside the tool home.\n");
        std::os::unix::fs::symlink(&outside, claude.with_file_name("outside")).unwrap();
        let skill_link = codex.with_file_name("linked-file");
        fs::create_dir(&skill_link).unwrap();
        std::os::unix::fs::symlink(outside.join("SKILL.md"), skill_link.join("SKILL.md")).unwrap();

        assert_eq!(
            scan_sources(&root, &home)
                .into_iter()
                .map(|skill| PathBuf::from(skill.path))
                .collect::<Vec<_>>(),
            vec![claude, codex, agents],
        );
    }
}
