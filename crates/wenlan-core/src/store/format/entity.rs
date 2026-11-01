// SPDX-License-Identifier: Apache-2.0
//! The entity file, `entities/<slug>-<id6>.md` in the backstage (spec
//! section 3). The body is the entity's description byte for byte.

use super::{exact_text, render_exact, require_frontmatter, required, Keys};
use crate::error::WenlanError;

#[derive(Debug, Clone, PartialEq)]
pub struct EntityFile {
    pub id: String,
    pub name: String,
    /// The id of the page that shows this entity, when it has one.
    pub page: Option<String>,
    pub entity_type: Option<String>,
    pub aliases: Vec<String>,
    pub confirmed: bool,
    pub established_by: Option<String>,
    pub space: Option<String>,
    pub other: serde_yaml::Mapping,
    pub description: String,
}

impl EntityFile {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            page: None,
            entity_type: None,
            aliases: Vec::new(),
            confirmed: false,
            established_by: None,
            space: None,
            other: serde_yaml::Mapping::new(),
            description: String::new(),
        }
    }
}

pub fn parse(text: &str) -> Result<EntityFile, WenlanError> {
    let (mut fields, body) = require_frontmatter(text, "entity")?;
    let id = required(fields.text("id"), "entity", "id")?;
    let name = required(fields.text("title"), "entity", "title")?;
    fields.expect_type("entity")?;
    let mut entity = EntityFile::new(id, name);
    entity.page = fields.text("page");
    entity.entity_type = fields.text("entity_type");
    entity.aliases = fields.list("aliases");
    entity.confirmed = fields.flag("confirmed");
    entity.established_by = fields.text("established_by");
    entity.space = fields.text("space");
    entity.other = fields.rest();
    entity.description = exact_text(&body);
    Ok(entity)
}

pub fn render(entity: &EntityFile) -> String {
    let frontmatter = Keys::default()
        .text("id", &entity.id)
        .text("title", &entity.name)
        .text("type", "entity")
        .some("page", entity.page.as_deref())
        .some("entity_type", entity.entity_type.as_deref())
        .list("aliases", &entity.aliases)
        .flag("confirmed", entity.confirmed)
        .some("established_by", entity.established_by.as_deref())
        .some("space", entity.space.as_deref())
        .finish(&entity.other);
    render_exact(&frontmatter, &entity.description)
}
