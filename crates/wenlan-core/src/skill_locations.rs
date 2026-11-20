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
    if crate::rulesync_personal::enabled() {
        return None;
    }
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

/// Where a rules location starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulesBase {
    Home,
    /// `CODEX_HOME`, else `~/.codex`.
    CodexHome,
    /// `COPILOT_HOME`, else `~/.copilot`.
    CopilotHome,
    ConfigDir,
    /// `XDG_CONFIG_HOME`, else `~/.config`, on every platform.
    XdgConfigHome,
    /// Zed's settings root off Windows: `XDG_CONFIG_HOME` where Zed follows
    /// it (Linux), else `~/.config`.
    ZedConfigHome,
}

/// How Wenlan's rules sit in the tool's file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulesFileKind {
    /// One marked part inside the person's own always-read file.
    Block,
    /// A whole file of Wenlan's own in a folder the tool reads every file of.
    OwnedFile,
}

/// The file a tool reads at the start of every conversation, whatever the
/// project: where rules must sit for the tool to have them without being
/// asked to look. Only tools whose file is documented are listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RulesLocation {
    /// The tool's canonical daemon agent id.
    pub tool: &'static str,
    pub base: RulesBase,
    /// From the base to the tool's own folder. Wenlan never creates it.
    pub folder: &'static [&'static str],
    /// From the tool's folder to the file.
    pub file: &'static [&'static str],
    pub kind: RulesFileKind,
    /// The most characters the tool reads from the whole file.
    pub max_chars: Option<usize>,
    /// Whether an `@path` line in the file pulls another file in.
    pub at_imports: bool,
    /// Whether Wenlan may make the file when the tool's folder has none.
    pub create_file: bool,
    /// The tool whose file this tool reads instead while its own is absent.
    pub falls_back_to: Option<&'static str>,
    /// What an owned file must open with for the tool to read it always.
    pub front_matter: Option<&'static str>,
}

/// The roots a [`RulesLocation`] is resolved against. `windows` picks the
/// Windows spelling of a path. The caller says which platform it runs on,
/// so this path table stays the same source on every platform.
#[derive(Debug, Clone, Copy)]
pub struct RulesRoots<'a> {
    pub home: Option<&'a Path>,
    pub config_dir: Option<&'a Path>,
    /// A non-empty `CODEX_HOME`.
    pub codex_home: Option<&'a Path>,
    /// A non-empty `COPILOT_HOME`.
    pub copilot_home: Option<&'a Path>,
    /// A non-empty, absolute `XDG_CONFIG_HOME`.
    pub xdg_config_home: Option<&'a Path>,
    /// Whether Zed on this platform follows `XDG_CONFIG_HOME`. It does on
    /// Linux; on macOS it always uses `~/.config`.
    pub zed_follows_xdg: bool,
    pub windows: bool,
}

impl<'a> RulesRoots<'a> {
    pub fn under_home(home: &'a Path) -> Self {
        Self {
            home: Some(home),
            config_dir: None,
            codex_home: None,
            copilot_home: None,
            xdg_config_home: None,
            zed_follows_xdg: false,
            windows: false,
        }
    }
}

/// A resolved [`RulesLocation`]: the tool's own folder and the file in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulesFile {
    pub tool_folder: PathBuf,
    pub path: PathBuf,
}

/// Kiro reads a steering file in every conversation only when its front
/// matter says so; the front matter must be the first thing in the file.
pub const KIRO_ALWAYS_FRONT_MATTER: &str = "---\ninclusion: always\n---\n";

/// Windsurf reads no more than this many characters of its global rules file.
pub const WINDSURF_RULES_MAX_CHARS: usize = 6_000;

/// Resolve either an app client type (`codex_cli`) or a daemon agent id
/// (`codex`) to where that tool reads always-on rules. `windows` picks the
/// Windows location where it differs. Tools reached only through MCP, and
/// tools whose file is not documented, have none.
pub fn rules_location(tool: &str, windows: bool) -> Option<RulesLocation> {
    if crate::rulesync_personal::enabled() {
        return None;
    }
    let block = |tool, base, folder, file| RulesLocation {
        tool,
        base,
        folder,
        file,
        kind: RulesFileKind::Block,
        max_chars: None,
        at_imports: false,
        create_file: true,
        falls_back_to: None,
        front_matter: None,
    };
    Some(match tool {
        "claude_code" | "claude-code" => RulesLocation {
            at_imports: true,
            ..block("claude-code", RulesBase::Home, &[".claude"], &["CLAUDE.md"])
        },
        "codex_cli" | "codex" => block("codex", RulesBase::CodexHome, &[], &["AGENTS.md"]),
        // Gemini CLI and Antigravity read the same file.
        "gemini_cli" | "gemini-cli" => RulesLocation {
            at_imports: true,
            ..block("gemini-cli", RulesBase::Home, &[".gemini"], &["GEMINI.md"])
        },
        "antigravity" => RulesLocation {
            at_imports: true,
            ..block("antigravity", RulesBase::Home, &[".gemini"], &["GEMINI.md"])
        },
        // Copilot CLI and Copilot in VS Code read the same file.
        "copilot_cli" | "copilot-cli" | "github-copilot" => block(
            "copilot-cli",
            RulesBase::CopilotHome,
            &[],
            &["copilot-instructions.md"],
        ),
        "vscode" => block(
            "vscode",
            RulesBase::CopilotHome,
            &[],
            &["copilot-instructions.md"],
        ),
        "windsurf" => RulesLocation {
            max_chars: Some(WINDSURF_RULES_MAX_CHARS),
            ..block(
                "windsurf",
                RulesBase::Home,
                &[".codeium", "windsurf"],
                &["memories", "global_rules.md"],
            )
        },
        // OpenCode reads `~/.claude/CLAUDE.md` only while it has no
        // `AGENTS.md` of its own, so making one would hide the person's
        // Claude file from it.
        "opencode" => RulesLocation {
            create_file: false,
            falls_back_to: Some("claude-code"),
            ..block(
                "opencode",
                RulesBase::XdgConfigHome,
                &["opencode"],
                &["AGENTS.md"],
            )
        },
        "kiro" | "kiro-cli" => RulesLocation {
            kind: RulesFileKind::OwnedFile,
            front_matter: Some(KIRO_ALWAYS_FRONT_MATTER),
            ..block(
                "kiro",
                RulesBase::Home,
                &[".kiro"],
                &["steering", "wenlan-rules.md"],
            )
        },
        // Zed keeps its settings folder at `~/.config/zed` on macOS and
        // Linux (under `XDG_CONFIG_HOME` on Linux when that is set) and at
        // `%APPDATA%\Zed` on Windows.
        "zed" if windows => block("zed", RulesBase::ConfigDir, &["Zed"], &["AGENTS.md"]),
        "zed" => block("zed", RulesBase::ZedConfigHome, &["zed"], &["AGENTS.md"]),
        _ => return None,
    })
}

impl RulesLocation {
    /// The tool's folder and file under `roots`, or `None` when the base it
    /// starts from is not known.
    pub fn resolve(&self, roots: &RulesRoots<'_>) -> Option<RulesFile> {
        let base = match self.base {
            RulesBase::Home => roots.home?.to_path_buf(),
            RulesBase::CodexHome => match roots.codex_home {
                Some(dir) => dir.to_path_buf(),
                None => roots.home?.join(".codex"),
            },
            RulesBase::CopilotHome => match roots.copilot_home {
                Some(dir) => dir.to_path_buf(),
                None => roots.home?.join(".copilot"),
            },
            RulesBase::ConfigDir => roots.config_dir?.to_path_buf(),
            RulesBase::XdgConfigHome => match roots.xdg_config_home {
                Some(dir) => dir.to_path_buf(),
                None => roots.home?.join(".config"),
            },
            RulesBase::ZedConfigHome => match roots.xdg_config_home {
                Some(dir) if roots.zed_follows_xdg => dir.to_path_buf(),
                _ => roots.home?.join(".config"),
            },
        };
        let tool_folder = self.folder.iter().fold(base, |p, part| p.join(part));
        let path = self
            .file
            .iter()
            .fold(tool_folder.clone(), |p, part| p.join(part));
        Some(RulesFile { tool_folder, path })
    }
}

/// The always-read file of `tool` under `roots`.
pub fn global_rules_file(tool: &str, roots: &RulesRoots<'_>) -> Option<PathBuf> {
    Some(rules_location(tool, roots.windows)?.resolve(roots)?.path)
}

/// Tools [`rules_location`] knows, in the order their files are written. A
/// file that can pull another in comes after the files it may name, and
/// OpenCode comes after Claude Code, whose file it falls back to.
pub const GLOBAL_RULES_TOOLS: [&str; 10] = [
    "codex",
    "copilot-cli",
    "vscode",
    "windsurf",
    "zed",
    "kiro",
    "claude-code",
    "gemini-cli",
    "antigravity",
    "opencode",
];

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

    fn rules_path(tool: &str, roots: &RulesRoots<'_>) -> PathBuf {
        global_rules_file(tool, roots).unwrap_or_else(|| panic!("no rules file for {tool}"))
    }

    #[test]
    fn ux_skill_8_each_tool_has_its_documented_always_read_file() {
        let home = Path::new("/scratch/home");
        let roots = RulesRoots::under_home(home);
        for (tool, expected) in [
            ("claude-code", ".claude/CLAUDE.md"),
            ("codex", ".codex/AGENTS.md"),
            ("gemini-cli", ".gemini/GEMINI.md"),
            ("antigravity", ".gemini/GEMINI.md"),
            ("copilot-cli", ".copilot/copilot-instructions.md"),
            ("github-copilot", ".copilot/copilot-instructions.md"),
            ("vscode", ".copilot/copilot-instructions.md"),
            ("windsurf", ".codeium/windsurf/memories/global_rules.md"),
            ("opencode", ".config/opencode/AGENTS.md"),
            ("kiro", ".kiro/steering/wenlan-rules.md"),
            ("kiro-cli", ".kiro/steering/wenlan-rules.md"),
            ("zed", ".config/zed/AGENTS.md"),
        ] {
            assert_eq!(rules_path(tool, &roots), home.join(expected), "{tool}");
        }
        for tool in GLOBAL_RULES_TOOLS {
            assert_eq!(
                rules_location(tool, false).map(|location| location.tool),
                Some(tool),
                "the order lists canonical ids"
            );
        }
    }

    #[test]
    fn ux_skill_8_tools_without_a_documented_file_have_no_rules_location() {
        for tool in [
            "cursor",
            "claude-desktop",
            "chatgpt",
            "openai-mcp",
            "cline",
            "goose",
            "hermes",
            "openclaw",
            "continue",
            "kimi-code",
            "kimi-desktop",
            "muse-code",
        ] {
            assert_eq!(rules_location(tool, false), None, "{tool}");
            assert_eq!(rules_location(tool, true), None, "{tool}");
        }
    }

    #[test]
    fn ux_skill_8_codex_and_copilot_follow_their_home_override() {
        let home = Path::new("/scratch/home");
        let roots = RulesRoots {
            codex_home: Some(Path::new("/elsewhere/codex")),
            copilot_home: Some(Path::new("/elsewhere/copilot")),
            ..RulesRoots::under_home(home)
        };
        assert_eq!(
            rules_path("codex", &roots),
            Path::new("/elsewhere/codex/AGENTS.md")
        );
        let codex = rules_location("codex", false).unwrap().resolve(&roots);
        assert_eq!(
            codex.unwrap().tool_folder,
            Path::new("/elsewhere/codex"),
            "the override is the folder that must already be there"
        );
        for tool in ["copilot-cli", "vscode"] {
            assert_eq!(
                rules_path(tool, &roots),
                Path::new("/elsewhere/copilot/copilot-instructions.md")
            );
        }
        // The other tools do not move with them.
        assert_eq!(
            rules_path("claude-code", &roots),
            home.join(".claude/CLAUDE.md")
        );
    }

    #[test]
    fn ux_skill_8_opencode_and_zed_follow_the_xdg_config_folder() {
        let home = Path::new("/home/me");
        let xdg = RulesRoots {
            xdg_config_home: Some(Path::new("/cfg")),
            zed_follows_xdg: true,
            ..RulesRoots::under_home(home)
        };
        assert_eq!(
            rules_path("opencode", &xdg),
            Path::new("/cfg/opencode/AGENTS.md")
        );
        assert_eq!(rules_path("zed", &xdg), Path::new("/cfg/zed/AGENTS.md"));
        // Zed on macOS keeps `~/.config` whatever the variable says.
        let mac = RulesRoots {
            zed_follows_xdg: false,
            ..xdg
        };
        assert_eq!(
            rules_path("zed", &mac),
            Path::new("/home/me/.config/zed/AGENTS.md")
        );
        assert_eq!(
            rules_path("opencode", &mac),
            Path::new("/cfg/opencode/AGENTS.md")
        );
        // Nothing else moves with it.
        assert_eq!(
            rules_path("claude-code", &xdg),
            Path::new("/home/me/.claude/CLAUDE.md")
        );
    }

    #[test]
    fn ux_skill_8_zed_is_under_the_config_folder_on_windows_only() {
        let home = Path::new("/scratch/home");
        let appdata = Path::new("/scratch/appdata");
        let roots = RulesRoots {
            config_dir: Some(appdata),
            windows: true,
            ..RulesRoots::under_home(home)
        };
        assert_eq!(rules_path("zed", &roots), appdata.join("Zed/AGENTS.md"));
        // Off Windows the platform config folder is not where Zed looks
        // (macOS reports `~/Library/Application Support`).
        let roots = RulesRoots {
            windows: false,
            ..roots
        };
        assert_eq!(
            rules_path("zed", &roots),
            home.join(".config/zed/AGENTS.md")
        );
        // No other tool's file moves on Windows.
        for tool in GLOBAL_RULES_TOOLS.into_iter().filter(|tool| *tool != "zed") {
            assert_eq!(
                rules_location(tool, true),
                rules_location(tool, false),
                "{tool}"
            );
        }
    }

    #[test]
    fn ux_skill_8_tool_limits_imports_and_owned_files_follow_the_tools() {
        let location = |tool| rules_location(tool, false).unwrap();
        assert_eq!(location("windsurf").max_chars, Some(6_000));
        for tool in GLOBAL_RULES_TOOLS {
            let found = location(tool);
            assert_eq!(
                found.at_imports,
                ["claude-code", "gemini-cli", "antigravity"].contains(&tool),
                "{tool} imports"
            );
            assert_eq!(
                found.kind == RulesFileKind::OwnedFile,
                tool == "kiro",
                "{tool} kind"
            );
            assert_eq!(found.create_file, tool != "opencode", "{tool} file");
            assert_eq!(found.max_chars.is_some(), tool == "windsurf", "{tool}");
        }
        assert_eq!(location("opencode").falls_back_to, Some("claude-code"));
        assert_eq!(
            location("kiro").front_matter,
            Some("---\ninclusion: always\n---\n")
        );
        assert_eq!(
            location("kiro")
                .resolve(&RulesRoots::under_home(Path::new("/h")))
                .unwrap()
                .tool_folder,
            Path::new("/h/.kiro")
        );
    }
}
