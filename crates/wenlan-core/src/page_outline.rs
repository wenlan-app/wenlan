// SPDX-License-Identifier: Apache-2.0
//! A page's own outline, read live from its Markdown body for the mind map.
//!
//! The mind map opens on what the page already says: its headings (nested by
//! level) and, under each heading, the items written there (list items, a
//! paragraph's bold lead, a table's first column). Nothing here is stored or
//! proposed: it is the page, drawn as a tree, so it needs no accepting.
//!
//! Rules, held by tests below:
//! - Only the part for people counts: the "Details for AI" section and
//!   everything after it is left out (`page_body::human_part`).
//! - Lines inside fenced code blocks never count, so a `# comment` in a code
//!   sample is not a heading.
//! - A "Sources" heading and its contents are left out.
//! - A lone top heading that repeats the page title is skipped.
//! - A page with no headings and no items falls back to the first sentence
//!   of its paragraphs.

pub use wenlan_types::page_map::{OutlineKind, PageOutlineNode as OutlineNode};

/// Upper bound on outline nodes, so a huge page can't produce an unreadable map.
const MAX_NODES: usize = 400;
/// Paragraph fallback length for a page with no headings or items.
const MAX_FALLBACK_ITEMS: usize = 7;
/// Short-label budget in display columns (a CJK character counts as two).
const SHORT_COLUMNS: usize = 40;

/// Headings whose section is left out of the map (source lists are for
/// checking, not for reading the page's shape).
const SKIPPED_HEADINGS: &[&str] = &[
    "sources",
    "來源",
    "来源",
    "fuentes",
    "references",
    "參考資料",
    "参考资料",
];

/// The outline of `body` for the page called `title`.
pub fn page_outline(body: &str, title: &str) -> Vec<OutlineNode> {
    let human = crate::page_body::human_part(strip_frontmatter(body));
    let mut out = Vec::new();
    // (level, id) of the open headings, innermost last.
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut item_counts: std::collections::HashMap<Option<String>, usize> = Default::default();
    let mut used_ids: std::collections::HashSet<String> = Default::default();
    // Level of a skipped section (Sources) while we're inside it.
    let mut skipping: Option<usize> = None;
    let mut in_fence: Option<&str> = None;
    let mut table_row = 0usize;
    let mut first_heading = true;
    let title_norm = normalize(title);

    for raw in human.lines() {
        if out.len() >= MAX_NODES {
            break;
        }
        let line = raw.trim_end();
        let trimmed = line.trim_start();

        if let Some(fence) = in_fence {
            if trimmed.starts_with(fence) {
                in_fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = Some(&trimmed[..3]);
            continue;
        }

        if let Some((level, heading)) = atx_heading(trimmed) {
            table_row = 0;
            let was_first = first_heading;
            first_heading = false;
            if let Some(skip_level) = skipping {
                if level > skip_level {
                    continue;
                }
                skipping = None;
            }
            if SKIPPED_HEADINGS.contains(&normalize(&heading).as_str()) {
                skipping = Some(level);
                continue;
            }
            if was_first && level == 1 && normalize(&heading) == title_norm {
                continue;
            }
            while stack.last().is_some_and(|(l, _)| *l >= level) {
                stack.pop();
            }
            let parent = stack.last().map(|(_, id)| id.clone());
            let anchor = crate::export::obsidian::slugify(&heading);
            let base = match &parent {
                Some(p) => format!("{p}/{}", short_slug(&anchor, out.len())),
                None => format!("h:{}", short_slug(&anchor, out.len())),
            };
            let id = unique(base, &mut used_ids);
            let text = clean_inline(&heading);
            out.push(OutlineNode {
                id: id.clone(),
                parent_id: parent,
                kind: OutlineKind::Heading,
                label: shorten(&text),
                text,
                anchor: Some(anchor),
            });
            stack.push((level, id));
            continue;
        }
        if skipping.is_some() {
            continue;
        }

        let parent = stack.last().map(|(_, id)| id.clone());
        let item = if let Some(text) = top_level_list_item(line) {
            table_row = 0;
            Some(text)
        } else if trimmed.starts_with('|') {
            table_row += 1;
            // Row 1 is the header, row 2 the separator.
            if table_row > 2 {
                first_table_cell(trimmed)
            } else {
                None
            }
        } else {
            table_row = 0;
            bold_lead(trimmed).map(|_| trimmed.to_string())
        };
        if let Some(text) = item {
            let text = clean_inline(&text);
            if text.is_empty() {
                continue;
            }
            let n = item_counts.entry(parent.clone()).or_default();
            *n += 1;
            let id = unique(
                format!("{}#{}", parent.as_deref().unwrap_or("i"), n),
                &mut used_ids,
            );
            let label = bold_lead(trimmed)
                .map(|lead| shorten(&clean_inline(&lead)))
                .unwrap_or_else(|| shorten(&text));
            out.push(OutlineNode {
                id,
                parent_id: parent,
                kind: OutlineKind::Item,
                label,
                text,
                anchor: None,
            });
        }
    }

    if out.is_empty() {
        out = paragraph_fallback(human);
    }
    out
}

fn strip_frontmatter(body: &str) -> &str {
    let Some(rest) = body
        .strip_prefix("---\n")
        .or_else(|| body.strip_prefix("---\r\n"))
    else {
        return body;
    };
    match rest.find("\n---") {
        Some(end) => {
            let after = &rest[end + 4..];
            after.split_once('\n').map(|(_, b)| b).unwrap_or("")
        }
        None => body,
    }
}

fn atx_heading(line: &str) -> Option<(usize, String)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &line[level..];
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim();
    (!text.is_empty())
        .then(|| text.to_string())
        .map(|t| (level, t))
}

/// A list item at the left margin (nested items belong to their parent item).
fn top_level_list_item(line: &str) -> Option<String> {
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            let rest = rest.trim();
            // A task box is still the item's text.
            let rest = rest
                .strip_prefix("[ ] ")
                .or_else(|| rest.strip_prefix("[x] "))
                .or_else(|| rest.strip_prefix("[X] "))
                .unwrap_or(rest);
            return Some(rest.to_string());
        }
    }
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// `**Lead.** rest` → `Lead.`; only when the line opens with bold text.
fn bold_lead(line: &str) -> Option<String> {
    let line = top_level_list_item(line).unwrap_or_else(|| line.trim().to_string());
    let rest = line.strip_prefix("**")?;
    let end = rest.find("**")?;
    let lead = rest[..end].trim().trim_end_matches(['.', ':', '：', '。']);
    (!lead.is_empty()).then(|| lead.to_string())
}

/// A row's first cell. A short first cell (a date, a name) is joined with the
/// second so the box still says what the row is about.
fn first_table_cell(row: &str) -> Option<String> {
    let mut cells = row.trim_start_matches('|').split('|').map(str::trim);
    let first = cells.next()?;
    if first.is_empty() || first.chars().all(|c| matches!(c, '-' | ':' | ' ')) {
        return None;
    }
    let width: usize = first.chars().map(display_width).sum();
    match cells.next().filter(|second| !second.is_empty()) {
        Some(second) if width <= 20 => Some(format!("{first} · {second}")),
        _ => Some(first.to_string()),
    }
}

/// Plain text: wikilinks show their display name, links their text, emphasis
/// and code marks dropped.
fn clean_inline(text: &str) -> String {
    let mut s = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("[[") {
        s.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("]]") else {
            s.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let inner = &after[..close];
        let shown = inner.rsplit('|').next().unwrap_or(inner);
        s.push_str(shown.split('#').next().unwrap_or(shown).trim());
        rest = &after[close + 2..];
    }
    s.push_str(rest);
    // [text](url) → text
    let mut out = String::with_capacity(s.len());
    let mut chars = s.as_str();
    while let Some(open) = chars.find('[') {
        let after = &chars[open + 1..];
        match (after.find("]("), after.find(')')) {
            (Some(mid), Some(close)) if mid < close => {
                out.push_str(&chars[..open]);
                out.push_str(&after[..mid]);
                chars = &after[close + 1..];
            }
            _ => {
                out.push_str(&chars[..=open]);
                chars = after;
            }
        }
    }
    out.push_str(chars);
    out.replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_width(c: char) -> usize {
    if (c as u32) >= 0x1100 && unicode_wide(c) {
        2
    } else {
        1
    }
}

fn unicode_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF |
        0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF |
        0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6)
}

/// The box text: the first clause when it fits, else cut at a word or
/// character boundary with an ellipsis.
pub fn shorten(text: &str) -> String {
    let width: usize = text.chars().map(display_width).sum();
    if width <= SHORT_COLUMNS {
        return text.to_string();
    }
    // Prefer ending at a clause mark that leaves a readable piece.
    let mut cols = 0;
    let mut cut_at_clause = None;
    for (i, c) in text.char_indices() {
        cols += display_width(c);
        if cols > SHORT_COLUMNS {
            break;
        }
        let clause = matches!(c, '。' | '，' | '：' | '；' | '、')
            || (matches!(c, '.' | ':' | ';' | ',') && text[i + 1..].starts_with(' '));
        // A cut inside quotes would leave one quote mark hanging.
        let quotes_balanced = text[..i].matches('"').count().is_multiple_of(2)
            && text[..i].matches('“').count() == text[..i].matches('”').count()
            && text[..i].matches('「').count() == text[..i].matches('」').count();
        if clause && cols >= 12 && quotes_balanced {
            cut_at_clause = Some(i);
        }
    }
    if let Some(i) = cut_at_clause {
        return text[..i].trim_end().to_string();
    }
    let mut cols = 0;
    let mut end = 0;
    let mut last_space = None;
    for (i, c) in text.char_indices() {
        let w = display_width(c);
        if cols + w > SHORT_COLUMNS - 1 {
            break;
        }
        cols += w;
        end = i + c.len_utf8();
        if c == ' ' {
            last_space = Some(i);
        }
    }
    // Never cut an English word in half when a space is reasonably close.
    let end = match last_space {
        Some(s)
            if s * 3 >= end * 2
                && !text[end..].starts_with(' ')
                && text[..end]
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_ascii_alphanumeric()) =>
        {
            s
        }
        _ => end,
    };
    format!("{}…", text[..end].trim_end())
}

fn normalize(s: &str) -> String {
    clean_inline(s).trim().to_lowercase()
}

fn short_slug(slug: &str, ordinal: usize) -> String {
    let s: String = slug.chars().take(40).collect();
    if s.is_empty() {
        format!("s{ordinal}")
    } else {
        s
    }
}

fn unique(base: String, used: &mut std::collections::HashSet<String>) -> String {
    if used.insert(base.clone()) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}~{n}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

fn paragraph_fallback(body: &str) -> Vec<OutlineNode> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for para in body.split("\n\n") {
        let para = para.trim();
        if para.starts_with("```") || para.starts_with("~~~") {
            in_fence = !para[3..].contains("```") && !para[3..].contains("~~~");
            continue;
        }
        if in_fence {
            if para.contains("```") || para.contains("~~~") {
                in_fence = false;
            }
            continue;
        }
        if para.is_empty()
            || para.starts_with('<')
            || para.starts_with('>')
            || para.starts_with('|')
        {
            continue;
        }
        let text = clean_inline(&para.replace('\n', " "));
        let sentence = first_sentence(&text);
        if sentence.is_empty() {
            continue;
        }
        let n = out.len() + 1;
        out.push(OutlineNode {
            id: format!("p#{n}"),
            parent_id: None,
            kind: OutlineKind::Item,
            label: shorten(&sentence),
            text,
            anchor: None,
        });
        if out.len() >= MAX_FALLBACK_ITEMS {
            break;
        }
    }
    out
}

fn first_sentence(text: &str) -> String {
    for (i, c) in text.char_indices() {
        if matches!(c, '。' | '！' | '？') {
            return text[..i + c.len_utf8()].to_string();
        }
        if matches!(c, '.' | '!' | '?') && (text[i + 1..].starts_with(' ') || i + 1 == text.len()) {
            return text[..=i].to_string();
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(nodes: &[OutlineNode]) -> Vec<(&str, Option<&str>, &str)> {
        nodes
            .iter()
            .map(|n| (n.id.as_str(), n.parent_id.as_deref(), n.label.as_str()))
            .collect()
    }

    #[test]
    fn headings_nest_by_level_and_items_hang_under_their_heading() {
        let body = "Intro line.\n\n## One\n\n- first point\n- second **point**\n\n### Deeper\n\n1. numbered\n\n## Two\n\n**Lead.** The rest of it.\n";
        let nodes = page_outline(body, "Page");
        let shape = labels(&nodes);
        assert_eq!(
            shape,
            vec![
                ("h:one", None, "One"),
                ("h:one#1", Some("h:one"), "first point"),
                ("h:one#2", Some("h:one"), "second point"),
                ("h:one/deeper", Some("h:one"), "Deeper"),
                ("h:one/deeper#1", Some("h:one/deeper"), "numbered"),
                ("h:two", None, "Two"),
                ("h:two#1", Some("h:two"), "Lead"),
            ]
        );
        assert_eq!(nodes[6].text, "Lead. The rest of it.");
        assert_eq!(nodes[0].anchor.as_deref(), Some("one"));
    }

    #[test]
    fn code_fences_details_for_ai_and_sources_are_left_out() {
        let body = "## Real\n\n```sh\n# not a heading\n- not an item\n```\n\n- kept\n\n## Sources\n\n- [1] a memory\n\n### Sub of sources\n\n- also hidden\n\n## After\n\n- shown\n\n<!-- wenlan:details -->\n## Details for AI\n\n- hidden\n";
        let nodes = page_outline(body, "P");
        let texts: Vec<_> = nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(texts, vec!["Real", "kept", "After", "shown"]);
    }

    #[test]
    fn a_top_heading_repeating_the_title_is_skipped() {
        let nodes = page_outline("# My Page\n\n## A\n", "My Page");
        assert_eq!(labels(&nodes), vec![("h:a", None, "A")]);
    }

    #[test]
    fn table_rows_contribute_their_first_cell() {
        let body = "## T\n\n| Date | What |\n|---|---|\n| 2026-03 | pivot |\n| 2026-04 | more |\n";
        let nodes = page_outline(body, "P");
        let texts: Vec<_> = nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(texts, vec!["T", "2026-03 · pivot", "2026-04 · more"]);
    }

    #[test]
    fn wikilinks_and_links_read_as_their_text() {
        let nodes = page_outline(
            "- see [[Other page|the other]] and [site](https://x.y)\n- [[Plain]]\n",
            "P",
        );
        let texts: Vec<_> = nodes.iter().map(|n| n.text.as_str()).collect();
        assert_eq!(texts, vec!["see the other and site", "Plain"]);
    }

    #[test]
    fn a_page_without_headings_or_items_uses_first_sentences() {
        let nodes = page_outline(
            "想把首頁的字再砍一半。現在第一屏要讀太久。\n\nSecond idea here. More words.\n",
            "P",
        );
        let texts: Vec<_> = nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(texts, vec!["想把首頁的字再砍一半。", "Second idea here."]);
    }

    #[test]
    fn long_text_is_shortened_by_display_width() {
        let s = shorten("Pivot from ambient file search to a developer-first AI memory layer that is local first");
        assert!(s.ends_with('…'), "{s}");
        assert!(s.chars().count() <= 45);
        let zh =
            shorten("替你整理的個人知識庫，給每天用 AI、又不想維護筆記系統的人，優勢還沒有被證明");
        assert_eq!(zh, "替你整理的個人知識庫，給每天用 AI");
    }

    #[test]
    fn ids_stay_unique_when_headings_repeat() {
        let nodes = page_outline("## Notes\n\n## Notes\n", "P");
        assert_eq!(nodes[0].id, "h:notes");
        assert_eq!(nodes[1].id, "h:notes~2");
    }

    #[test]
    fn frontmatter_is_not_read() {
        let nodes = page_outline("---\ntitle: X\ntags: [a]\n---\n\n## Body\n", "X");
        assert_eq!(labels(&nodes), vec![("h:body", None, "Body")]);
    }
}
