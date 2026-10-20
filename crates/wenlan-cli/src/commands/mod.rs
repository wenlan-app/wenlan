// SPDX-License-Identifier: Apache-2.0
//! Subcommand implementations for the origin CLI.

pub mod agents;
pub mod brief;
pub mod curate;
pub mod entities;
pub mod export;
pub mod ingest;
pub mod lint;
pub mod list;
pub mod mcp;
pub mod outbox;
pub mod pages;
pub mod recall;
pub mod search;
pub mod service;
pub mod setup;
pub mod space;
pub mod status;
pub mod store;
pub mod sweep;

#[cfg(not(target_os = "windows"))]
pub use service::service_unit_path;
pub use service::{install, is_installed, restart, SERVICE_LABEL};

/// Render the `Compiled pages:` section that `search` and `recall` share.
/// Empty or absent pages render nothing.
pub(crate) fn format_supplemental_pages(
    pages: Option<&[wenlan_types::memory::SearchResult]>,
) -> String {
    let Some(pages) = pages.filter(|pages| !pages.is_empty()) else {
        return String::new();
    };
    let mut output = String::from("Compiled pages:\n");
    for page in pages {
        let title = if page.title.is_empty() {
            page.content.lines().next().unwrap_or("(page)")
        } else {
            &page.title
        };
        output.push_str(&format!("  - {} ({})\n", title, page.source_id));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::format_supplemental_pages;
    use wenlan_types::memory::SearchResult;

    pub(super) fn page(title: &str, source_id: &str) -> SearchResult {
        SearchResult {
            basis: Vec::new(),
            code_basis: Vec::new(),
            id: "p1".to_string(),
            content: "First page line\nSecond page line".to_string(),
            source: "page".to_string(),
            source_id: source_id.to_string(),
            title: title.to_string(),
            url: None,
            chunk_index: 0,
            last_modified: 0,
            score: 0.9,
            chunk_type: None,
            language: None,
            semantic_unit: None,
            memory_type: None,
            space: None,
            source_agent: None,
            confidence: None,
            confirmed: None,
            stability: None,
            supersedes: None,
            summary: None,
            entity_id: None,
            entity_name: None,
            quality: None,
            importance: None,
            event_date: None,
            is_archived: false,
            is_recap: false,
            structured_fields: None,
            retrieval_cue: None,
            source_text: None,
            content_hash: None,
            raw_score: 0.0,
            version: 0,
            pending_revision: false,
            merged_from: None,
            last_delta_summary: None,
        }
    }

    #[test]
    fn renders_supplemental_pages_section() {
        let pages = [page("Compiled page", "page-source-1")];
        let output = format_supplemental_pages(Some(&pages));
        assert!(output.starts_with("Compiled pages:\n"), "{output}");
        assert!(output.contains("Compiled page (page-source-1)"), "{output}");
    }

    #[test]
    fn falls_back_to_first_content_line_without_title() {
        let pages = [page("", "page-source-2")];
        let output = format_supplemental_pages(Some(&pages));
        assert!(
            output.contains("First page line (page-source-2)"),
            "{output}"
        );
    }

    #[test]
    fn absent_or_empty_pages_render_nothing() {
        assert_eq!(format_supplemental_pages(None), "");
        assert_eq!(format_supplemental_pages(Some(&[])), "");
    }
}
