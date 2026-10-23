//! Shared rules for images and videos placed in pages: the folder they live in,
//! which extensions count, and how a page's Markdown names them. The daemon,
//! MCP server and app read pages the same way through these.

/// Folder under the pages root that holds attached files. Pages link to it from
/// the root, `![alt](_attachments/name.png)`, the vault-root form Obsidian reads.
pub const ATTACHMENTS_DIR: &str = "_attachments";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    Video,
}

impl AttachmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AttachmentKind::Image => "image",
            AttachmentKind::Video => "video",
        }
    }
}

/// The kind a lowercased extension names, or `None` when it is not supported.
pub fn kind_for_extension(extension: &str) -> Option<AttachmentKind> {
    match extension {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "avif" | "heic" => {
            Some(AttachmentKind::Image)
        }
        "mp4" | "m4v" | "mov" | "webm" => Some(AttachmentKind::Video),
        _ => None,
    }
}

/// The kind of an attachment file name, by its extension.
pub fn kind_for_name(name: &str) -> Option<AttachmentKind> {
    let (_, extension) = name.rsplit_once('.')?;
    kind_for_extension(&extension.to_ascii_lowercase())
}

fn name_is_valid(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\'])
}

/// Attachment file names a page's Markdown shows as `![alt](_attachments/name)`
/// (or `(<_attachments/name>)`), in order, without repeats.
pub fn referenced_attachments(content: &str) -> Vec<String> {
    let prefix = format!("{ATTACHMENTS_DIR}/");
    let mut names: Vec<String> = Vec::new();
    for (at, _) in content.match_indices("](") {
        let target = &content[at + 2..];
        let (target, closer) = match target.strip_prefix('<') {
            Some(inner) => (inner, '>'),
            None => (target, ')'),
        };
        let Some(tail) = target.strip_prefix(prefix.as_str()) else {
            continue;
        };
        // `<...>` may hold spaces; a bare target ends at `)` or before a title.
        let end = if closer == '>' {
            tail.find(['>', '\n'])
        } else {
            tail.find(|c: char| c == ')' || c.is_whitespace())
        }
        .unwrap_or(tail.len());
        let next = tail[end..].chars().next();
        if !(next == Some(closer) || (closer == ')' && next.is_some_and(|c| c == ' '))) {
            continue;
        }
        // Only an image link: an unclosed `![` opens the label on this line.
        let line_start = content[..at].rfind('\n').map_or(0, |i| i + 1);
        let label = &content[line_start..at];
        let Some(open) = label.rfind("![") else {
            continue;
        };
        if label[open + 2..].contains(']') {
            continue;
        }
        let name = &tail[..end];
        if name_is_valid(name) && !names.iter().any(|seen| seen == name) {
            names.push(name.to_string());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_attachment_links_in_page_markdown() {
        let content = "Intro\n\n![Home](_attachments/home-abc.png)\n\n![film](<_attachments/film 1.mp4>)\n![Home again](_attachments/home-abc.png)\n![web](https://example.com/x.png)\n![escape](_attachments/../secret.png)\n[plain link](_attachments/doc.png)\n![titled](_attachments/t.gif \"A title\")";
        assert_eq!(
            referenced_attachments(content),
            vec!["home-abc.png", "film 1.mp4", "t.gif"]
        );
    }

    #[test]
    fn tells_images_from_videos() {
        assert_eq!(kind_for_name("a.PNG"), Some(AttachmentKind::Image));
        assert_eq!(kind_for_name("a.mov"), Some(AttachmentKind::Video));
        assert_eq!(kind_for_name("a.pdf"), None);
        assert_eq!(kind_for_name("noext"), None);
    }
}
