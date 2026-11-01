// SPDX-License-Identifier: Apache-2.0
//! The file formats of the move to Markdown files (storage migration spec,
//! section 3). Each kind of file has a parser and a renderer, and
//! `parse(render(file)) == file` for every file the renderer can write.
//!
//! Frontmatter is YAML between `---` lines with `id` first. Keys a format
//! does not own (a person's own Obsidian properties) are kept, in order,
//! after the owned ones.

pub mod brief;
pub mod canvas;
pub mod entity;
pub mod memory;
pub mod names;
pub mod original;
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

/// Reads the keys a format owns out of a frontmatter map. A value of the
/// wrong shape is not lost: it stays, as written, with the keys the format
/// does not own.
pub(crate) struct Fields {
    map: serde_yaml::Mapping,
    unread: serde_yaml::Mapping,
}

impl Fields {
    pub(crate) fn new(map: serde_yaml::Mapping) -> Self {
        Self {
            map,
            unread: serde_yaml::Mapping::new(),
        }
    }

    fn take<T>(
        &mut self,
        key: &str,
        read: impl FnOnce(&serde_yaml::Value) -> Option<T>,
    ) -> Option<T> {
        let value = self.map.shift_remove(key)?;
        let read = read(&value);
        if read.is_none() {
            self.unread
                .insert(serde_yaml::Value::String(key.to_string()), value);
        }
        read
    }

    pub(crate) fn text(&mut self, key: &str) -> Option<String> {
        self.take(key, scalar_text)
    }

    pub(crate) fn list(&mut self, key: &str) -> Vec<String> {
        self.take(key, text_list).unwrap_or_default()
    }

    pub(crate) fn flag(&mut self, key: &str) -> bool {
        self.take(key, serde_yaml::Value::as_bool).unwrap_or(false)
    }

    pub(crate) fn int(&mut self, key: &str) -> Option<i64> {
        self.take(key, serde_yaml::Value::as_i64)
    }

    /// Any value, as written.
    pub(crate) fn value(&mut self, key: &str) -> Option<serde_yaml::Value> {
        self.map.shift_remove(key)
    }

    /// Errors when `type` names a different kind of file.
    pub(crate) fn expect_type(&mut self, kind: &str) -> Result<(), WenlanError> {
        match self.map.shift_remove("type") {
            None => Ok(()),
            Some(value) if value.as_str() == Some(kind) => Ok(()),
            Some(value) => Err(WenlanError::Validation(format!(
                "a {kind} file has type {value:?}"
            ))),
        }
    }

    /// The keys that were not read, those of the wrong shape first.
    pub(crate) fn rest(mut self) -> serde_yaml::Mapping {
        self.unread.extend(self.map);
        self.unread
    }
}

/// Builds a frontmatter map in order.
#[derive(Default)]
pub(crate) struct Keys(serde_yaml::Mapping);

impl Keys {
    pub(crate) fn text(&mut self, key: &str, value: &str) -> &mut Self {
        self.0.insert(key.into(), value.into());
        self
    }

    pub(crate) fn some(&mut self, key: &str, value: Option<&str>) -> &mut Self {
        if let Some(value) = value {
            self.text(key, value);
        }
        self
    }

    pub(crate) fn list(&mut self, key: &str, values: &[String]) -> &mut Self {
        if !values.is_empty() {
            let items = values.iter().map(|value| value.as_str().into()).collect();
            self.0
                .insert(key.into(), serde_yaml::Value::Sequence(items));
        }
        self
    }

    /// Written only when true.
    pub(crate) fn flag(&mut self, key: &str, value: bool) -> &mut Self {
        if value {
            self.0.insert(key.into(), true.into());
        }
        self
    }

    pub(crate) fn int(&mut self, key: &str, value: Option<i64>) -> &mut Self {
        if let Some(value) = value {
            self.0.insert(key.into(), value.into());
        }
        self
    }

    pub(crate) fn value(&mut self, key: &str, value: Option<&serde_yaml::Value>) -> &mut Self {
        if let Some(value) = value {
            self.0.insert(key.into(), value.clone());
        }
        self
    }

    /// The map, with `other`'s keys after the owned ones.
    pub(crate) fn finish(&mut self, other: &serde_yaml::Mapping) -> serde_yaml::Mapping {
        let mut map = std::mem::take(&mut self.0);
        for (key, value) in other {
            if !map.contains_key(key) {
                map.insert(key.clone(), value.clone());
            }
        }
        map
    }
}

/// The frontmatter and the body of a file that must have frontmatter.
pub(crate) fn require_frontmatter(text: &str, kind: &str) -> Result<(Fields, String), WenlanError> {
    let text = normalize_line_ends(text);
    let split = split_frontmatter(&text)?
        .ok_or_else(|| WenlanError::Validation(format!("a {kind} file has no frontmatter")))?;
    let body = split.body.strip_prefix('\n').unwrap_or(split.body);
    Ok((Fields::new(split.frontmatter), body.to_string()))
}

/// `---` frontmatter, a blank line, then `text` byte for byte and one line
/// end; [`exact_text`] reads it back.
pub(crate) fn render_exact(frontmatter: &serde_yaml::Mapping, text: &str) -> String {
    let mut out = render_frontmatter(frontmatter);
    out.push('\n');
    if !text.is_empty() {
        out.push_str(text);
        out.push('\n');
    }
    out
}

/// The text [`render_exact`] wrote.
pub(crate) fn exact_text(body: &str) -> String {
    body.strip_suffix('\n').unwrap_or(body).to_string()
}

/// `---` frontmatter, a blank line, then the body with one line end.
pub(crate) fn render_file(frontmatter: &serde_yaml::Mapping, body: &str) -> String {
    let mut out = render_frontmatter(frontmatter);
    out.push('\n');
    let body = body.trim_end_matches('\n');
    if !body.is_empty() {
        out.push_str(body);
        out.push('\n');
    }
    out
}

fn required(value: Option<String>, kind: &str, key: &str) -> Result<String, WenlanError> {
    value.ok_or_else(|| WenlanError::Validation(format!("a {kind} file has no {key}")))
}

#[cfg(test)]
#[path = "format_test.rs"]
mod tests;
