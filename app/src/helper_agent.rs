// SPDX-License-Identifier: AGPL-3.0-only
//! Installs `wenlan-helper`, a small read-only agent pinned to an AI tool's
//! cheap model, next to Wenlan's skills.
//!
//! The steep skill hands its small work (grouping new material by topic,
//! organizing a page's sources) to a helper on the tool's cheapest model so
//! it never spends the main model. Some tools can only start such a helper
//! from an agent file that names the model. Wenlan writes one only where the
//! file format and the cheap model's id are confirmed:
//!
//! - Cursor: `~/.cursor/agents/wenlan-helper.md`, `model: composer-2.5`,
//!   `readonly: true`.
//! - Antigravity: `~/.gemini/config/agents/wenlan-helper.md`, `model: flash`.
//!
//! The file carries [`MANAGED_LINE`]. A `wenlan-helper.md` without it is the
//! user's and is never overwritten. Installing is best effort: the skills are
//! the deliverable, so a helper that cannot be written never fails them.

use std::path::{Path, PathBuf};

/// The agent's name, which the generated steep skill names too.
pub(crate) const HELPER_NAME: &str = "wenlan-helper";

/// The line that marks a helper file as Wenlan's. Removing it keeps the
/// user's edits: the next setup leaves the file alone.
pub(crate) const MANAGED_LINE: &str = "<!-- wenlan-managed: Wenlan writes this file when it \
connects this tool and replaces it on the next setup. Delete this line to keep your own \
version. -->";

const BODY: &str = "You are a small helper for Wenlan. Each request is one piece of small \
work: group items by topic, answer a page's organize tasks, or answer a yes or no check. Do \
exactly what the request's instructions say and reply with only what they ask for, usually \
JSON. Use only the text you are given. Do not read or change files, run commands, call tools \
or write pages.";

const DESCRIPTION: &str = "Small read-only helper for Wenlan's steep skill: groups new \
material by topic and organizes a page's sources on a cheap model, replying with only the \
JSON asked for.";

/// Where `client_type` reads agent files, and the frontmatter lines that
/// pin its cheap model. `None` for a tool Wenlan writes no helper for.
fn spec(client_type: &str) -> Option<(&'static [&'static str], &'static [&'static str])> {
    match client_type {
        "cursor" => Some((
            &[".cursor", "agents"],
            &["model: composer-2.5", "readonly: true"],
        )),
        "antigravity" => Some((&[".gemini", "config", "agents"], &["model: flash"])),
        _ => None,
    }
}

/// The helper file `client_type` reads, under `home`.
pub(crate) fn helper_agent_path(client_type: &str, home: &Path) -> Option<PathBuf> {
    let (tail, _) = spec(client_type)?;
    let dir = tail
        .iter()
        .fold(home.to_path_buf(), |acc, part| acc.join(part));
    Some(dir.join(format!("{HELPER_NAME}.md")))
}

/// The helper file's text for `client_type`.
pub(crate) fn helper_agent_text(client_type: &str) -> Option<String> {
    let (_, model_lines) = spec(client_type)?;
    let mut text = format!("---\nname: {HELPER_NAME}\ndescription: {DESCRIPTION}\n");
    for line in model_lines {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str("---\n\n");
    text.push_str(MANAGED_LINE);
    text.push_str("\n\n");
    text.push_str(BODY);
    text.push('\n');
    Some(text)
}

/// What an install did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HelperInstall {
    /// Written: it was absent, or Wenlan's own copy was replaced.
    Written,
    /// A file of that name is the user's; it was left alone.
    LeftAlone(PathBuf),
    /// This tool gets no helper.
    NotForThisTool,
}

/// Write `text` at `path` unless a file there is not Wenlan's. The write goes
/// to a hidden sibling first and is renamed into place, so a tool reading
/// the folder never sees half a file.
pub(crate) fn install_helper_agent_at(path: &Path, text: &str) -> std::io::Result<HelperInstall> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() => return Ok(HelperInstall::LeftAlone(path.to_path_buf())),
        Ok(_) => {
            let existing = std::fs::read_to_string(path).unwrap_or_default();
            if !existing.contains(MANAGED_LINE) {
                return Ok(HelperInstall::LeftAlone(path.to_path_buf()));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("helper path has no folder"))?;
    std::fs::create_dir_all(dir)?;
    let staging = dir.join(format!(".{HELPER_NAME}.wenlan-new-{}", std::process::id()));
    std::fs::write(&staging, text)?;
    if let Err(error) = std::fs::rename(&staging, path) {
        // Windows refuses a rename onto an existing file.
        let retry = std::fs::remove_file(path).and_then(|_| std::fs::rename(&staging, path));
        if retry.is_err() {
            let _ = std::fs::remove_file(&staging);
            return Err(error);
        }
    }
    Ok(HelperInstall::Written)
}

/// Install the helper for `client_type` under the AI-tool `home`.
pub(crate) fn install_helper_agent(
    client_type: &str,
    home: &Path,
) -> std::io::Result<HelperInstall> {
    let (Some(path), Some(text)) = (
        helper_agent_path(client_type, home),
        helper_agent_text(client_type),
    ) else {
        return Ok(HelperInstall::NotForThisTool);
    };
    install_helper_agent_at(&path, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cursor_and_antigravity_get_a_helper_at_their_agent_folders() {
        let home = PathBuf::from("/fixture/home");
        assert_eq!(
            helper_agent_path("cursor", &home),
            Some(home.join(".cursor").join("agents").join("wenlan-helper.md"))
        );
        assert_eq!(
            helper_agent_path("antigravity", &home),
            Some(
                home.join(".gemini")
                    .join("config")
                    .join("agents")
                    .join("wenlan-helper.md")
            )
        );
        for client_type in [
            "gemini_cli",
            "copilot_cli",
            "vscode",
            "windsurf",
            "kiro",
            "opencode",
            "kimi_code",
            "claude_code",
            "codex_cli",
        ] {
            assert_eq!(helper_agent_path(client_type, &home), None, "{client_type}");
            assert_eq!(helper_agent_text(client_type), None, "{client_type}");
        }
    }

    #[test]
    fn the_helper_pins_each_tools_cheap_model_and_is_marked() {
        let cursor = helper_agent_text("cursor").unwrap();
        assert!(cursor.starts_with("---\nname: wenlan-helper\ndescription: "));
        assert!(cursor.contains("\nmodel: composer-2.5\nreadonly: true\n---\n"));
        let antigravity = helper_agent_text("antigravity").unwrap();
        assert!(antigravity.contains("\nmodel: flash\n---\n"));
        for text in [&cursor, &antigravity] {
            assert!(text.contains(MANAGED_LINE));
            assert!(text.contains("Do not read or change files"));
            // One frontmatter block, each key on one line.
            assert_eq!(text.matches("---\n").count(), 2, "{text}");
        }
    }

    #[test]
    fn install_writes_replaces_its_own_copy_and_leaves_the_users_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let path = helper_agent_path("cursor", home).unwrap();

        assert_eq!(
            install_helper_agent("cursor", home).unwrap(),
            HelperInstall::Written
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            helper_agent_text("cursor").unwrap()
        );

        // Wenlan's own older copy is replaced.
        std::fs::write(&path, format!("old\n{MANAGED_LINE}\n")).unwrap();
        assert_eq!(
            install_helper_agent("cursor", home).unwrap(),
            HelperInstall::Written
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            helper_agent_text("cursor").unwrap()
        );

        // Without the marker line it is the user's.
        std::fs::write(&path, "---\nname: wenlan-helper\nmodel: mine\n---\n").unwrap();
        assert_eq!(
            install_helper_agent("cursor", home).unwrap(),
            HelperInstall::LeftAlone(path.clone())
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "---\nname: wenlan-helper\nmodel: mine\n---\n"
        );

        // A folder of that name is the user's too.
        let ag = helper_agent_path("antigravity", home).unwrap();
        std::fs::create_dir_all(&ag).unwrap();
        assert_eq!(
            install_helper_agent("antigravity", home).unwrap(),
            HelperInstall::LeftAlone(ag.clone())
        );
        assert!(ag.is_dir());

        // No staging file is left behind, and other agents stay.
        let agents = path.parent().unwrap();
        std::fs::write(agents.join("theirs.md"), "theirs").unwrap();
        std::fs::remove_file(&path).unwrap();
        install_helper_agent("cursor", home).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(agents)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["theirs.md", "wenlan-helper.md"]);

        assert_eq!(
            install_helper_agent("kiro", home).unwrap(),
            HelperInstall::NotForThisTool
        );
    }
}
