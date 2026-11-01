// SPDX-License-Identifier: Apache-2.0
//! The Markdown every page shares with Obsidian, read the same way on every
//! page wherever it came from.
//!
//! Two things here:
//!
//! - **Tags.** A `#tag` in the body (and a `tags:` list in front matter) is a
//!   tag of the page, as Obsidian reads it: `#` at the start of a line or
//!   after a space, then letters in any script, digits, `_`, `-` or `/`, with
//!   at least one character that is not a digit. Code is skipped. Tags are
//!   read from the text each time, never stored, so they cannot drift from
//!   it.
//! - **Embeds.** `![[name.png]]` names a file, not a path. [`resolve_embed`]
//!   finds that file in the pages folder the way Obsidian does: an exact path
//!   from the pages folder first, then a file of that name in the page's own
//!   folder, then the one with the shortest path. Only images and videos, and
//!   never in a hidden folder (`.obsidian`, `.wenlan`, `.trash`).
//!
//! Anything only Obsidian has (Canvas, plugins, `^block` references) belongs
//! in the Obsidian layer under `sources::obsidian`, not here.

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use wenlan_types::attachments::{kind_for_name, AttachmentKind};

use crate::sources::obsidian::code_block_ranges;

/// How many directory entries one embed lookup reads before it gives up.
/// A pages folder is a few thousand files; this keeps a huge or looping tree
/// from stalling a page read.
const MAX_SCANNED_ENTRIES: usize = 50_000;

/// The page's tags, in the order they first appear, each once (compared
/// without case, keeping the first spelling). Front matter `tags:` come
/// first, then `#tags` in the body.
pub fn page_tags(content: &str) -> Vec<String> {
    let (front, body) = split_front_matter(content);
    let mut seen = HashSet::new();
    let mut tags = Vec::new();
    let mut push = |tag: &str| {
        let tag = tag.trim().trim_start_matches('#').trim_end_matches('/');
        if is_tag(tag) && seen.insert(tag.to_lowercase()) {
            tags.push(tag.to_string());
        }
    };
    if let Some(front) = front {
        for tag in front_matter_tags(front) {
            push(&tag);
        }
    }
    for tag in body_tags(body) {
        push(tag);
    }
    tags
}

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '/')
}

fn is_tag(tag: &str) -> bool {
    !tag.is_empty() && tag.chars().all(is_tag_char) && !tag.chars().all(|c| c.is_ascii_digit())
}

/// `---` front matter at the very top, and the body after it.
fn split_front_matter(content: &str) -> (Option<&str>, &str) {
    let Some(rest) = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
    else {
        return (None, content);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return (Some(&rest[..offset]), &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    (None, content)
}

/// `tags:` as a YAML list, a flow list, or one comma or space separated
/// string; `tag:` too, as Obsidian accepts it.
fn front_matter_tags(front: &str) -> Vec<String> {
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(front) else {
        return Vec::new();
    };
    let mut tags = Vec::new();
    for key in ["tags", "tag"] {
        match value.get(key) {
            Some(serde_yaml::Value::Sequence(items)) => tags.extend(
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string)),
            ),
            Some(serde_yaml::Value::String(text)) => tags.extend(
                text.split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_string),
            ),
            _ => {}
        }
    }
    tags
}

/// Byte ranges of inline code spans (`` `like this` ``) outside fenced code.
fn inline_code_ranges(content: &str, fenced: &[Range<usize>]) -> Vec<Range<usize>> {
    let bytes = content.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'`' || fenced.iter().any(|r| r.contains(&i)) {
            i += 1;
            continue;
        }
        let ticks = bytes[i..].iter().take_while(|&&b| b == b'`').count();
        let open = i;
        let mut j = i + ticks;
        let mut close = None;
        while j < bytes.len() {
            if bytes[j] == b'`' {
                let run = bytes[j..].iter().take_while(|&&b| b == b'`').count();
                if run == ticks {
                    close = Some(j + run);
                    break;
                }
                j += run;
            } else {
                j += 1;
            }
        }
        match close {
            Some(end) => {
                ranges.push(open..end);
                i = end;
            }
            None => i += ticks,
        }
    }
    ranges
}

fn body_tags(body: &str) -> Vec<&str> {
    let fenced = code_block_ranges(body);
    let inline = inline_code_ranges(body, &fenced);
    let in_code = |at: usize| fenced.iter().chain(&inline).any(|r| r.contains(&at));
    let mut tags = Vec::new();
    for (at, _) in body.match_indices('#') {
        let starts_word = body[..at]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        if !starts_word || in_code(at) {
            continue;
        }
        let rest = &body[at + 1..];
        let end = rest.find(|c: char| !is_tag_char(c)).unwrap_or(rest.len());
        let tag = &rest[..end];
        if is_tag(tag.trim_end_matches('/')) {
            tags.push(tag);
        }
    }
    tags
}

/// A file an `![[embed]]` names, found in the pages folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEmbed {
    /// Path from the pages folder, with `/` between folders.
    pub relative: String,
    pub absolute: PathBuf,
    pub kind: AttachmentKind,
}

fn hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// The embed target as safe path parts: no `..`, no root, no hidden folder.
fn target_parts(target: &str) -> Option<Vec<String>> {
    let target = target.trim().replace('\\', "/");
    let mut parts = Vec::new();
    for component in Path::new(&target).components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str()?;
                if hidden(part) {
                    return None;
                }
                parts.push(part.to_string());
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then_some(parts)
}

/// `root/<relative>` is a regular file reached through real folders only: no
/// part of the path is a symbolic link.
fn regular_file(root: &Path, relative: &str) -> bool {
    let parts: Vec<&str> = relative.split('/').collect();
    let mut path = root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        path.push(part);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            return false;
        };
        let last = index + 1 == parts.len();
        let ok = if last {
            meta.file_type().is_file()
        } else {
            meta.file_type().is_dir()
        };
        if !ok {
            return false;
        }
    }
    true
}

/// Find the file `![[target]]` shows, for a page whose own file is
/// `page_file` (a path from the pages folder, when the page has one).
///
/// `target` may carry a `|size` or `#heading` part, which is dropped. Only an
/// image or video is resolved. Symbolic links are never followed, and nothing
/// outside `root` is read.
pub fn resolve_embed(root: &Path, page_file: Option<&str>, target: &str) -> Option<ResolvedEmbed> {
    let target = target.split(['|', '#']).next().unwrap_or_default();
    let parts = target_parts(target)?;
    let name = parts.last()?.clone();
    let kind = kind_for_name(&name)?;
    let found = |relative: String| {
        regular_file(root, &relative).then(|| ResolvedEmbed {
            absolute: root.join(&relative),
            relative,
            kind,
        })
    };

    // 1. An exact path from the pages folder.
    if let Some(hit) = found(parts.join("/")) {
        return Some(hit);
    }
    // 2. The same path from the page's own folder.
    let page_dir: Vec<String> = page_file
        .and_then(target_parts)
        .map(|mut page| {
            page.pop();
            page
        })
        .unwrap_or_default();
    if !page_dir.is_empty() {
        let mut beside = page_dir.clone();
        beside.extend(parts.iter().cloned());
        if let Some(hit) = found(beside.join("/")) {
            return Some(hit);
        }
    }
    // 3. A bare name anywhere: the page's folder first, then the shortest
    //    path, then the first in name order so the answer is stable.
    if parts.len() != 1 {
        return None;
    }
    let mut best: Option<(bool, usize, String)> = None;
    let mut stack = vec![(PathBuf::new(), 0usize)];
    let mut scanned = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(root.join(&dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            scanned += 1;
            if scanned > MAX_SCANNED_ENTRIES {
                break;
            }
            let Some(entry_name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if hidden(&entry_name) {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let relative = dir.join(&entry_name);
            if file_type.is_dir() {
                stack.push((relative, depth + 1));
            } else if file_type.is_file() && entry_name == name {
                let relative = relative
                    .components()
                    .filter_map(|c| c.as_os_str().to_str())
                    .collect::<Vec<_>>()
                    .join("/");
                let in_page_dir = relative
                    .rsplit_once('/')
                    .map_or(page_dir.is_empty(), |(folder, _)| {
                        folder == page_dir.join("/")
                    });
                let rank = (!in_page_dir, depth, relative);
                if best.as_ref().is_none_or(|current| rank < *current) {
                    best = Some(rank);
                }
            }
        }
    }
    best.and_then(|(_, _, relative)| found(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_tags_are_read_as_obsidian_reads_them() {
        let content = "# Heading\n\
                       Reading #books and #读书 today.\n\
                       #project/wenlan starts a line. Not a tag: a#b, #123, # space.\n\
                       `#code` stays code.\n\
                       ```\n#fenced\n```\n\
                       Again #Books, then #2026-plans.";
        assert_eq!(
            page_tags(content),
            vec!["books", "读书", "project/wenlan", "2026-plans"]
        );
    }

    #[test]
    fn front_matter_tags_come_first_and_count_once() {
        let content = "---\ntags: [Rust, notes]\ntitle: T\n---\nAbout #rust and #wiki.";
        assert_eq!(page_tags(content), vec!["Rust", "notes", "wiki"]);
        let content = "---\ntags: rust, notes\n---\nBody.";
        assert_eq!(page_tags(content), vec!["rust", "notes"]);
        let content = "---\ntags:\n  - one\n  - \"#two\"\n---\n";
        assert_eq!(page_tags(content), vec!["one", "two"]);
    }

    #[test]
    fn a_page_without_tags_has_none() {
        assert!(page_tags("Plain text. A heading:\n\n## Two").is_empty());
        assert!(page_tags("See https://example.com/#anchor").is_empty());
    }

    fn touch(root: &Path, relative: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn an_embed_resolves_like_obsidian() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(root, "_attachments/cat.png");
        touch(root, "trips/cat.png");
        touch(root, "trips/2026/deep/dog.jpg");
        touch(root, "a/dog.jpg");
        touch(root, ".obsidian/bird.png");
        touch(root, "notes.md");

        // Exact path from the pages folder.
        let hit = resolve_embed(root, None, "trips/cat.png").unwrap();
        assert_eq!(hit.relative, "trips/cat.png");
        assert_eq!(hit.kind, AttachmentKind::Image);
        // A bare name prefers the page's own folder…
        let hit = resolve_embed(root, Some("trips/Trip.md"), "cat.png|300").unwrap();
        assert_eq!(hit.relative, "trips/cat.png");
        // …and is otherwise the shortest path.
        let hit = resolve_embed(root, Some("Home.md"), "dog.jpg").unwrap();
        assert_eq!(hit.relative, "a/dog.jpg");
        // Hidden folders, other kinds, escapes and missing files resolve to nothing.
        assert!(resolve_embed(root, None, "bird.png").is_none());
        assert!(resolve_embed(root, None, ".obsidian/bird.png").is_none());
        assert!(resolve_embed(root, None, "notes.md").is_none());
        assert!(resolve_embed(root, None, "../etc/passwd.png").is_none());
        assert!(resolve_embed(root, None, "/etc/cat.png").is_none());
        assert!(resolve_embed(root, None, "missing.png").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn an_embed_never_follows_a_symbolic_link() {
        let outside = tempfile::tempdir().unwrap();
        touch(outside.path(), "secret.png");
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.png"),
            dir.path().join("secret.png"),
        )
        .unwrap();
        assert!(resolve_embed(dir.path(), None, "secret.png").is_none());
        assert!(resolve_embed(dir.path(), None, "link/secret.png").is_none());
    }
}
