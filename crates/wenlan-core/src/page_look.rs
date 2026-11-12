// SPDX-License-Identifier: Apache-2.0
//! The data a page's look reads, and the checks a look passes to be saved
//! (UX-LOOK-1..6).
//!
//! [`look_data`] builds `window.wenlan` for one page from what the daemon
//! already holds: the part of the body meant for people (never "Details for
//! AI", never a Sources section), cut into sections with their list items and
//! tables, the page's links, and the page list of its Space. One function
//! feeds the app's frame and the MCP tool that shows an AI the same data, so
//! the AI sees exactly what the look will get.
//!
//! [`check_look`] runs when a look is saved. It refuses a look that reaches
//! for the internet or that copies the page's text into the HTML, with a
//! reason an AI tool can act on.
//!
//! Rules held by tests:
//! - Raw HTML in the body is dropped; links are reduced to their text. A
//!   `[[link]]` to a page becomes `<a data-wenlan-page="id">`. A picture in
//!   the pages folder becomes `<img data-wenlan-image="n">` listed in
//!   `page.images`; a picture on the web keeps its alt text.
//! - Arrays are never null; the JSON shape is pinned by a golden fixture.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use regex::Regex;
use wenlan_types::page_look::{
    LookData, LookImage, LookItem, LookLink, LookPageData, LookSection, LookSpaceData,
    LookSpacePage, LookTable, LookTheme, LOOK_API, LOOK_SPACE_PAGES_MAX,
};
use wenlan_types::pages::Page;

/// Headings whose section is left out, as the mind map leaves them out.
const SKIPPED_HEADINGS: &[&str] = &[
    "sources",
    "來源",
    "来源",
    "fuentes",
    "references",
    "參考資料",
    "参考资料",
];

/// Largest look HTML a page may keep.
pub const LOOK_MAX_BYTES: usize = 1_000_000;

/// Longest summary `space.pages` carries per page, in characters.
const SUMMARY_CHARS: usize = 240;

static WIKILINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(!?)\[\[([^\]|#]+)(?:#([^\]|]+))?(?:\|([^\]]+))?\]\]").unwrap());

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH
}

/// How a `[[label]]` resolves: label as written (lowercased, trimmed) to the
/// id of the page it names. Built from the page's outbound link rows.
pub type LinkTargets = HashMap<String, String>;

/// Build the label map from `(label, target page id)` rows.
pub fn look_link_targets<'a>(
    rows: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> LinkTargets {
    rows.into_iter()
        .filter_map(|(label, id)| Some((label.trim().to_lowercase(), id?.to_string())))
        .collect()
}

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// `window.wenlan` for `page`. `space_pages` is the Space's pages newest
/// first (at most [`LOOK_SPACE_PAGES_MAX`] are used); `more` is true when the
/// Space has pages beyond the ones given.
pub fn look_data(
    page: &Page,
    targets: &LinkTargets,
    space_name: &str,
    space_pages: &[Page],
    more: bool,
) -> LookData {
    let human = human_text(&page.content);
    let segments = segments(human);
    let kept: Vec<&Segment> = segments.iter().filter(|s| !s.skipped).collect();

    let text = kept
        .iter()
        .map(|s| s.source.as_str())
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string();
    // The whole text first, so every picture has its place before a section
    // names it.
    let mut images: Vec<LookImage> = Vec::new();
    let html = render_html(&text, targets, &mut images);

    let sections = kept
        .iter()
        .filter(|s| s.level > 0 || !s.body.trim().is_empty())
        .map(|s| {
            let body = s.body.trim();
            let (items, tables) = analyze(body);
            LookSection {
                heading: s.heading.clone(),
                level: s.level,
                text: body.to_string(),
                html: render_html(body, targets, &mut images),
                items,
                tables,
            }
        })
        .collect();

    let links = crate::synthesis::wikilinks::extract_wikilinks(&text)
        .into_iter()
        .map(|title| LookLink {
            id: targets.get(&title.trim().to_lowercase()).cloned(),
            title,
        })
        .collect();

    let listed = space_pages.len().min(LOOK_SPACE_PAGES_MAX);
    LookData {
        api: LOOK_API,
        page: LookPageData {
            id: page.id.clone(),
            title: page.title.clone(),
            space: space_name.to_string(),
            created: page.created_at.clone(),
            modified: page.last_modified.clone(),
            text,
            html,
            sections,
            links,
            images,
        },
        space: LookSpaceData {
            name: space_name.to_string(),
            pages: space_pages
                .iter()
                .take(LOOK_SPACE_PAGES_MAX)
                .map(|p| LookSpacePage {
                    id: p.id.clone(),
                    title: p.title.clone(),
                    created: p.created_at.clone(),
                    modified: p.last_modified.clone(),
                    summary: page_summary(p),
                })
                .collect(),
            more: more || space_pages.len() > listed,
        },
        theme: LookTheme {
            mode: "dark".to_string(),
        },
    }
}

fn page_summary(page: &Page) -> String {
    let summary = page
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let human = human_text(&page.content);
            crate::page_body::short_part(human).trim().to_string()
        });
    let plain = plain_text(&summary).replace('\n', " ").trim().to_string();
    let mut out: String = plain.chars().take(SUMMARY_CHARS).collect();
    if plain.chars().count() > SUMMARY_CHARS {
        out.push('…');
    }
    out
}

/// The part of the body for people: no frontmatter, nothing from the details
/// marker on.
fn human_text(content: &str) -> &str {
    crate::page_body::human_part(crate::page_outline::strip_frontmatter(content))
}

struct Segment {
    heading: String,
    level: u8,
    /// The Markdown of the heading and what follows it, as written.
    source: String,
    /// What follows the heading line.
    body: String,
    skipped: bool,
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Cut `human` at its headings.
fn segments(human: &str) -> Vec<Segment> {
    // (start, end, level, text) of every heading.
    let mut heads: Vec<(usize, usize, u8, String)> = Vec::new();
    let mut open: Option<(usize, u8, String)> = None;
    for (event, range) in Parser::new_ext(human, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                open = Some((range.start, heading_level(level), String::new()));
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, _, text)) = open.as_mut() {
                    text.push_str(&t);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((start, level, text)) = open.take() {
                    heads.push((start, range.end, level, strip_links(&text)));
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    let first = heads.first().map(|h| h.0).unwrap_or(human.len());
    if !human[..first].trim().is_empty() {
        out.push(Segment {
            heading: String::new(),
            level: 0,
            source: human[..first].to_string(),
            body: human[..first].to_string(),
            skipped: false,
        });
    }
    let mut skipping: Option<u8> = None;
    for (i, (start, end, level, text)) in heads.iter().enumerate() {
        let next = heads.get(i + 1).map(|h| h.0).unwrap_or(human.len());
        if let Some(skip_level) = skipping {
            if *level > skip_level {
                out.push(Segment {
                    heading: text.clone(),
                    level: *level,
                    source: String::new(),
                    body: String::new(),
                    skipped: true,
                });
                continue;
            }
            skipping = None;
        }
        let skipped = SKIPPED_HEADINGS.contains(&normalize(text).as_str());
        if skipped {
            skipping = Some(*level);
        }
        out.push(Segment {
            heading: text.clone(),
            level: *level,
            source: human[*start..next].to_string(),
            body: human[(*end).min(next)..next].to_string(),
            skipped,
        });
    }
    out
}

/// `text` with every `[[link]]` replaced by what a reader sees.
fn strip_links(text: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    for cap in WIKILINK_RE.captures_iter(text) {
        let whole = cap.get(0).unwrap();
        out.push_str(&text[last..whole.start()]);
        if &cap[1] == "!" {
            out.push_str(whole.as_str());
        } else {
            out.push_str(link_display(&cap).trim());
        }
        last = whole.end();
    }
    out.push_str(&text[last..]);
    out
}

fn link_display<'a>(cap: &regex::Captures<'a>) -> &'a str {
    cap.get(4)
        .or_else(|| cap.get(2))
        .map(|m| m.as_str())
        .unwrap_or("")
}

/// The list items and tables in one section's Markdown.
fn analyze(md: &str) -> (Vec<LookItem>, Vec<LookTable>) {
    let mut items: Vec<LookItem> = Vec::new();
    let mut open_items: Vec<usize> = Vec::new();
    let mut tables: Vec<LookTable> = Vec::new();
    let mut table: Option<LookTable> = None;
    let mut row: Vec<String> = Vec::new();
    let mut cell: Option<String> = None;
    let mut in_head = false;
    let mut in_code = false;

    for event in Parser::new_ext(md, options()) {
        match event {
            Event::Start(Tag::Item) => {
                items.push(LookItem {
                    text: String::new(),
                    done: None,
                });
                open_items.push(items.len() - 1);
            }
            Event::End(TagEnd::Item) => {
                if let Some(i) = open_items.pop() {
                    items[i].text = strip_links(items[i].text.trim());
                }
            }
            Event::TaskListMarker(done) => {
                if let Some(i) = open_items.last() {
                    items[*i].done = Some(done);
                }
            }
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(TagEnd::CodeBlock) => in_code = false,
            Event::Start(Tag::Table(_)) => {
                table = Some(LookTable {
                    columns: Vec::new(),
                    rows: Vec::new(),
                });
            }
            Event::End(TagEnd::Table) => {
                if let Some(t) = table.take() {
                    tables.push(t);
                }
            }
            Event::Start(Tag::TableHead) => {
                in_head = true;
                row = Vec::new();
            }
            Event::End(TagEnd::TableHead) => {
                in_head = false;
                if let Some(t) = table.as_mut() {
                    t.columns = std::mem::take(&mut row);
                }
            }
            Event::Start(Tag::TableRow) => row = Vec::new(),
            Event::End(TagEnd::TableRow) => {
                if let Some(t) = table.as_mut() {
                    if !in_head {
                        t.rows.push(std::mem::take(&mut row));
                    }
                }
            }
            Event::Start(Tag::TableCell) => cell = Some(String::new()),
            Event::End(TagEnd::TableCell) => {
                if let Some(text) = cell.take() {
                    row.push(strip_links(text.trim()));
                }
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some(c) = cell.as_mut() {
                    c.push_str(&t);
                } else if !in_code {
                    if let Some(i) = open_items.last() {
                        items[*i].text.push_str(&t);
                    }
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(c) = cell.as_mut() {
                    c.push(' ');
                } else if let Some(i) = open_items.last() {
                    items[*i].text.push(' ');
                }
            }
            _ => {}
        }
    }
    items.retain(|item| !item.text.is_empty());
    (items, tables)
}

fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

/// Markdown to HTML for a look. Raw HTML is dropped; links and images are
/// reduced to their text; a `[[link]]` becomes an anchor naming the page.
fn render_html(md: &str, targets: &LinkTargets, images: &mut Vec<LookImage>) -> String {
    // Adjacent text events are one run, so a `[[link]]` pulldown split at its
    // brackets is found whole.
    let mut events: Vec<Event> = Vec::new();
    let mut run = String::new();
    let mut in_code = false;
    let flush = |run: &mut String, events: &mut Vec<Event>| {
        if !run.is_empty() {
            events.push(Event::Text(std::mem::take(run).into()));
        }
    };
    for event in Parser::new_ext(md, options()) {
        match event {
            Event::Text(t) if !in_code => run.push_str(&t),
            other => {
                flush(&mut run, &mut events);
                match other {
                    Event::Start(Tag::CodeBlock(_)) => in_code = true,
                    Event::End(TagEnd::CodeBlock) => in_code = false,
                    _ => {}
                }
                events.push(other);
            }
        }
    }
    flush(&mut run, &mut events);

    let mut out = String::new();
    let mut in_code = false;
    let mut in_head = false;
    let mut body_open = false;
    // A picture in the pages folder being read: its source and alt so far.
    let mut picture: Option<(String, String)> = None;
    for event in events {
        if let Some((_, alt)) = picture.as_mut() {
            match &event {
                Event::Text(t) | Event::Code(t) => {
                    alt.push_str(t);
                    continue;
                }
                Event::End(TagEnd::Image) => {
                    let (src, alt) = picture.take().unwrap_or_default();
                    push_image(&src, alt.trim(), images, &mut out);
                    continue;
                }
                _ => continue,
            }
        }
        match event {
            Event::Start(Tag::Image { dest_url, .. }) if is_folder_picture(&dest_url) => {
                picture = Some((
                    dest_url.trim().trim_matches(['<', '>']).to_string(),
                    String::new(),
                ));
            }
            Event::Start(tag) => match tag {
                Tag::Paragraph => out.push_str("<p>"),
                Tag::Heading { level, .. } => {
                    out.push_str(&format!("<h{}>", heading_level(level)));
                }
                Tag::BlockQuote(_) => out.push_str("<blockquote>\n"),
                Tag::CodeBlock(_) => {
                    in_code = true;
                    out.push_str("<pre><code>");
                }
                Tag::List(None) => out.push_str("<ul>\n"),
                Tag::List(Some(1)) => out.push_str("<ol>\n"),
                Tag::List(Some(n)) => out.push_str(&format!("<ol start=\"{n}\">\n")),
                Tag::Item => out.push_str("<li>"),
                Tag::Emphasis => out.push_str("<em>"),
                Tag::Strong => out.push_str("<strong>"),
                Tag::Strikethrough => out.push_str("<del>"),
                Tag::Table(_) => {
                    body_open = false;
                    out.push_str("<table>\n");
                }
                Tag::TableHead => {
                    in_head = true;
                    out.push_str("<thead><tr>");
                }
                Tag::TableRow => {
                    if !body_open {
                        body_open = true;
                        out.push_str("<tbody>\n");
                    }
                    out.push_str("<tr>");
                }
                Tag::TableCell => out.push_str(if in_head { "<th>" } else { "<td>" }),
                // Links and pictures on the web keep only their text.
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => out.push_str("</p>\n"),
                TagEnd::Heading(level) => {
                    out.push_str(&format!("</h{}>\n", heading_level(level)));
                }
                TagEnd::BlockQuote(_) => out.push_str("</blockquote>\n"),
                TagEnd::CodeBlock => {
                    in_code = false;
                    out.push_str("</code></pre>\n");
                }
                TagEnd::List(false) => out.push_str("</ul>\n"),
                TagEnd::List(true) => out.push_str("</ol>\n"),
                TagEnd::Item => out.push_str("</li>\n"),
                TagEnd::Emphasis => out.push_str("</em>"),
                TagEnd::Strong => out.push_str("</strong>"),
                TagEnd::Strikethrough => out.push_str("</del>"),
                TagEnd::Table => {
                    if body_open {
                        out.push_str("</tbody>\n");
                    }
                    out.push_str("</table>\n");
                }
                TagEnd::TableHead => {
                    in_head = false;
                    out.push_str("</tr></thead>\n");
                }
                TagEnd::TableRow => out.push_str("</tr>\n"),
                TagEnd::TableCell => out.push_str(if in_head { "</th>" } else { "</td>" }),
                _ => {}
            },
            Event::Text(t) => {
                if in_code {
                    escape(&t, &mut out);
                } else {
                    push_text_with_links(&t, targets, images, &mut out);
                }
            }
            Event::Code(t) => {
                out.push_str("<code>");
                escape(&t, &mut out);
                out.push_str("</code>");
            }
            Event::SoftBreak => out.push('\n'),
            Event::HardBreak => out.push_str("<br>\n"),
            Event::Rule => out.push_str("<hr>\n"),
            Event::TaskListMarker(done) => {
                out.push_str(if done {
                    "<input type=\"checkbox\" disabled checked> "
                } else {
                    "<input type=\"checkbox\" disabled> "
                });
            }
            // Raw HTML in a page never reaches a look.
            Event::Html(_) | Event::InlineHtml(_) => {}
            _ => {}
        }
    }
    out.trim_end().to_string()
}

fn push_text_with_links(
    text: &str,
    targets: &LinkTargets,
    images: &mut Vec<LookImage>,
    out: &mut String,
) {
    let mut last = 0;
    for cap in WIKILINK_RE.captures_iter(text) {
        let whole = cap.get(0).unwrap();
        escape(&text[last..whole.start()], out);
        last = whole.end();
        if &cap[1] == "!" {
            let name = cap[2].trim();
            if is_picture_name(name) {
                // `![[shot.png|240]]` sets a width, any other `|text` is the alt.
                let alt = cap
                    .get(4)
                    .map(|m| m.as_str().trim())
                    .filter(|o| !o.chars().all(|c| c.is_ascii_digit() || c == 'x'))
                    .unwrap_or_else(|| name.rsplit('/').next().unwrap_or(name));
                push_image(name, alt, images, out);
            } else {
                escape(whole.as_str(), out);
            }
            continue;
        }
        let display = link_display(&cap).trim();
        match targets.get(&cap[2].trim().to_lowercase()) {
            Some(id) => {
                out.push_str("<a data-wenlan-page=\"");
                escape(id, out);
                out.push_str("\">");
                escape(display, out);
                out.push_str("</a>");
            }
            None => escape(display, out),
        }
    }
    escape(&text[last..], out);
}

const PICTURE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "heic"];

/// Whether a file name is a picture a look can show.
fn is_picture_name(name: &str) -> bool {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    file.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && PICTURE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
    })
}

/// Whether `![alt](dest)` names a picture in the pages folder, not the web.
fn is_folder_picture(dest: &str) -> bool {
    let dest = dest.trim().trim_matches(['<', '>']);
    !dest.is_empty() && !is_external(dest) && !dest.contains(':') && is_picture_name(dest)
}

/// `<img data-wenlan-image="n">` for the picture at `src`, listing it once.
fn push_image(src: &str, alt: &str, images: &mut Vec<LookImage>, out: &mut String) {
    let index = match images.iter().position(|image| image.src == src) {
        Some(index) => index,
        None => {
            images.push(LookImage {
                src: src.to_string(),
                alt: alt.to_string(),
            });
            images.len() - 1
        }
    };
    out.push_str(&format!("<img data-wenlan-image=\"{index}\" alt=\""));
    escape(alt, out);
    out.push_str("\">");
}

/// Plain text of Markdown: words only, one block per line.
fn plain_text(md: &str) -> String {
    let mut out = String::new();
    for event in Parser::new_ext(md, options()) {
        match event {
            Event::Text(t) | Event::Code(t) => out.push_str(&t),
            Event::SoftBreak | Event::HardBreak => out.push(' '),
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::TableCell
                | TagEnd::TableRow
                | TagEnd::CodeBlock,
            ) => out.push('\n'),
            _ => {}
        }
    }
    strip_links(&out)
}

// ---------------------------------------------------------------------------
// Save-time checks
// ---------------------------------------------------------------------------

/// A sentence this long, copied literally, counts as copied content.
const COPIED_SENTENCE_CHARS: usize = 20;
/// This many copied sentences refuse the look.
const COPIED_SENTENCES_REFUSED: usize = 3;

static ATTR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?is)(?:^|[^\w:-])(src|href|action|formaction|poster|data|xlink:href)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#,
    )
    .unwrap()
});
static SRCSET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)\bsrcset\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).unwrap()
});
static SCRIPT_SRC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<script\b[^>]*?\bsrc\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).unwrap()
});
static BASE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<base[\s>/]").unwrap());
static IMPORT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)@import\s*(?:url\(\s*)?["']?\s*(?:https?:)?//"#).unwrap());
static CSS_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)url\(\s*["']?\s*(?:https?:)?//"#).unwrap());
static REFRESH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<meta\b[^>]*http-equiv\s*=\s*["']?\s*refresh"#).unwrap());

fn is_external(value: &str) -> bool {
    let v = value.trim().to_ascii_lowercase();
    v.starts_with("http://") || v.starts_with("https://") || v.starts_with("//")
}

fn first_group(cap: &regex::Captures<'_>, from: usize) -> String {
    (from..cap.len())
        .find_map(|i| cap.get(i))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

const EXTERNAL_REASON: &str = "This look loads something from the internet. Looks cannot reach \
the internet, so nothing may come from a web address. Put styles in <style>, scripts inline in \
<script>, draw pictures as inline SVG, and embed any font or image as a data: URL.";

/// Whether `html` may be kept as the look of a page whose body is
/// `page_content`. The error is a plain-English reason an AI tool can act on.
pub fn check_look(html: &str, page_content: &str) -> Result<(), String> {
    if html.trim().is_empty() {
        return Err("The look is empty. Write the HTML of the look.".to_string());
    }
    if html.len() > LOOK_MAX_BYTES {
        return Err(format!(
            "The look is {} KB, over the {} KB limit. Make it smaller: no embedded fonts or \
             images that are not needed.",
            html.len() / 1000,
            LOOK_MAX_BYTES / 1000
        ));
    }

    if BASE_RE.is_match(html) {
        return Err(format!(
            "The look has a <base> tag. {EXTERNAL_REASON} Remove the <base> tag."
        ));
    }
    if REFRESH_RE.is_match(html) {
        return Err(
            "The look has a <meta http-equiv=\"refresh\"> tag, which navigates away. Remove it."
                .to_string(),
        );
    }
    if IMPORT_RE.is_match(html) || CSS_URL_RE.is_match(html) {
        return Err(format!(
            "The look's CSS loads a file from the internet with @import or url(). {EXTERNAL_REASON}"
        ));
    }
    for cap in SCRIPT_SRC_RE.captures_iter(html) {
        let value = first_group(&cap, 1);
        if !value.trim().to_ascii_lowercase().starts_with("data:") {
            return Err(format!(
                "The look has a <script src=\"{}\">. Looks cannot load script files. Write the \
                 script inline between <script> and </script>.",
                value.chars().take(80).collect::<String>()
            ));
        }
    }
    for cap in ATTR_RE.captures_iter(html) {
        let value = first_group(&cap, 2);
        if is_external(&value) {
            return Err(format!(
                "The look has {}=\"{}\", a web address. {EXTERNAL_REASON}",
                cap[1].to_ascii_lowercase(),
                value.chars().take(80).collect::<String>()
            ));
        }
    }
    for cap in SRCSET_RE.captures_iter(html) {
        let value = first_group(&cap, 1);
        if value
            .split(',')
            .any(|candidate| is_external(candidate.trim()))
        {
            return Err(format!(
                "The look has a srcset with a web address. {EXTERNAL_REASON}"
            ));
        }
    }

    let copied = copied_sentences(html, page_content);
    if copied >= COPIED_SENTENCES_REFUSED {
        return Err(format!(
            "The look copies the page's text into the HTML ({copied} whole sentences appear \
             literally), so it would go stale the moment the page changes: use wenlan.page \
             instead of copying text. Read the text from wenlan.page.text, wenlan.page.html or \
             wenlan.page.sections, and fill the elements from script when the page loads."
        ));
    }
    Ok(())
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How many distinct sentences of the page's text appear literally in `html`.
fn copied_sentences(html: &str, page_content: &str) -> usize {
    let text = plain_text(human_text(page_content));
    let collapsed_html = collapse_whitespace(html);
    let mut sentences: HashSet<String> = HashSet::new();
    let mut current = String::new();
    let push = |current: &mut String, sentences: &mut HashSet<String>| {
        let sentence = collapse_whitespace(current);
        if sentence.chars().count() >= COPIED_SENTENCE_CHARS {
            sentences.insert(sentence);
        }
        current.clear();
    };
    for c in text.chars() {
        if matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | '\n') {
            push(&mut current, &mut sentences);
        } else {
            current.push(c);
        }
    }
    push(&mut current, &mut sentences);
    sentences
        .iter()
        .filter(|sentence| {
            let mut escaped = String::new();
            escape(sentence, &mut escaped);
            collapsed_html.contains(sentence.as_str()) || collapsed_html.contains(&escaped)
        })
        .count()
}

#[cfg(test)]
#[path = "page_look_test.rs"]
mod tests;
