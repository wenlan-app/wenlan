// SPDX-License-Identifier: Apache-2.0
//! The page file (spec section 3: visible page frontmatter, page body).
//!
//! ```text
//! ---
//! id: page_abc
//! title: Trip planning
//! type: page
//! tags: [Travel]
//! created: 2026-11-01T10:00:00Z
//! modified: 2026-11-01T12:00:00Z
//! ---
//!
//! The part the person reads [1].
//!
//! <!-- wenlan:details -->
//! ## Details for AI
//!
//! The longer part an AI tool reads.
//!
//! <!-- origin:sources:start -->
//! ## Sources
//! [1]: [[Flights]] · wenlan:page_def@3
//! basis: [[Budget]] · wenlan:page_ghi@2
//! <!-- origin:sources:end -->
//! ```
//!
//! Only `id, title, type, tags, created, modified, aliases` are Wenlan's in
//! the frontmatter; who wrote what, versions and review state live in history
//! and the cache. `[N]` markers stay in the body, and the sources block lists
//! one line per marker, then the basis lines.

use super::{normalize_line_ends, render_frontmatter, scalar_text, split_frontmatter, text_list};
use crate::error::WenlanError;

/// The line before the heading that starts the part for AI tools. The
/// heading's text is localized, so the marker is what is matched.
pub const DETAILS_MARKER: &str = "<!-- wenlan:details -->";
/// The heading written under [`DETAILS_MARKER`] when a page has none yet.
pub const DETAILS_HEADING: &str = "Details for AI";
/// The sources block keeps the delimiters today's export writes.
pub const SOURCES_START: &str = "<!-- origin:sources:start -->";
pub const SOURCES_END: &str = "<!-- origin:sources:end -->";

const OWNED_KEYS: &[&str] = &[
    "id", "title", "type", "tags", "created", "modified", "aliases",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    Page,
    Topic,
}

impl PageKind {
    fn as_str(self) -> &'static str {
        match self {
            PageKind::Page => "page",
            PageKind::Topic => "topic",
        }
    }
}

/// What a citation or basis line points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Locator {
    /// `wenlan:<id>@<version>`: a page, memory or original at one version.
    Wenlan { id: String, version: i64 },
    /// Anything else, kept as written (`repo://…`, `url:…`).
    Other(String),
}

impl Locator {
    fn parse(text: &str) -> Self {
        if let Some((id, version)) = text
            .strip_prefix("wenlan:")
            .and_then(|rest| rest.rsplit_once('@'))
        {
            if let Ok(version) = version.parse() {
                if !id.is_empty() && !id.contains(char::is_whitespace) {
                    return Locator::Wenlan {
                        id: id.to_string(),
                        version,
                    };
                }
            }
        }
        Locator::Other(text.to_string())
    }

    fn render(&self) -> String {
        match self {
            Locator::Wenlan { id, version } => format!("wenlan:{id}@{version}"),
            Locator::Other(text) => text.clone(),
        }
    }
}

/// `[N]: <label> · <locator>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Citation {
    pub marker: u32,
    /// What a reader sees, usually `[[Title]]`.
    pub label: String,
    pub locator: Locator,
}

/// `basis: <label> · <locator>`: a page this page relies on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Basis {
    pub label: String,
    pub locator: Locator,
}

/// The part for AI tools, under its heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Details {
    pub heading: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PageFile {
    /// `None` for a file someone dropped in that Wenlan has not seen yet.
    pub id: Option<String>,
    pub title: String,
    pub kind: PageKind,
    pub tags: Vec<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
    pub aliases: Vec<String>,
    /// Frontmatter keys that are not Wenlan's, in their order.
    pub other: serde_yaml::Mapping,
    /// The part the person reads.
    pub body: String,
    pub details: Option<Details>,
    pub citations: Vec<Citation>,
    pub basis: Vec<Basis>,
}

impl PageFile {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            id: None,
            title: title.into(),
            kind: PageKind::Page,
            tags: Vec::new(),
            created: None,
            modified: None,
            aliases: Vec::new(),
            other: serde_yaml::Mapping::new(),
            body: body.into(),
            details: None,
            citations: Vec::new(),
            basis: Vec::new(),
        }
    }
}

/// Reads a page file. `fallback_title` (the file's stem) is used when the
/// frontmatter names no title, as for a file someone made in Obsidian.
pub fn parse(text: &str, fallback_title: &str) -> Result<PageFile, WenlanError> {
    let text = normalize_line_ends(text);
    let (mut frontmatter, body) = match split_frontmatter(&text)? {
        Some(split) => (
            split.frontmatter,
            split.body.strip_prefix('\n').unwrap_or(split.body),
        ),
        None => (serde_yaml::Mapping::new(), text.as_ref()),
    };

    let mut page = PageFile::new(fallback_title, "");
    let mut take = |key: &str| frontmatter.shift_remove(key);
    let mut kept = serde_yaml::Mapping::new();
    for key in OWNED_KEYS {
        let Some(value) = take(key) else { continue };
        let understood = match *key {
            "id" => scalar_text(&value).map(|id| page.id = Some(id)),
            "title" => scalar_text(&value).map(|title| page.title = title),
            "type" => match scalar_text(&value).as_deref() {
                Some("page") => Some(PageKind::Page),
                Some("topic") => Some(PageKind::Topic),
                _ => None,
            }
            .map(|kind| page.kind = kind),
            "tags" => text_list(&value).map(|tags| page.tags = tags),
            "created" => scalar_text(&value).map(|at| page.created = Some(at)),
            "modified" => scalar_text(&value).map(|at| page.modified = Some(at)),
            "aliases" => text_list(&value).map(|aliases| page.aliases = aliases),
            _ => None,
        };
        // A value Wenlan cannot read stays as it was written.
        if understood.is_none() {
            kept.insert(serde_yaml::Value::String(key.to_string()), value);
        }
    }
    kept.extend(frontmatter);
    page.other = kept;

    let (rest, citations, basis) = split_sources(body);
    page.citations = citations;
    page.basis = basis;
    let (human, details) = split_details(rest);
    page.body = human.trim_end_matches('\n').to_string();
    page.details = details;
    Ok(page)
}

/// Writes a page file.
pub fn render(page: &PageFile) -> String {
    let mut frontmatter = serde_yaml::Mapping::new();
    let mut put = |key: &str, value: serde_yaml::Value| {
        frontmatter.insert(serde_yaml::Value::String(key.to_string()), value);
    };
    let text = |text: &str| serde_yaml::Value::String(text.to_string());
    let list = |items: &[String]| {
        serde_yaml::Value::Sequence(items.iter().map(|item| text(item)).collect())
    };
    if let Some(id) = &page.id {
        put("id", text(id));
    }
    put("title", text(&page.title));
    put("type", text(page.kind.as_str()));
    put("tags", list(&page.tags));
    if let Some(created) = &page.created {
        put("created", text(created));
    }
    if let Some(modified) = &page.modified {
        put("modified", text(modified));
    }
    if !page.aliases.is_empty() {
        put("aliases", list(&page.aliases));
    }
    for (key, value) in &page.other {
        if !frontmatter.contains_key(key) {
            frontmatter.insert(key.clone(), value.clone());
        }
    }

    let mut out = render_frontmatter(&frontmatter);
    out.push('\n');
    out.push_str(page.body.trim_end_matches('\n'));
    out.push('\n');
    if let Some(details) = &page.details {
        out.push('\n');
        out.push_str(DETAILS_MARKER);
        out.push_str("\n## ");
        out.push_str(&details.heading);
        out.push_str("\n\n");
        let text = details.text.trim_end_matches('\n');
        if !text.is_empty() {
            out.push_str(text);
            out.push('\n');
        }
    }
    if !page.citations.is_empty() || !page.basis.is_empty() {
        out.push('\n');
        out.push_str(SOURCES_START);
        out.push_str("\n## Sources\n");
        for citation in &page.citations {
            out.push_str(&format!(
                "[{}]: {} · {}\n",
                citation.marker,
                citation.label,
                citation.locator.render()
            ));
        }
        for basis in &page.basis {
            out.push_str(&format!(
                "basis: {} · {}\n",
                basis.label,
                basis.locator.render()
            ));
        }
        out.push_str(SOURCES_END);
        out.push('\n');
    }
    out
}

/// Takes the sources block off the end of `body`. A block with a line that
/// is neither a citation nor a basis line is not Wenlan's and stays in the
/// body.
fn split_sources(body: &str) -> (&str, Vec<Citation>, Vec<Basis>) {
    let trimmed = body.trim_end();
    let Some(inner_end) = trimmed.strip_suffix(SOURCES_END) else {
        return (body, Vec::new(), Vec::new());
    };
    let Some(start) = inner_end.rfind(SOURCES_START) else {
        return (body, Vec::new(), Vec::new());
    };
    let inner = &inner_end[start + SOURCES_START.len()..];
    let mut citations = Vec::new();
    let mut basis = Vec::new();
    for line in inner.lines() {
        let line = line.trim_end();
        if line.is_empty() || line == "## Sources" {
            continue;
        }
        if let Some(rest) = line.strip_prefix("basis: ") {
            let Some((label, locator)) = rest.rsplit_once(" · ") else {
                return (body, Vec::new(), Vec::new());
            };
            basis.push(Basis {
                label: label.to_string(),
                locator: Locator::parse(locator),
            });
            continue;
        }
        let parsed = line
            .strip_prefix('[')
            .and_then(|rest| rest.split_once("]: "))
            .and_then(|(marker, rest)| {
                let marker = marker.parse().ok()?;
                let (label, locator) = rest.rsplit_once(" · ")?;
                Some(Citation {
                    marker,
                    label: label.to_string(),
                    locator: Locator::parse(locator),
                })
            });
        match parsed {
            Some(citation) => citations.push(citation),
            None => return (body, Vec::new(), Vec::new()),
        }
    }
    let before = &body[..start];
    (
        before.strip_suffix('\n').unwrap_or(before),
        citations,
        basis,
    )
}

/// Splits the body at the details marker: the marker line, then a `## `
/// heading line.
fn split_details(body: &str) -> (&str, Option<Details>) {
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        if line.trim_end() == DETAILS_MARKER {
            let after = &body[offset + line.len()..];
            let (heading_line, rest) = after.split_once('\n').unwrap_or((after, ""));
            if let Some(heading) = heading_line.strip_prefix("## ") {
                let human = &body[..offset];
                let text = rest.strip_prefix('\n').unwrap_or(rest);
                return (
                    human.strip_suffix('\n').unwrap_or(human),
                    Some(Details {
                        heading: heading.trim_end().to_string(),
                        text: text.trim_end_matches('\n').to_string(),
                    }),
                );
            }
        }
        offset += line.len();
    }
    (body, None)
}
