// SPDX-License-Identifier: Apache-2.0
//! Two lengths in one page body (UX-VIEW-2/3).
//!
//! A page is one Markdown body. The part for people comes first; a fixed
//! marker heading then starts the details meant for AI tools:
//!
//! ```text
//! One plain paragraph that states the point, for a person.
//!
//! <!-- wenlan:details -->
//! ## 給 AI 的細節
//!
//! Exact commands, paths, edge cases…
//! ```
//!
//! The heading text is localized, so it is not what identifies the marker:
//! the HTML comment anchor on the line above it is. A body written by hand or
//! by an AI tool without the anchor is still recognized when its heading is
//! one of the known localized titles. Lines inside fenced code blocks never
//! count. Only the first marker splits; a later one stays in the details.

/// Stable anchor written on the line above the details heading. It is what
/// detection keys on, whatever the heading says.
pub const DETAILS_ANCHOR: &str = "<!-- wenlan:details -->";

/// The canonical heading, used when no locale is known (the spec's marker).
pub const DETAILS_HEADING_DEFAULT: &str = "給 AI 的細節";

/// Localized titles of the details heading, `(locale, title)`. Detection
/// accepts any of them without the anchor; writers pick one by locale.
pub const DETAILS_HEADINGS: &[(&str, &str)] = &[
    ("zh-Hant", "給 AI 的細節"),
    ("zh-Hans", "给 AI 的细节"),
    ("en", "Details for AI"),
    ("es", "Detalles para la IA"),
];

/// A body split at its details marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodySplit<'a> {
    /// Everything before the marker, trailing whitespace removed.
    pub human: &'a str,
    /// Everything after the heading line, surrounding blank lines removed.
    pub details: &'a str,
    /// The heading text as written (without `## `), when there is one.
    pub heading: Option<&'a str>,
    /// Byte offset where the marker starts (the anchor line, else the heading).
    pub marker_offset: usize,
}

/// The heading title for `locale` (`zh-Hant`, `zh-TW`, `zh-Hans`, `zh-CN`,
/// `en-US`, `es`…). Unknown or missing locales get the spec's default.
pub fn details_heading(locale: Option<&str>) -> &'static str {
    let Some(locale) = locale.map(str::trim).filter(|l| !l.is_empty()) else {
        return DETAILS_HEADING_DEFAULT;
    };
    let lower = locale.to_ascii_lowercase().replace('_', "-");
    let key = if lower.starts_with("zh") {
        if lower.contains("hans") || lower.ends_with("-cn") || lower.ends_with("-sg") {
            "zh-Hans"
        } else {
            "zh-Hant"
        }
    } else if lower.starts_with("es") {
        "es"
    } else if lower.starts_with("en") {
        "en"
    } else {
        return DETAILS_HEADING_DEFAULT;
    };
    DETAILS_HEADINGS
        .iter()
        .find(|(l, _)| *l == key)
        .map(|(_, title)| *title)
        .unwrap_or(DETAILS_HEADING_DEFAULT)
}

/// The two marker lines a writer puts before the details: the anchor, then
/// the localized heading.
pub fn details_marker(locale: Option<&str>) -> String {
    format!("{DETAILS_ANCHOR}\n## {}", details_heading(locale))
}

/// Join a human part and a details part into one body. Empty details give the
/// human part alone, with no marker.
pub fn join_body(human: &str, details: &str, locale: Option<&str>) -> String {
    let human = human.trim_end();
    let details = details.trim();
    if details.is_empty() {
        return human.to_string();
    }
    format!("{human}\n\n{}\n\n{details}\n", details_marker(locale))
}

fn normalized_heading_key(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_known_heading(text: &str) -> bool {
    let key = normalized_heading_key(text);
    DETAILS_HEADINGS
        .iter()
        .any(|(_, title)| normalized_heading_key(title) == key)
}

/// `## Title` → `Title`. Only level-2 headings are markers.
fn level_two_heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("##")?;
    if rest.starts_with('#') {
        return None;
    }
    let title = rest.trim();
    // A closing `##` sequence is allowed in ATX headings.
    let title = title.trim_end_matches('#').trim_end();
    (!title.is_empty()).then_some(title)
}

fn is_fence(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// Lines of `body` with their byte offsets, keeping line endings out.
fn lines_with_offsets(body: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0usize;
    body.split_inclusive('\n').map(move |raw| {
        let start = offset;
        offset += raw.len();
        (start, raw.trim_end_matches(['\n', '\r']))
    })
}

/// Split `body` at its first details marker, or `None` when it has none.
pub fn split_body(body: &str) -> Option<BodySplit<'_>> {
    let lines: Vec<(usize, &str)> = lines_with_offsets(body).collect();
    let mut fence: Option<&'static str> = None;
    for (index, (offset, line)) in lines.iter().enumerate() {
        if let Some(open) = fence {
            if line.trim_start().starts_with(open) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = is_fence(line) {
            fence = Some(open);
            continue;
        }
        if line.trim() == DETAILS_ANCHOR {
            // The heading is the next non-blank line, when it is a heading.
            let next = lines[index + 1..]
                .iter()
                .find(|(_, l)| !l.trim().is_empty());
            let (heading, details_start) = match next {
                Some((next_offset, next_line)) => match level_two_heading(next_line) {
                    Some(title) => (Some(title), next_offset + next_line.len()),
                    None => (None, offset + line.len()),
                },
                None => (None, offset + line.len()),
            };
            return Some(build_split(body, *offset, heading, details_start));
        }
        if let Some(title) = level_two_heading(line) {
            if is_known_heading(title) {
                return Some(build_split(body, *offset, Some(title), offset + line.len()));
            }
        }
    }
    None
}

fn build_split<'a>(
    body: &'a str,
    marker_offset: usize,
    heading: Option<&'a str>,
    details_start: usize,
) -> BodySplit<'a> {
    let details_start = details_start.min(body.len());
    BodySplit {
        human: body[..marker_offset].trim_end(),
        details: body[details_start..].trim_matches(|c: char| c == '\n' || c == '\r'),
        heading,
        marker_offset,
    }
}

/// The part for people: the whole body when it has no marker.
pub fn human_part(body: &str) -> &str {
    split_body(body).map(|s| s.human).unwrap_or(body)
}

/// The details for AI, when the body has a marker.
pub fn details_part(body: &str) -> Option<&str> {
    split_body(body).map(|s| s.details)
}

/// The short text: the first paragraph of the human part. Leading `#`
/// headings and blank lines are skipped; a paragraph ends at a blank line or
/// the next heading.
pub fn short_part(body: &str) -> &str {
    let human = human_part(body);
    let mut start: Option<usize> = None;
    let mut end = human.len();
    for (offset, line) in lines_with_offsets(human) {
        let trimmed = line.trim();
        match start {
            None => {
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                start = Some(offset);
            }
            Some(_) => {
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    end = offset;
                    break;
                }
            }
        }
    }
    match start {
        Some(start) => human[start..end].trim(),
        None => "",
    }
}

/// The three layers the read API returns beside the whole body.
pub fn body_parts(body: &str) -> wenlan_types::chat_check::PageBodyParts {
    let split = split_body(body);
    wenlan_types::chat_check::PageBodyParts {
        short: short_part(body).to_string(),
        human: split
            .as_ref()
            .map(|s| s.human)
            .unwrap_or_else(|| body.trim_end())
            .to_string(),
        details: split.as_ref().map(|s| s.details.to_string()),
        details_heading: split.and_then(|s| s.heading.map(str::to_string)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_at_the_anchor_whatever_the_heading_says() {
        let body = "Short for people.\n\n<!-- wenlan:details -->\n## Para la máquina\n\nRun `x`.\n";
        let split = split_body(body).expect("marker");
        assert_eq!(split.human, "Short for people.");
        assert_eq!(split.details, "Run `x`.");
        assert_eq!(split.heading, Some("Para la máquina"));
        assert_eq!(split.marker_offset, body.find("<!--").unwrap());
    }

    #[test]
    fn recognizes_every_localized_heading_without_the_anchor() {
        for (_, title) in DETAILS_HEADINGS {
            let body = format!("Opening.\n\n## {title}\n\ndetail\n");
            let split = split_body(&body).unwrap_or_else(|| panic!("{title} not found"));
            assert_eq!(split.human, "Opening.");
            assert_eq!(split.details, "detail");
            assert_eq!(split.heading, Some(*title));
        }
        // Spacing and case do not matter.
        let body = "Opening.\n## 給AI的細節\nx";
        assert_eq!(split_body(body).unwrap().details, "x");
        let body = "Opening.\n##   details FOR ai ##\nx";
        assert_eq!(split_body(body).unwrap().details, "x");
    }

    #[test]
    fn ignores_other_headings_and_fenced_code() {
        assert!(split_body("One.\n\n## Background\n\ntext").is_none());
        assert!(split_body("One.\n\n### Details for AI\n\ntext").is_none());
        let fenced = "One.\n\n```md\n<!-- wenlan:details -->\n## Details for AI\n```\n\nafter";
        assert!(split_body(fenced).is_none());
        let tilde = "One.\n~~~\n## 給 AI 的細節\n~~~\n";
        assert!(split_body(tilde).is_none());
    }

    #[test]
    fn only_the_first_marker_splits() {
        let body = "A.\n## Details for AI\nB\n## 給 AI 的細節\nC";
        let split = split_body(body).unwrap();
        assert_eq!(split.human, "A.");
        assert_eq!(split.details, "B\n## 給 AI 的細節\nC");
    }

    #[test]
    fn anchor_without_heading_still_splits() {
        let split = split_body("A.\n<!-- wenlan:details -->\nB").unwrap();
        assert_eq!(split.human, "A.");
        assert_eq!(split.details, "B");
        assert_eq!(split.heading, None);
    }

    #[test]
    fn crlf_bodies_split_cleanly() {
        let split =
            split_body("A.\r\n\r\n<!-- wenlan:details -->\r\n## Details for AI\r\n\r\nB\r\n")
                .unwrap();
        assert_eq!(split.human, "A.");
        assert_eq!(split.details, "B");
        assert_eq!(split.heading, Some("Details for AI"));
    }

    #[test]
    fn body_parts_carry_all_three_layers() {
        let parts = body_parts(
            "Short.\n\nMore for people.\n\n<!-- wenlan:details -->\n## Details for AI\n\nAI only.",
        );
        assert_eq!(parts.short, "Short.");
        assert_eq!(parts.human, "Short.\n\nMore for people.");
        assert_eq!(parts.details.as_deref(), Some("AI only."));
        assert_eq!(parts.details_heading.as_deref(), Some("Details for AI"));
        let plain = body_parts("Just this.\n");
        assert_eq!(plain.human, "Just this.");
        assert_eq!(plain.details, None);
        assert_eq!(plain.details_heading, None);
    }

    #[test]
    fn helpers_fall_back_to_the_whole_body() {
        assert_eq!(human_part("Only people text."), "Only people text.");
        assert_eq!(details_part("Only people text."), None);
    }

    #[test]
    fn short_is_the_first_paragraph_of_the_human_part() {
        let body = "# Title\n\nFirst sentence. Second one.\nStill first.\n\nSecond para.\n\n<!-- wenlan:details -->\n## Details for AI\nx";
        assert_eq!(
            short_part(body),
            "First sentence. Second one.\nStill first."
        );
        assert_eq!(short_part("Only one."), "Only one.");
        assert_eq!(short_part("One.\n## Heading\nTwo."), "One.");
        assert_eq!(short_part(""), "");
        assert_eq!(
            short_part("<!-- wenlan:details -->\n## Details for AI\nonly ai"),
            ""
        );
    }

    #[test]
    fn join_round_trips_through_split() {
        for locale in [
            None,
            Some("en-US"),
            Some("zh-TW"),
            Some("zh-CN"),
            Some("es-ES"),
            Some("fr"),
        ] {
            let body = join_body("For people.", "For AI.", locale);
            let split = split_body(&body).expect("joined body splits");
            assert_eq!(split.human, "For people.");
            assert_eq!(split.details, "For AI.");
            assert_eq!(split.heading, Some(details_heading(locale)));
        }
        assert_eq!(join_body("Only.\n", "  ", None), "Only.");
    }

    #[test]
    fn locale_picks_the_heading() {
        assert_eq!(details_heading(None), "給 AI 的細節");
        assert_eq!(details_heading(Some("zh-Hant-TW")), "給 AI 的細節");
        assert_eq!(details_heading(Some("zh_CN")), "给 AI 的细节");
        assert_eq!(details_heading(Some("en")), "Details for AI");
        assert_eq!(details_heading(Some("es-419")), "Detalles para la IA");
        assert_eq!(details_heading(Some("ja")), DETAILS_HEADING_DEFAULT);
    }
}
