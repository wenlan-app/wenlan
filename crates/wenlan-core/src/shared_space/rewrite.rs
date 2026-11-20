// SPDX-License-Identifier: Apache-2.0
//! What a page's sources block may say once it leaves this computer.
//!
//! A page's sources block names the memories and pages it cites
//! (`[1]: [[Title]] · wenlan:<id>@<v>`). A line whose target is a page in the
//! shared Space stays as it is: every member has that page. So does a line
//! citing a memory the person's library shares with their other computers
//! (`kept`). Every other citation and its body marker are omitted. A basis line
//! to a page outside the Space is dropped. The rest of the body is the person's
//! text; its `[[links]]` are not resolved or given titles.
//!
//! Rewriting a rewritten block changes nothing, so a version applied on
//! another computer and read back does not come back as a new version.

use std::collections::{HashMap, HashSet};

use crate::store::format::page::{Basis, Citation, Locator, SOURCES_END, SOURCES_START};

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

fn is_shared_page(id: &str, ctx: &RewriteContext<'_>) -> bool {
    ctx.space_pages.contains_key(id) && !crate::document_enrichment::is_source_page_id(id)
}

fn citation_is_shared(citation: &Citation, ctx: &RewriteContext<'_>) -> bool {
    ctx.keep_all || target(&citation.locator).is_some_and(|id| is_shared_page(id, ctx))
}

fn shared_marker_map(citations: &[Citation], ctx: &RewriteContext<'_>) -> HashMap<u32, u32> {
    let mut visible: Vec<u32> = citations
        .iter()
        .filter(|citation| citation_is_shared(citation, ctx))
        .map(|citation| citation.marker)
        .collect();
    visible.sort_unstable();
    visible.dedup();
    visible
        .into_iter()
        .enumerate()
        .map(|(index, marker)| (marker, index as u32 + 1))
        .collect()
}

/// Removes references that would otherwise disclose the number of omitted
/// private citations and renumbers the remaining shared-page markers.
pub(crate) fn rewrite_body(body: &str, citations: &[Citation], ctx: &RewriteContext<'_>) -> String {
    if ctx.keep_all || citations.is_empty() {
        return body.to_string();
    }
    let visible = shared_marker_map(citations, ctx);
    let known: HashSet<u32> = citations.iter().map(|citation| citation.marker).collect();
    if visible.len() == known.len() && visible.iter().all(|(old, new)| old == new) {
        return body.to_string();
    }

    let mut out = String::with_capacity(body.len());
    let mut cursor = 0;
    for (start, end) in crate::citations::marker_spans(body) {
        let Some(marker) = body[start + 1..end - 1].parse::<u32>().ok() else {
            continue;
        };
        if !known.contains(&marker) {
            continue;
        }
        out.push_str(&body[cursor..start]);
        cursor = end;
        if let Some(new_marker) = visible.get(&marker) {
            out.push('[');
            out.push_str(&new_marker.to_string());
            out.push(']');
        } else if out.ends_with(' ') {
            match body[end..].chars().next() {
                Some(' ') => cursor += 1,
                None | Some('.' | ',' | ';' | ':' | '!' | '?' | '\n' | '\r') => {
                    out.pop();
                }
                _ => {}
            }
        }
    }
    out.push_str(&body[cursor..]);
    out
}

/// Legacy quotes and placeholders carry no target that can be verified.
#[cfg(test)]
pub(crate) fn has_unshared_references(
    page: &crate::store::format::page::PageFile,
    shared_page_ids: &HashSet<String>,
) -> bool {
    page.citations
        .iter()
        .map(|citation| &citation.locator)
        .chain(page.basis.iter().map(|line| &line.locator))
        .any(|locator| target(locator).is_none_or(|id| !shared_page_ids.contains(id)))
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
    let markers = shared_marker_map(citations, ctx);
    let citations = citations
        .iter()
        .filter_map(|citation| {
            if ctx.keep_all {
                return Some(citation.clone());
            }
            let id = target(&citation.locator);
            if let Some(title) = id
                .filter(|id| is_shared_page(id, ctx))
                .and_then(|id| ctx.space_pages.get(id))
            {
                return Some(Citation {
                    marker: markers[&citation.marker],
                    label: page_label(title),
                    locator: citation.locator.clone(),
                });
            }
            None
        })
        .collect();
    let basis = basis
        .iter()
        .filter_map(|line| {
            let id = target(&line.locator)?;
            if !is_shared_page(id, ctx) {
                return None;
            }
            let title = ctx.space_pages.get(id)?;
            Some(Basis {
                label: page_label(title),
                locator: line.locator.clone(),
            })
        })
        .collect();
    (citations, basis)
}
