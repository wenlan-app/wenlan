// SPDX-License-Identifier: Apache-2.0
//! Shared personal skill-folder locations for connected AI tools.
//!
//! This is a path table only. The app decides which tools receive its bundled
//! plugin skills.
//!
//! The documented global paths follow vercel-labs/skills, licensed MIT:
//! https://github.com/vercel-labs/skills/blob/main/docs/agents.md

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
        "cursor" => (SkillsBase::Home, &[".agents", "skills"]),
        "gemini_cli" | "gemini-cli" => (SkillsBase::Home, &[".gemini", "skills"]),
        "antigravity" => (SkillsBase::Home, &[".gemini", "config", "skills"]),
        "copilot_cli" | "copilot-cli" | "github-copilot" | "vscode" => {
            (SkillsBase::Home, &[".copilot", "skills"])
        }
        "windsurf" => (SkillsBase::Home, &[".codeium", "windsurf", "skills"]),
        "kiro" | "kiro-cli" => (SkillsBase::Home, &[".kiro", "skills"]),
        "opencode" => (SkillsBase::Home, &[".config", "opencode", "skills"]),
        "kimi_code" | "kimi-code" => (SkillsBase::Home, &[".agents", "skills"]),
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

/// The file a tool reads at the start of every conversation, whatever the
/// project: where rules must sit for the tool to follow them without being
/// asked to look. Only tools whose file is known for certain are listed.
pub fn global_rules_file(tool: &str, home: &Path) -> Option<PathBuf> {
    let tail: &[&str] = match tool {
        "claude_code" | "claude-code" => &[".claude", "CLAUDE.md"],
        "codex_cli" | "codex" => &[".codex", "AGENTS.md"],
        _ => return None,
    };
    Some(tail.iter().fold(home.to_path_buf(), |p, part| p.join(part)))
}

/// Tools [`global_rules_file`] knows, in the order their files are written.
pub const GLOBAL_RULES_TOOLS: [&str; 2] = ["codex", "claude-code"];

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

    #[test]
    fn shared_agent_skill_paths_follow_upstream_global_paths() {
        let home = Path::new("/scratch/home");
        for (tool, expected) in [
            ("cursor", ".agents/skills"),
            ("gemini-cli", ".gemini/skills"),
            ("antigravity", ".gemini/config/skills"),
            ("github-copilot", ".copilot/skills"),
            ("kimi-code", ".agents/skills"),
            ("windsurf", ".codeium/windsurf/skills"),
            ("kiro-cli", ".kiro/skills"),
            ("opencode", ".config/opencode/skills"),
        ] {
            assert_eq!(
                skills_dir_for(tool, Some(home), None),
                Some(Ok(home.join(expected))),
                "unexpected global skill path for {tool}"
            );
        }
    }
}
