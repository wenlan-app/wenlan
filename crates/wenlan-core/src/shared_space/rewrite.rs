// SPDX-License-Identifier: Apache-2.0
//! What a page's sources block may say once it leaves this computer.
//!
//! A page's sources block names the memories and pages it cites
//! (`[1]: [[Title]] · wenlan:<id>@<v>`). A line whose target is a page in the
//! shared Space stays as it is: every member has that page. So does a line
//! citing a memory the person's library shares with their other computers
//! (`kept`). Every other
//! citation becomes a quote: at most three lines of the cited text and
//! "from <member>'s conversation", with no id and no title of anything
//! outside the Space. A basis line to a page outside the Space is dropped.
//! The body is the person's text and is left alone; its `[[links]]` are not
//! resolved or given titles.
//!
//! Rewriting a rewritten block changes nothing, so a version applied on
//! another computer and read back does not come back as a new version.

use std::collections::HashMap;

use crate::store::format::page::{Basis, Citation, Locator, SOURCES_END, SOURCES_START};

/// The locator of a quoted citation. It points at nothing.
pub const QUOTE_LOCATOR: &str = "shared:quote";

/// Lines of the cited text a quote keeps.
const QUOTE_LINES: usize = 3;
/// Characters of each quoted line.
const QUOTE_LINE_CHARS: usize = 200;

/// What the rewrite looks up.
pub struct RewriteContext<'a> {
    /// The shared Space's pages, id to title.
    pub space_pages: &'a HashMap<String, String>,
    /// The text of cited memories, by id, for the quote.
    pub cited_text: &'a HashMap<String, String>,
    /// The name other members know this person by.
    pub member_name: &'a str,
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

fn shorten(line: String) -> String {
    if line.chars().count() <= QUOTE_LINE_CHARS {
        return line;
    }
    let mut short: String = line.chars().take(QUOTE_LINE_CHARS).collect();
    short.push('…');
    short
}

/// `“first / second / third” (from <name>'s conversation)`, or just the
/// part in brackets when there is no text to quote.
pub fn quote_label(cited: Option<&str>, member_name: &str) -> String {
    let lines: Vec<String> = cited
        .unwrap_or_default()
        .lines()
        .map(clean)
        .filter(|line| !line.is_empty())
        .take(QUOTE_LINES)
        .map(shorten)
        .collect();
    let from = format!("from {}'s conversation", clean(member_name));
    if lines.is_empty() {
        format!("({from})")
    } else {
        format!("“{}” ({from})", lines.join(" / "))
    }
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
            if citation.locator == Locator::Other(QUOTE_LOCATOR.to_string()) {
                return citation.clone();
            }
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
            let cited = id.and_then(|id| ctx.cited_text.get(id)).map(String::as_str);
            Citation {
                marker: citation.marker,
                label: quote_label(cited, ctx.member_name),
                locator: Locator::Other(QUOTE_LOCATOR.to_string()),
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
