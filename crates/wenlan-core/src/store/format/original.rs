// SPDX-License-Identifier: Apache-2.0
//! The imported original, `originals/<vendor>/<external-id>.md` in the
//! backstage (spec section 3): a conversation as the other tool kept it.
//! Wenlan never edits it, and nothing in it counts as the person's writing.
//!
//! ```text
//! ---
//! id: orig_abc
//! type: original
//! author: original
//! vendor: chatgpt
//! external_id: 6720-…
//! title: Trip ideas
//! imported: 2026-11-01T10:00:00Z
//! url: https://chatgpt.com/c/6720-…
//! ---
//!
//! <!-- wenlan:message -->
//! ## user · 2026-10-30T09:12:00Z
//!
//! Where should we go in May?
//!
//! <!-- wenlan:message -->
//! ## assistant · 2026-10-30T09:12:05Z
//!
//! A few ideas…
//! ```
//!
//! Each message starts at a marker line, so a `##` heading inside a message
//! is just text. Each message ends with one blank line that is not part of
//! it, so the message's own text, line ends included, is kept as written.

use super::{render_exact, require_frontmatter, required, Keys};
use crate::error::WenlanError;

pub const MESSAGE_MARKER: &str = "<!-- wenlan:message -->";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: String,
    pub at: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OriginalFile {
    pub id: String,
    pub vendor: String,
    pub external_id: String,
    pub title: Option<String>,
    pub imported: Option<String>,
    pub url: Option<String>,
    pub other: serde_yaml::Mapping,
    /// In the order the tool kept them.
    pub messages: Vec<Message>,
}

pub fn parse(text: &str) -> Result<OriginalFile, WenlanError> {
    let (mut fields, body) = require_frontmatter(text, "original")?;
    let id = required(fields.text("id"), "original", "id")?;
    fields.expect_type("original")?;
    match fields.text("author").as_deref() {
        Some("original") => {}
        author => {
            return Err(WenlanError::Validation(format!(
                "an original has author {author:?}, not \"original\""
            )))
        }
    }
    let vendor = required(fields.text("vendor"), "original", "vendor")?;
    let external_id = required(fields.text("external_id"), "original", "external_id")?;
    let title = fields.text("title");
    let imported = fields.text("imported");
    let url = fields.text("url");
    let other = fields.rest();

    let mut messages = Vec::new();
    let marker = format!("{MESSAGE_MARKER}\n");
    let mut chunks = body.split(marker.as_str());
    if let Some(before) = chunks.next() {
        if !before.trim().is_empty() {
            return Err(WenlanError::Validation(
                "an original has text before its first message".to_string(),
            ));
        }
    }
    for chunk in chunks {
        let (heading, rest) = chunk.split_once('\n').unwrap_or((chunk, ""));
        let heading = heading
            .strip_prefix("## ")
            .ok_or_else(|| WenlanError::Validation(format!("a message heading is {heading:?}")))?;
        let (role, at) = match heading.split_once(" · ") {
            Some((role, at)) => (role.to_string(), Some(at.to_string())),
            None => (heading.to_string(), None),
        };
        let text = rest.strip_prefix('\n').unwrap_or(rest);
        // Each message ends with exactly one blank line, so its own line
        // ends are kept as written.
        let text = text
            .strip_suffix("\n\n")
            .or_else(|| text.strip_suffix('\n'))
            .unwrap_or(text);
        messages.push(Message {
            role,
            at,
            text: text.to_string(),
        });
    }
    Ok(OriginalFile {
        id,
        vendor,
        external_id,
        title,
        imported,
        url,
        other,
        messages,
    })
}

pub fn render(original: &OriginalFile) -> String {
    let frontmatter = Keys::default()
        .text("id", &original.id)
        .text("type", "original")
        .text("author", "original")
        .text("vendor", &original.vendor)
        .text("external_id", &original.external_id)
        .some("title", original.title.as_deref())
        .some("imported", original.imported.as_deref())
        .some("url", original.url.as_deref())
        .finish(&original.other);
    let mut body = String::new();
    for message in &original.messages {
        body.push_str(MESSAGE_MARKER);
        body.push_str("\n## ");
        body.push_str(&message.role);
        if let Some(at) = &message.at {
            body.push_str(" · ");
            body.push_str(at);
        }
        body.push_str("\n\n");
        body.push_str(&message.text);
        body.push_str("\n\n");
    }
    // `render_exact` adds the last line end back.
    render_exact(&frontmatter, body.strip_suffix('\n').unwrap_or(&body))
}
