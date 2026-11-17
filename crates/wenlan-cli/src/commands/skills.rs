// SPDX-License-Identifier: Apache-2.0
//! Read approved Space rules from the local Wenlan library.

use anyhow::Result;
use wenlan_types::{SkillLineKind, SpaceRulesResponse};

use crate::client::WenlanClient;
use crate::output::{print_json, ResolvedFormat};

pub async fn read_space_rules(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
) -> Result<()> {
    let response = client.read_space_rules().await?;
    if response.lines.is_empty() {
        if !quiet {
            println!("{}", no_rules_line(&response.space_name));
        }
        return Ok(());
    }
    match format {
        // Piped output and `--format json` keep every field: an AI tool needs
        // the rule ids to report a rule that changed its plan.
        ResolvedFormat::Json => print_json(&response)?,
        ResolvedFormat::Table => print!("{}", rules_for_a_person(&response)),
    }
    Ok(())
}

fn no_rules_line(space_name: &str) -> String {
    if space_name.is_empty() {
        "No approved working rules.".to_string()
    } else {
        format!("No approved working rules for {space_name}.")
    }
}

/// The rules as a person reads them in a terminal: the line, then the page it
/// came from. Ids, revisions and quotes stay in the JSON form.
fn rules_for_a_person(response: &SpaceRulesResponse) -> String {
    // No Space picked: the daemon returns only the rules used in every Space.
    let mut out = if response.space_name.is_empty() {
        "Working rules used in every Space\n\n".to_string()
    } else {
        format!("Working rules for {}\n\n", response.space_name)
    };
    for line in &response.lines {
        out.push_str("- ");
        // A decision or a pitfall read as a bare order misleads, so say which it is.
        match line.kind {
            SkillLineKind::Rule => {}
            SkillLineKind::Decision => out.push_str("Decision: "),
            SkillLineKind::Pitfall => out.push_str("Pitfall: "),
        }
        out.push_str(line.text.trim());
        out.push('\n');
        let scope = if line.general {
            ", used in every Space"
        } else {
            ""
        };
        // A rule the person wrote themselves says so (UX-SKILL-10).
        let source = if line.hand_written {
            "written by you in"
        } else {
            "from"
        };
        out.push_str(&format!(
            "  {source} \"{}\"{scope}\n",
            line.source_page_title
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wenlan_types::SkillLine;

    fn line(text: &str, title: &str, general: bool) -> SkillLine {
        SkillLine {
            id: "rule_0123456789abcdef".to_string(),
            kind: if text.starts_with("Never") {
                SkillLineKind::Pitfall
            } else {
                SkillLineKind::Rule
            },
            text: text.to_string(),
            source_page_id: "page_0123456789abcdef".to_string(),
            source_page_title: title.to_string(),
            source_revision: 3,
            source_revision_at: 1_794_000_000,
            source_quote: text.to_string(),
            person_edited: false,
            general,
            hand_written: false,
        }
    }

    #[test]
    fn ux_skill_10_a_rule_the_person_wrote_says_so() {
        let response = SpaceRulesResponse {
            space_id: "space_0123456789abcdef".to_string(),
            space_name: "Wenlan".to_string(),
            rules_page_id: None,
            lines: vec![
                SkillLine {
                    hand_written: true,
                    person_edited: true,
                    ..line("Use the merge queue.", "Git", false)
                },
                line("Answer in plain words.", "How I talk", true),
            ],
        };
        assert_eq!(
            rules_for_a_person(&response),
            "Working rules for Wenlan\n\n\
             - Use the merge queue.\n  written by you in \"Git\"\n\
             - Answer in plain words.\n  from \"How I talk\", used in every Space\n"
        );
    }

    #[test]
    fn a_person_sees_each_rule_and_its_page_without_ids() {
        let response = SpaceRulesResponse {
            space_id: "space_0123456789abcdef".to_string(),
            space_name: "Wenlan".to_string(),
            rules_page_id: Some("page_fedcba9876543210".to_string()),
            lines: vec![
                line("Merge through the merge queue only.", "Release flow", false),
                line("Answer in plain words.", "How I talk", true),
            ],
        };
        assert_eq!(
            rules_for_a_person(&response),
            "Working rules for Wenlan\n\n\
             - Merge through the merge queue only.\n  from \"Release flow\"\n\
             - Answer in plain words.\n  from \"How I talk\", used in every Space\n"
        );
    }

    #[test]
    fn with_no_space_the_heading_and_the_empty_line_name_no_space() {
        let response = SpaceRulesResponse {
            space_id: String::new(),
            space_name: String::new(),
            rules_page_id: None,
            lines: vec![line("Never skip a failing test.", "How I test", true)],
        };
        assert_eq!(
            rules_for_a_person(&response),
            "Working rules used in every Space\n\n- Pitfall: Never skip a failing test.\n  from \"How I test\", used in every Space\n"
        );
        assert_eq!(no_rules_line(""), "No approved working rules.");
        assert_eq!(
            no_rules_line("Wenlan"),
            "No approved working rules for Wenlan."
        );
    }
}
