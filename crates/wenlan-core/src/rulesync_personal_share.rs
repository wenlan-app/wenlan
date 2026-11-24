// SPDX-License-Identifier: Apache-2.0
//! Classify lines in the personal rule overlay before they enter a share preview.

use std::sync::LazyLock;

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag};
use regex::Regex;

use super::reject_secret_lines;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LocalReason {
    Credential,
    Imported,
    UnverifiedOrigin,
    KeptByPerson,
    Import,
    ToolReference,
    MarkdownBlock,
    Section,
    EmptyHeading,
}

impl LocalReason {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Credential => "This line may hold a credential.",
            Self::Imported => "Comes from a file Claude Code imports",
            Self::UnverifiedOrigin => {
                "Could not verify that this line was authored in Claude Code's rule file."
            }
            Self::KeptByPerson => "You chose to keep this line in its own tool.",
            Self::Import => "Only Claude Code reads this import.",
            Self::ToolReference => "This line names a tool or a tool-specific file or folder.",
            Self::EmptyHeading => "This heading has no rule to share under it.",
            Self::MarkdownBlock => "This line belongs to a Markdown block that stays together.",
            Self::Section => "Belongs to a section that stays in this tool",
        }
    }
}

fn tool_reference(line: &str) -> bool {
    static TOOL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r"(?i)\b(?:claude(?:code)?|codex(?:cli)?|gemini(?:cli)?|antigravity",
            r"|copilot(?:cli)?|cursor|open[ -]?code|muse[ -]?code)\b",
            r"|\b(?:agents(?:\.codex)?\.md|claude\.md|gemini\.md|copilot-instructions\.md)\b",
            r"|\.(?:agents|claude|codex|gemini|copilot|cursor|config[/\\](?:opencode|muse))(?:[/\\]|\b)",
        ))
        .expect("tool names and paths")
    });
    TOOL.is_match(line)
}

/// Keep the original line text (without its line ending) and mark lines which
/// cannot move independently. Check credentials before assigning any reason
/// that would let a caller display the line.
pub fn overlay_lines(text: &str) -> Vec<(&str, Option<LocalReason>)> {
    overlay_lines_with(text, |_| None)
}

pub fn overlay_lines_with(
    text: &str,
    local: impl Fn(&str) -> Option<LocalReason>,
) -> Vec<(&str, Option<LocalReason>)> {
    let mut blocks = Vec::new();
    let mut heading_ranges = Vec::new();
    for (event, range) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock) => blocks.push(range),
            Event::Start(Tag::Heading { level, .. }) => heading_ranges.push((range, level)),
            _ => {}
        }
    }
    let mut blocks = blocks.into_iter().peekable();
    let mut headings = heading_ranges.iter().enumerate().peekable();
    let mut lines = Vec::new();
    let mut sections = Vec::new();
    let mut lead_ins = Vec::new();
    let mut in_key = false;
    let mut start = 0;
    for raw in text.split_inclusive('\n') {
        let end = start + raw.len();
        let line = raw.lines().next().unwrap_or(raw);
        let trimmed = line.trim();
        while blocks.peek().is_some_and(|block| block.end <= start) {
            blocks.next();
        }
        while headings
            .peek()
            .is_some_and(|(_, (range, _))| range.end <= start)
        {
            headings.next();
        }
        let heading = headings.peek().filter(|(_, (range, _))| end > range.start);
        let in_block = blocks.peek().is_some_and(|block| end > block.start);
        start = end;
        if trimmed.is_empty() {
            continue;
        }

        let refused = reject_secret_lines(line).is_err();
        let credential = refused || in_key;
        if refused && trimmed.contains("-----BEGIN ") && trimmed.contains("PRIVATE KEY-----") {
            in_key = true;
        }
        if trimmed.contains("-----END ") && trimmed.contains("PRIVATE KEY-----") {
            in_key = false;
        }
        let reason = if credential {
            Some(LocalReason::Credential)
        } else if let Some(reason) = local(line) {
            Some(reason)
        } else if trimmed
            .strip_prefix('@')
            .is_some_and(|path| !path.is_empty() && !path.contains(char::is_whitespace))
        {
            // The same syntax in a different tool would become a Claude
            // import when delivered there.
            Some(LocalReason::Import)
        } else if tool_reference(line) {
            Some(LocalReason::ToolReference)
        } else if in_block {
            Some(LocalReason::MarkdownBlock)
        } else {
            None
        };
        if let Some((index, (_, level))) = heading {
            match sections.last_mut() {
                Some((previous, _, _, stop)) if previous == index => *stop += 1,
                _ => sections.push((*index, *level, lines.len(), lines.len() + 1)),
            }
        }
        lead_ins.push(heading.is_none() && !in_block && trimmed.ends_with(':'));
        lines.push((line, reason));
    }

    let mut headings = sections.iter().peekable();
    let mut section_level: Option<HeadingLevel> = None;
    let mut local_lead_in = false;
    let mut at = 0;
    while at < lines.len() {
        if let Some((_, level, _, end)) = headings.next_if(|(_, _, start, _)| *start == at) {
            local_lead_in = false;
            section_level = section_level.filter(|enclosing| enclosing < level);
            let heading = &mut lines[at..*end];
            let local_heading = heading.iter().any(|(_, reason)| reason.is_some());
            if local_heading || section_level.is_some() {
                for (_, reason) in heading {
                    reason.get_or_insert(LocalReason::Section);
                }
                section_level.get_or_insert(*level);
            }
            at = *end;
        } else {
            let (_, reason) = &mut lines[at];
            if local_lead_in || section_level.is_some() {
                reason.get_or_insert(LocalReason::Section);
            }
            local_lead_in |= lead_ins[at] && reason.is_some();
            at += 1;
        }
    }

    // A heading needs portable prose in its section, including any portable
    // subsections. Blank lines and other headings cannot supply that prose.
    for (index, &(_, level, start, end)) in sections.iter().enumerate().rev() {
        let stop = sections[index + 1..]
            .iter()
            .find(|(_, next, _, _)| *next <= level)
            .map_or(lines.len(), |(_, _, start, _)| *start);
        if !lines[end..stop].iter().any(|(_, reason)| reason.is_none()) {
            for (_, reason) in &mut lines[start..end] {
                reason.get_or_insert(LocalReason::EmptyHeading);
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rulesync_share_classifies_imports_tool_references_and_empty_headings() {
        let overlay = "## Empty\n\n## Local\n@./more.md\nUse CLAUDE.md here.\n\n\
            ## Portable\nCheck the release list.\n\
            Use the .codex/ folder.\nUse AGENTS.md instead of GEMINI.md.\nRestart OpenCode.\n\
            # Another empty heading\n\nStandalone\n----------\n";
        assert_eq!(
            overlay_lines(overlay),
            [
                ("## Empty", Some(LocalReason::EmptyHeading)),
                ("## Local", Some(LocalReason::EmptyHeading)),
                ("@./more.md", Some(LocalReason::Import)),
                ("Use CLAUDE.md here.", Some(LocalReason::ToolReference)),
                ("## Portable", None),
                ("Check the release list.", None),
                ("Use the .codex/ folder.", Some(LocalReason::ToolReference)),
                (
                    "Use AGENTS.md instead of GEMINI.md.",
                    Some(LocalReason::ToolReference)
                ),
                ("Restart OpenCode.", Some(LocalReason::ToolReference)),
                ("# Another empty heading", Some(LocalReason::EmptyHeading)),
                ("Standalone", Some(LocalReason::EmptyHeading)),
                ("----------", Some(LocalReason::EmptyHeading)),
            ]
        );
    }

    #[test]
    fn rulesync_share_redacts_a_credential_even_in_a_tool_line_or_key_block() {
        let overlay = "Codex PASSWORD=fixture-secret\n```\n-----BEGIN PRIVATE KEY-----\n\
            synthetic-key-body\n-----END PRIVATE KEY-----\n```\nKeep secrets local.\n";
        let reasons = overlay_lines(overlay)
            .into_iter()
            .map(|(_, reason)| reason)
            .collect::<Vec<_>>();
        assert_eq!(
            reasons,
            [
                Some(LocalReason::Credential),
                Some(LocalReason::MarkdownBlock),
                Some(LocalReason::Credential),
                Some(LocalReason::Credential),
                Some(LocalReason::Credential),
                Some(LocalReason::MarkdownBlock),
                None,
            ]
        );
    }
}
