// SPDX-License-Identifier: Apache-2.0
//! What a page's sources block may say once it leaves this computer.
//!
//! A page's sources block names the memories and pages it cites
//! (`[1]: [[Title]] · wenlan:<id>@<v>`). A line whose target is a page in the
//! shared Space stays as it is: every member has that page. So does a line
//! citing a memory the person's library shares with their other computers
//! (`kept`). Every other citation gets a placeholder with no text or locator
//! from outside the Space. A basis line to a page outside the Space is dropped.
//! The body is the person's text and is left alone; its `[[links]]` are not
//! resolved or given titles.
//!
//! Rewriting a rewritten block changes nothing, so a version applied on
//! another computer and read back does not come back as a new version.

use std::collections::HashMap;

use crate::store::format::page::{Basis, Citation, Locator, PageFile, SOURCES_END, SOURCES_START};

const LEGACY_QUOTE_LOCATOR: &str = "shared:quote";
const PRIVATE_SOURCE_LOCATOR: &str = "shared:private-source";

/// What the rewrite looks up.
pub struct RewriteContext<'a> {
    /// The shared Space's pages, id to title.
    pub space_pages: &'a HashMap<String, String>,
    /// The person's own library: every computer is theirs, so every
    /// citation stays as it is. A source only one computer has (an imported
    /// chat, a file, a web page) then still links back there, and a page
    /// edited on another computer comes back with its sources whole.
    pub keep_all: bool,
}

/// The id a locator points at: `wenlan:<id>@<v>`, or `wenlan:<id>` for a
/// citation made before versions were recorded.
fn target(locator: &Locator) -> Option<&str> {
    match locator {
        Locator::Wenlan { id, .. } => Some(id),
        Locator::Other(text) => text.strip_prefix("wenlan:"),
    }
}

/// `text` on one line, without anything that would end the sources block.
fn clean(text: &str) -> String {
    text.replace(SOURCES_START, " ")
        .replace(SOURCES_END, " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn is_private_placeholder(locator: &Locator) -> bool {
    matches!(locator, Locator::Other(text)
        if text == LEGACY_QUOTE_LOCATOR || text == PRIVATE_SOURCE_LOCATOR)
}

/// A legacy quote has lost the identity needed to verify its publication.
pub(crate) fn has_legacy_quotes(page: &PageFile) -> bool {
    page.citations.iter().any(|citation| {
        matches!(&citation.locator, Locator::Other(text) if text == LEGACY_QUOTE_LOCATOR)
    })
}

fn page_label(title: &str) -> String {
    format!("[[{}]]", clean(title))
}

/// The sources block as it may be shared.
pub fn rewrite_sources(
    citations: &[Citation],
    basis: &[Basis],
    ctx: &RewriteContext<'_>,
) -> (Vec<Citation>, Vec<Basis>) {
    let citations = citations
        .iter()
        .map(|citation| {
            if ctx.keep_all {
                return citation.clone();
            }
            let id = target(&citation.locator);
            if let Some(title) = id.and_then(|id| ctx.space_pages.get(id)) {
                return Citation {
                    marker: citation.marker,
                    label: page_label(title),
                    locator: citation.locator.clone(),
                };
            }
            Citation {
                marker: citation.marker,
                label: "Unavailable source".to_string(),
                locator: Locator::Other(PRIVATE_SOURCE_LOCATOR.to_string()),
            }
        })
        .collect();
    let basis = basis
        .iter()
        .filter_map(|line| {
            let Locator::Wenlan { id, .. } = &line.locator else {
                return None;
            };
            let title = ctx.space_pages.get(id)?;
            Some(Basis {
                label: page_label(title),
                locator: line.locator.clone(),
            })
        })
        .collect();
    (citations, basis)
}
