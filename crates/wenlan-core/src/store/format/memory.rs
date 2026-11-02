// SPDX-License-Identifier: Apache-2.0
//! The memory file, `memories/YYYY-MM/<id>.md` in the backstage (spec
//! section 3). The body is the memory's text byte for byte; `kind` is
//! written only when someone stated it. Times are UTC, to the second.

use super::{exact_text, render_exact, require_frontmatter, required, Keys};
use crate::error::WenlanError;

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryFile {
    pub id: String,
    /// identity, preference, decision, lesson, gotcha, fact, goal, recap or
    /// episode, only when it was given explicitly.
    pub kind: Option<String>,
    pub space: Option<String>,
    pub agent: Option<String>,
    pub created: Option<String>,
    pub pinned: bool,
    pub confirmed: bool,
    pub supersedes: Option<String>,
    pub supersede_mode: Option<String>,
    pub episode_of: Option<String>,
    pub event_date: Option<String>,
    pub event_end: Option<String>,
    pub importance: Option<i64>,
    /// `human_capture`, `generated` or `document_ingest`: whether the text
    /// counts as the person's own writing.
    pub origin: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub session: Option<String>,
    /// The entity the memory is filed under.
    pub entity: Option<String>,
    pub recap: bool,
    /// Fields someone stated with the memory, as given.
    pub fields: Option<serde_yaml::Value>,
    /// The prose as sent, when the text is a rewrite of it.
    pub source_text: Option<String>,
    /// Entity names, written as `[[name]]` links.
    pub about: Vec<String>,
    pub other: serde_yaml::Mapping,
    pub text: String,
}

impl MemoryFile {
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: None,
            space: None,
            agent: None,
            created: None,
            pinned: false,
            confirmed: false,
            supersedes: None,
            supersede_mode: None,
            episode_of: None,
            event_date: None,
            event_end: None,
            importance: None,
            origin: None,
            title: None,
            url: None,
            session: None,
            entity: None,
            recap: false,
            fields: None,
            source_text: None,
            about: Vec::new(),
            other: serde_yaml::Mapping::new(),
            text: text.into(),
        }
    }
}

pub fn parse(text: &str) -> Result<MemoryFile, WenlanError> {
    let (mut fields, body) = require_frontmatter(text, "memory")?;
    let id = required(fields.text("id"), "memory", "id")?;
    fields.expect_type("memory")?;
    let mut memory = MemoryFile::new(id, exact_text(&body));
    memory.kind = fields.text("kind");
    memory.space = fields.text("space");
    memory.agent = fields.text("agent");
    memory.created = fields.text("created");
    memory.pinned = fields.flag("pinned");
    memory.confirmed = fields.flag("confirmed");
    memory.supersedes = fields.text("supersedes");
    memory.supersede_mode = fields.text("supersede_mode");
    memory.episode_of = fields.text("episode_of");
    memory.event_date = fields.text("event_date");
    memory.event_end = fields.text("event_end");
    memory.importance = fields.int("importance");
    memory.origin = fields.text("origin");
    memory.title = fields.text("title");
    memory.url = fields.text("url");
    memory.session = fields.text("session");
    memory.entity = fields.text("entity");
    memory.recap = fields.flag("recap");
    memory.fields = fields.value("fields");
    memory.source_text = fields.text("source_text");
    memory.about = fields
        .list("about")
        .into_iter()
        .map(|link| unlink(&link).to_string())
        .collect();
    memory.other = fields.rest();
    Ok(memory)
}

pub fn render(memory: &MemoryFile) -> String {
    let about: Vec<String> = memory
        .about
        .iter()
        .map(|name| format!("[[{name}]]"))
        .collect();
    let frontmatter = Keys::default()
        .text("id", &memory.id)
        .text("type", "memory")
        .some("kind", memory.kind.as_deref())
        .some("space", memory.space.as_deref())
        .some("agent", memory.agent.as_deref())
        .some("created", memory.created.as_deref())
        .flag("pinned", memory.pinned)
        .flag("confirmed", memory.confirmed)
        .some("supersedes", memory.supersedes.as_deref())
        .some("supersede_mode", memory.supersede_mode.as_deref())
        .some("episode_of", memory.episode_of.as_deref())
        .some("event_date", memory.event_date.as_deref())
        .some("event_end", memory.event_end.as_deref())
        .int("importance", memory.importance)
        .some("origin", memory.origin.as_deref())
        .some("title", memory.title.as_deref())
        .some("url", memory.url.as_deref())
        .some("session", memory.session.as_deref())
        .some("entity", memory.entity.as_deref())
        .flag("recap", memory.recap)
        .value("fields", memory.fields.as_ref())
        .some("source_text", memory.source_text.as_deref())
        .list("about", &about)
        .finish(&memory.other);
    render_exact(&frontmatter, &memory.text)
}

/// `[[name]]` → `name`; anything else as it is.
pub(crate) fn unlink(link: &str) -> &str {
    link.strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        .unwrap_or(link)
}
