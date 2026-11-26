// SPDX-License-Identifier: Apache-2.0
//! `wenlan recall <query>` — POST /api/memory/search.

use anyhow::Result;
use wenlan_types::responses::SearchMemoryResponse;

use crate::client::WenlanClient;
use crate::output::{print_json, ResolvedFormat};

pub async fn run(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    query: String,
    limit: usize,
) -> Result<()> {
    let response = client.recall(query.clone(), limit).await?;
    if quiet {
        return Ok(());
    }
    match format {
        ResolvedFormat::Json => print_json(&response)?,
        ResolvedFormat::Table => {
            print!("{}", format_table(&response));
            if response.results.is_empty() && reads_one_space(client) {
                // Best effort: a failed look adds nothing.
                if let Ok(wider) = client
                    .recall_outside_space(query.clone(), OTHER_SPACES_PROBE)
                    .await
                {
                    if let Some(line) = other_spaces_hint(&query, wider.results.len()) {
                        println!("{line}");
                    }
                }
            }
        }
    }
    Ok(())
}

/// How many matches outside the Space the zero-result hint asks for.
const OTHER_SPACES_PROBE: usize = 5;

/// A terminal pinned to one Space with `WENLAN_SPACE` chose not to see others.
fn reads_one_space(client: &WenlanClient) -> bool {
    client.is_space_scoped()
        && std::env::var("WENLAN_SPACE")
            .map(|space| space.trim().is_empty())
            .unwrap_or(true)
}

/// The line after "(no recalled memories)" when other Spaces match.
fn other_spaces_hint(query: &str, found: usize) -> Option<String> {
    let count = match found {
        0 => return None,
        1 => "1 match".to_string(),
        n if n >= OTHER_SPACES_PROBE => format!("{n} or more matches"),
        n => format!("{n} matches"),
    };
    Some(format!(
        "{count} in other Spaces: `wenlan --all-spaces recall {query:?}` shows them."
    ))
}

fn format_table(response: &SearchMemoryResponse) -> String {
    if response.results.is_empty() {
        // A library of pages and no memories yet (every first-run user who
        // only wrote notes) still has pages worth listing.
        let mut output = "(no recalled memories)\n".to_string();
        output.push_str(&super::format_supplemental_pages(
            response.supplemental_pages.as_deref(),
        ));
        output.push_str(&super::format_linked_pages(
            response.linked_pages.as_deref(),
        ));
        return output;
    }
    let mut output = format!(
        "{} recalled memor{} in {:.0}ms\n",
        response.results.len(),
        if response.results.len() == 1 {
            "y"
        } else {
            "ies"
        },
        response.took_ms
    );
    for result in &response.results {
        let title = if result.title.is_empty() {
            result.content.lines().next().unwrap_or("(no title)")
        } else {
            &result.title
        };
        let title = if title.chars().count() > 70 {
            format!("{}...", title.chars().take(67).collect::<String>())
        } else {
            title.to_string()
        };
        output.push_str(&format!(
            "  [{:.3}] {} ({})\n",
            result.score, title, result.source_id
        ));
    }
    output.push_str(&super::format_supplemental_pages(
        response.supplemental_pages.as_deref(),
    ));
    output.push_str(&super::format_linked_pages(
        response.linked_pages.as_deref(),
    ));
    output
}

#[cfg(test)]
mod tests {
    use super::{format_table, other_spaces_hint};

    #[test]
    fn zero_results_hint_counts_other_spaces() {
        assert_eq!(other_spaces_hint("pnpm", 0), None);
        assert_eq!(
            other_spaces_hint("pnpm", 1).unwrap(),
            "1 match in other Spaces: `wenlan --all-spaces recall \"pnpm\"` shows them."
        );
        assert!(other_spaces_hint("q", 3)
            .unwrap()
            .starts_with("3 matches in other Spaces"));
        assert!(other_spaces_hint("q", 5)
            .unwrap()
            .starts_with("5 or more matches"));
    }

    use wenlan_types::responses::SearchMemoryResponse;

    #[test]
    fn lists_compiled_pages_when_no_memory_matched() {
        let response = SearchMemoryResponse {
            linked_pages: None,
            results: Vec::new(),
            took_ms: 3.0,
            supplemental_pages: Some(vec![super::super::tests::page(
                "Trip plan",
                "page-source-1",
            )]),
        };
        let output = format_table(&response);
        assert!(output.starts_with("(no recalled memories)\n"), "{output}");
        assert!(output.contains("Compiled pages:\n"), "{output}");
        assert!(output.contains("Trip plan (page-source-1)"), "{output}");
    }

    #[test]
    fn ux_find_5_lists_the_pages_compiled_pages_link_to() {
        let response = SearchMemoryResponse {
            results: Vec::new(),
            took_ms: 3.0,
            supplemental_pages: Some(vec![super::super::tests::page(
                "Trip plan",
                "page-source-1",
            )]),
            linked_pages: Some(vec![wenlan_types::responses::LinkedPage {
                id: "page-tickets".to_string(),
                title: "Tickets".to_string(),
                point: "Buy them in March.".to_string(),
                linked_from: vec!["Trip plan".to_string()],
                space: None,
                confirmed: true,
            }]),
        };
        let output = format_table(&response);
        assert!(
            output.contains("Linked pages:\n  - Tickets (page-tickets), linked from Trip plan\n"),
            "{output}"
        );
    }

    #[test]
    fn says_so_when_nothing_matched_at_all() {
        let response = SearchMemoryResponse {
            linked_pages: None,
            results: Vec::new(),
            took_ms: 3.0,
            supplemental_pages: None,
        };
        assert_eq!(format_table(&response), "(no recalled memories)\n");
    }
}
