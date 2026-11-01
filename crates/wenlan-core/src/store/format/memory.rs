// SPDX-License-Identifier: Apache-2.0
//! The memory file, `memories/YYYY-MM/<id>.md` in the backstage (spec
//! section 3). The body is the memory's text; `kind`, `event_date` and
//! `importance` are written only when someone stated them.

use super::{render_file, require_frontmatter, required, Keys};
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
    pub importance: Option<i64>,
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
            importance: None,
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
    let mut memory = MemoryFile::new(id, body.trim_end_matches('\n'));
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
    memory.importance = fields.int("importance");
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
        .int("importance", memory.importance)
        .list("about", &about)
        .finish(&memory.other);
    render_file(&frontmatter, &memory.text)
}

/// `[[name]]` → `name`; anything else as it is.
pub(crate) fn unlink(link: &str) -> &str {
    link.strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        .unwrap_or(link)
}
