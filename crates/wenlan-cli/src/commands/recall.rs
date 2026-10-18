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
    let response = client.recall(query, limit).await?;
    if quiet {
        return Ok(());
    }
    match format {
        ResolvedFormat::Json => print_json(&response)?,
        ResolvedFormat::Table => print!("{}", format_table(&response)),
    }
    Ok(())
}

fn format_table(response: &SearchMemoryResponse) -> String {
    if response.results.is_empty() {
        // A library of pages and no memories yet (every first-run user who
        // only wrote notes) still has pages worth listing.
        let mut output = "(no recalled memories)\n".to_string();
        output.push_str(&super::format_supplemental_pages(
            response.supplemental_pages.as_deref(),
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
    output
}

#[cfg(test)]
mod tests {
    use super::format_table;
    use wenlan_types::responses::SearchMemoryResponse;

    #[test]
    fn lists_compiled_pages_when_no_memory_matched() {
        let response = SearchMemoryResponse {
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
    fn says_so_when_nothing_matched_at_all() {
        let response = SearchMemoryResponse {
            results: Vec::new(),
            took_ms: 3.0,
            supplemental_pages: None,
        };
        assert_eq!(format_table(&response), "(no recalled memories)\n");
    }
}
