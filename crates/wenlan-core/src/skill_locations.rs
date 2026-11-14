// SPDX-License-Identifier: Apache-2.0
//! Shared personal skill-folder locations for connected AI tools.
//!
//! This is a path table only. The app decides which tools receive its bundled
//! plugin skills; the library-grown feature uses every location here.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillsBase {
    Home,
    ConfigDir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillsLocation {
    pub base: SkillsBase,
    pub tail: &'static [&'static str],
}

/// Resolve either an app client type (`codex_cli`) or its canonical daemon
/// agent id (`codex`). Unknown tools have no filesystem skill surface here.
pub fn skills_location(tool: &str) -> Option<SkillsLocation> {
    let (base, tail): (SkillsBase, &'static [&'static str]) = match tool {
        "claude_code" | "claude-code" => (SkillsBase::Home, &[".claude", "skills"]),
        // OpenAI's user-scope Agent Skills directory for Codex. The installed
        // version and documentation receipt belong in the delivery/PR record.
        "codex_cli" | "codex" => (SkillsBase::Home, &[".agents", "skills"]),
        "cursor" => (SkillsBase::Home, &[".cursor", "skills"]),
        "gemini_cli" | "gemini-cli" | "antigravity" => (SkillsBase::Home, &[".gemini", "skills"]),
        "copilot_cli" | "copilot-cli" | "vscode" => (SkillsBase::Home, &[".copilot", "skills"]),
        "windsurf" => (SkillsBase::Home, &[".codeium", "windsurf", "skills"]),
        "kiro" => (SkillsBase::Home, &[".kiro", "skills"]),
        "opencode" => (SkillsBase::Home, &[".config", "opencode", "skills"]),
        "kimi_code" | "kimi-code" => (SkillsBase::Home, &[".kimi-code", "skills"]),
        "kimi_desktop" | "kimi-desktop" => (
            SkillsBase::ConfigDir,
            &["kimi-desktop", "daimon-share", "daimon", "skills"],
        ),
        _ => return None,
    };
    Some(SkillsLocation { base, tail })
}

pub fn skills_dir_for(
    tool: &str,
    home: Option<&Path>,
    config_dir: Option<&Path>,
) -> Option<Result<PathBuf, &'static str>> {
    let location = skills_location(tool)?;
    let base = match location.base {
        SkillsBase::Home => home.ok_or("the home directory"),
        SkillsBase::ConfigDir => config_dir.ok_or("the application-configuration directory"),
    };
    Some(base.map(|base| {
        location
            .tail
            .iter()
            .fold(base.to_path_buf(), |p, part| p.join(part))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_and_claude_use_documented_personal_skill_folders() {
        let home = Path::new("/scratch/home");
        assert_eq!(
            skills_dir_for("codex", Some(home), None),
            Some(Ok(home.join(".agents/skills")))
        );
        assert_eq!(
            skills_dir_for("claude-code", Some(home), None),
            Some(Ok(home.join(".claude/skills")))
        );
    }
}
