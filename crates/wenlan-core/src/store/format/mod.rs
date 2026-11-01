// SPDX-License-Identifier: Apache-2.0
//! The file formats of the move to Markdown files (storage migration spec,
//! section 3). Each kind of file has a parser and a renderer, and
//! `parse(render(file)) == file` for every file the renderer can write.
//!
//! Frontmatter is YAML between `---` lines with `id` first. Keys a format
//! does not own (a person's own Obsidian properties) are kept, in order,
//! after the owned ones.

pub mod names;
pub mod page;

use crate::error::WenlanError;

/// A file split into its frontmatter and its body.
pub(crate) struct Split<'a> {
    pub frontmatter: serde_yaml::Mapping,
    pub body: &'a str,
}

/// Splits `text` at its frontmatter. `None` when the file has none. A file
/// whose opening fence ends in CRLF was rewritten line by line by an editor,
/// so the caller reads it with LF (see [`normalize_line_ends`]).
pub(crate) fn split_frontmatter(text: &str) -> Result<Option<Split<'_>>, WenlanError> {
    let Some(rest) = text.strip_prefix("---\n") else {
        return Ok(None);
    };
    let (yaml, body) = if let Some(body) = rest.strip_prefix("---\n") {
        ("", body)
    } else if rest == "---" {
        ("", "")
    } else if let Some(end) = rest.find("\n---\n") {
        (&rest[..end + 1], &rest[end + 5..])
    } else if let Some(yaml) = rest.strip_suffix("\n---") {
        (yaml, "")
    } else {
        return Ok(None);
    };
    let frontmatter = if yaml.trim().is_empty() {
        serde_yaml::Mapping::new()
    } else {
        match serde_yaml::from_str::<serde_yaml::Value>(yaml) {
            Ok(serde_yaml::Value::Mapping(map)) => map,
            Ok(_) => {
                return Err(WenlanError::Validation(
                    "frontmatter is not a list of keys".to_string(),
                ))
            }
            Err(error) => {
                return Err(WenlanError::Validation(format!(
                    "frontmatter is not valid YAML: {error}"
                )))
            }
        }
    };
    Ok(Some(Split { frontmatter, body }))
}

/// The text with CRLF line ends turned into LF when its first line ends in
/// CRLF, the sign that an editor rewrote every line.
pub(crate) fn normalize_line_ends(text: &str) -> std::borrow::Cow<'_, str> {
    if text.starts_with("---\r\n") {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// `---`, the keys, `---`, each on its own line.
pub(crate) fn render_frontmatter(map: &serde_yaml::Mapping) -> String {
    let mut out = String::from("---\n");
    if !map.is_empty() {
        let yaml = serde_yaml::to_string(map).unwrap_or_default();
        out.push_str(&yaml);
        if !yaml.ends_with('\n') {
            out.push('\n');
        }
    }
    out.push_str("---\n");
    out
}

/// A scalar frontmatter value as text, or `None` for a list or a map.
pub(crate) fn scalar_text(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(text) => Some(text.clone()),
        serde_yaml::Value::Number(number) => Some(number.to_string()),
        serde_yaml::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// A list of strings, accepting the single-string shorthand (`tags: work`).
pub(crate) fn text_list(value: &serde_yaml::Value) -> Option<Vec<String>> {
    match value {
        serde_yaml::Value::Sequence(items) => items.iter().map(scalar_text).collect(),
        serde_yaml::Value::Null => Some(Vec::new()),
        other => scalar_text(other).map(|text| vec![text]),
    }
}

#[cfg(test)]
#[path = "format_test.rs"]
mod tests;
