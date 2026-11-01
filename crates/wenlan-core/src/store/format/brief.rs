// SPDX-License-Identifier: Apache-2.0
//! The Brief file, `briefs/<space-slug>-<space-id6>.md` in the backstage
//! (spec section 3).
//!
//! ```text
//! ---
//! id: brief_abc
//! type: brief
//! space: Work
//! version: 7
//! ---
//!
//! What the last session left off with.
//!
//! ## Active
//! - [ ] Ship the export <!-- wenlan {"id":"item_1","added":1793491200,"version":3} -->
//!
//! ## Backlog
//! - [ ] Try the new parser <!-- wenlan {"id":"item_2","added":1793491300,"version":5,"gate":"after the export"} -->
//! ```
//!
//! Each item's id, when it was added, its version and its gate ride in a
//! comment at the end of its line, so the list reads as a plain checklist.

use serde::{Deserialize, Serialize};

use super::{render_file, require_frontmatter, required, Keys};
use crate::error::WenlanError;

const ACTIVE: &str = "## Active";
const BACKLOG: &str = "## Backlog";
const ITEM_PREFIX: &str = "- [ ] ";
const META_START: &str = " <!-- wenlan ";
const META_END: &str = " -->";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BriefItemLine {
    pub id: String,
    /// Only in the comment when the visible line could not hold it as is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    pub added: i64,
    pub version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefItem {
    pub id: String,
    pub text: String,
    pub added: i64,
    pub version: i64,
    pub gate: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BriefFile {
    pub id: String,
    pub space: String,
    pub version: i64,
    pub other: serde_yaml::Mapping,
    /// What the last session left off with.
    pub summary: String,
    pub active: Vec<BriefItem>,
    pub backlog: Vec<BriefItem>,
}

pub fn parse(text: &str) -> Result<BriefFile, WenlanError> {
    let (mut fields, body) = require_frontmatter(text, "brief")?;
    let id = required(fields.text("id"), "brief", "id")?;
    fields.expect_type("brief")?;
    let space = required(fields.text("space"), "brief", "space")?;
    let version = fields
        .int("version")
        .ok_or_else(|| WenlanError::Validation("a brief file has no version".to_string()))?;
    let other = fields.rest();

    let mut summary = Vec::new();
    let mut active = Vec::new();
    let mut backlog = Vec::new();
    let mut section = None;
    for line in body.lines() {
        match line {
            ACTIVE => section = Some(true),
            BACKLOG => section = Some(false),
            _ => match section {
                None => summary.push(line),
                Some(is_active) => {
                    if line.is_empty() {
                        continue;
                    }
                    let item = parse_item(line)?;
                    if is_active {
                        active.push(item);
                    } else {
                        backlog.push(item);
                    }
                }
            },
        }
    }
    Ok(BriefFile {
        id,
        space,
        version,
        other,
        summary: summary.join("\n").trim_end_matches('\n').to_string(),
        active,
        backlog,
    })
}

pub fn render(brief: &BriefFile) -> String {
    let frontmatter = Keys::default()
        .text("id", &brief.id)
        .text("type", "brief")
        .text("space", &brief.space)
        .int("version", Some(brief.version))
        .finish(&brief.other);
    let mut body = String::new();
    let summary = brief.summary.trim_end_matches('\n');
    if !summary.is_empty() {
        body.push_str(summary);
        body.push_str("\n\n");
    }
    for (heading, items) in [(ACTIVE, &brief.active), (BACKLOG, &brief.backlog)] {
        body.push_str(heading);
        body.push('\n');
        for item in items {
            body.push_str(&render_item(item));
            body.push('\n');
        }
        body.push('\n');
    }
    render_file(&frontmatter, &body)
}

fn render_item(item: &BriefItem) -> String {
    // The visible line holds the text unless that would change it: a line
    // end, or an end that reads as the comment.
    let fits = !item.text.contains('\n')
        && !item.text.contains(META_START)
        && item.text.trim_end() == item.text;
    let visible = if fits {
        item.text.clone()
    } else {
        item.text.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let meta = BriefItemLine {
        id: item.id.clone(),
        text: (!fits).then(|| item.text.clone()),
        added: item.added,
        version: item.version,
        gate: item.gate.clone(),
    };
    // `<` and `>` are escaped so the comment cannot be closed or reopened
    // from inside.
    let json = serde_json::to_string(&meta)
        .unwrap_or_default()
        .replace('>', "\\u003e")
        .replace('<', "\\u003c");
    format!("{ITEM_PREFIX}{visible}{META_START}{json}{META_END}")
}

fn parse_item(line: &str) -> Result<BriefItem, WenlanError> {
    let bad = || WenlanError::Validation(format!("not a Brief item: {line:?}"));
    let rest = line.strip_prefix(ITEM_PREFIX).ok_or_else(bad)?;
    let rest = rest.strip_suffix(META_END).ok_or_else(bad)?;
    let (visible, json) = rest.rsplit_once(META_START).ok_or_else(bad)?;
    let meta: BriefItemLine = serde_json::from_str(json).map_err(|_| bad())?;
    Ok(BriefItem {
        id: meta.id,
        text: meta.text.unwrap_or_else(|| visible.to_string()),
        added: meta.added,
        version: meta.version,
        gate: meta.gate,
    })
}
