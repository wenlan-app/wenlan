// SPDX-License-Identifier: Apache-2.0
//! Images and videos placed in pages.
//!
//! A page shows a file by plain Markdown, `![alt](_attachments/name.png)`, the
//! same link Obsidian resolves from the vault root. The file itself is copied
//! once into `_attachments/` under the pages folder, named by its stem and a
//! content-hash prefix, so the same file attached twice is stored once and the
//! page keeps working after the original moves. Nothing is converted.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub use wenlan_types::attachments::{
    kind_for_extension, kind_for_name, referenced_attachments, AttachmentKind, ATTACHMENTS_DIR,
};

use crate::WenlanError;

const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_VIDEO_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_STEM_CHARS: usize = 60;
const HASH_PREFIX_CHARS: usize = 10;

fn max_bytes(kind: AttachmentKind) -> u64 {
    match kind {
        AttachmentKind::Image => MAX_IMAGE_BYTES,
        AttachmentKind::Video => MAX_VIDEO_BYTES,
    }
}

/// A file stored under `_attachments/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAttachment {
    /// File name inside `_attachments/`.
    pub name: String,
    /// Root-relative link target, `_attachments/<name>`.
    pub path: String,
    pub absolute_path: PathBuf,
    pub kind: AttachmentKind,
    pub bytes: u64,
    /// Whether this call copied the file (false when it was already stored).
    pub created: bool,
}

impl StoredAttachment {
    /// The Markdown line that shows this file in a page.
    pub fn markdown(&self, alt: Option<&str>) -> String {
        let alt = alt
            .map(clean_alt)
            .filter(|alt| !alt.is_empty())
            .unwrap_or_else(|| default_alt(&self.name));
        format!("![{alt}]({})", self.path)
    }
}

fn clean_alt(alt: &str) -> String {
    alt.chars()
        .map(|c| {
            if matches!(c, '[' | ']' | '\n' | '\r') {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn default_alt(name: &str) -> String {
    let stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
    // Drop the "-<hash>" suffix this module appended.
    let stem = stem
        .rsplit_once('-')
        .filter(|(_, hash)| hash.len() == HASH_PREFIX_CHARS)
        .map(|(stem, _)| stem)
        .unwrap_or(stem);
    clean_alt(&stem.replace(['-', '_'], " "))
}

fn invalid(reason: &str) -> WenlanError {
    WenlanError::Validation(reason.to_string())
}

fn lowercase_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
}

/// Keep letters (any script), digits, `-` and `_`; everything else becomes `-`.
fn safe_stem(path: &Path) -> String {
    let raw = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let mut out = String::new();
    for c in raw.chars() {
        let c = if c.is_alphanumeric() || c == '_' {
            c
        } else {
            '-'
        };
        if c == '-' && (out.is_empty() || out.ends_with('-')) {
            continue;
        }
        out.push(c);
        if out.chars().count() >= MAX_STEM_CHARS {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "file".to_string()
    } else {
        out
    }
}

/// Whether the first bytes look like the claimed kind. Extensions alone are not
/// trusted: this folder syncs to other apps, so only real media lands in it.
fn content_matches(extension: &str, head: &[u8]) -> bool {
    let ftyp = head.len() >= 12 && &head[4..8] == b"ftyp";
    match extension {
        "png" => head.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => head.starts_with(&[0xFF, 0xD8, 0xFF]),
        "gif" => head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a"),
        "webp" => head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP",
        "avif" | "heic" | "mp4" | "m4v" | "mov" => {
            ftyp || (extension == "mov"
                && head.len() >= 8
                && matches!(&head[4..8], b"moov" | b"wide" | b"mdat" | b"free"))
        }
        "webm" => head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]),
        "svg" => {
            let text = String::from_utf8_lossy(head);
            text.contains("<svg")
        }
        _ => false,
    }
}

/// Copy `source` into `<pages_root>/_attachments/`, reusing an identical file
/// already stored there.
pub fn store_attachment(pages_root: &Path, source: &Path) -> Result<StoredAttachment, WenlanError> {
    let extension =
        lowercase_extension(source).ok_or_else(|| invalid("attachment_type_unsupported"))?;
    let kind =
        kind_for_extension(&extension).ok_or_else(|| invalid("attachment_type_unsupported"))?;
    let metadata = fs::metadata(source).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => invalid("attachment_not_found"),
        _ => WenlanError::Io(error),
    })?;
    if !metadata.is_file() {
        return Err(invalid("attachment_not_a_file"));
    }
    let bytes = metadata.len();
    if bytes == 0 {
        return Err(invalid("attachment_empty"));
    }
    if bytes > max_bytes(kind) {
        return Err(invalid("attachment_too_large"));
    }

    let mut file = fs::File::open(source)?;
    let mut head = vec![0u8; 512];
    let read = read_up_to(&mut file, &mut head)?;
    head.truncate(read);
    if !content_matches(&extension, &head) {
        return Err(invalid("attachment_content_mismatch"));
    }

    let mut hasher = Sha256::new();
    hasher.update(&head);
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    let digest = format!("{:x}", hasher.finalize());
    let name = format!(
        "{}-{}.{}",
        safe_stem(source),
        &digest[..HASH_PREFIX_CHARS],
        extension
    );

    let directory = pages_root.join(ATTACHMENTS_DIR);
    fs::create_dir_all(&directory)?;
    let destination = directory.join(&name);
    let path = format!("{ATTACHMENTS_DIR}/{name}");
    if let Ok(existing) = fs::symlink_metadata(&destination) {
        if existing.is_file() && existing.len() == bytes {
            return Ok(StoredAttachment {
                name,
                path,
                absolute_path: destination,
                kind,
                bytes,
                created: false,
            });
        }
        return Err(WenlanError::Conflict("attachment_name_taken".into()));
    }

    // Copy beside the target, then rename, so a page never shows a half file.
    let temporary = directory.join(format!(".{name}.{}.part", uuid::Uuid::new_v4()));
    let copied = (|| -> Result<(), WenlanError> {
        let mut input = fs::File::open(source)?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        std::io::copy(&mut input, &mut output)?;
        output.flush()?;
        output.sync_all()?;
        Ok(())
    })();
    if let Err(error) = copied {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, &destination) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(StoredAttachment {
        name,
        path,
        absolute_path: destination,
        kind,
        bytes,
        created: true,
    })
}

fn read_up_to(file: &mut fs::File, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        let n = file.read(&mut buffer[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn stores_once_and_reuses_identical_files() {
        let source = tempfile::tempdir().unwrap();
        let pages = tempfile::tempdir().unwrap();
        let first = write(source.path(), "Home Screen (1).png", PNG);
        let stored = store_attachment(pages.path(), &first).unwrap();
        assert!(stored.created);
        assert!(stored.name.starts_with("Home-Screen-1-"));
        assert!(stored.path.starts_with("_attachments/"));
        assert_eq!(fs::read(&stored.absolute_path).unwrap(), PNG);

        let copy = write(source.path(), "Home Screen (1).PNG", PNG);
        let again = store_attachment(pages.path(), &copy).unwrap();
        assert!(!again.created);
        assert_eq!(again.name, stored.name);
        let leftovers: Vec<_> = fs::read_dir(pages.path().join(ATTACHMENTS_DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(leftovers, vec![stored.name.clone()]);
    }

    #[test]
    fn keeps_non_latin_names_readable() {
        let source = tempfile::tempdir().unwrap();
        let pages = tempfile::tempdir().unwrap();
        let path = write(source.path(), "首頁 截圖.png", PNG);
        let stored = store_attachment(pages.path(), &path).unwrap();
        assert!(stored.name.starts_with("首頁-截圖-"), "{}", stored.name);
        assert_eq!(
            stored.markdown(None),
            format!("![首頁 截圖]({})", stored.path)
        );
        assert_eq!(
            stored.markdown(Some("Home [after] import")),
            format!("![Home after import]({})", stored.path)
        );
    }

    #[test]
    fn refuses_files_that_are_not_media() {
        let source = tempfile::tempdir().unwrap();
        let pages = tempfile::tempdir().unwrap();
        let fake = write(
            source.path(),
            "id_rsa.png",
            b"-----BEGIN OPENSSH PRIVATE KEY-----",
        );
        assert!(
            matches!(store_attachment(pages.path(), &fake), Err(WenlanError::Validation(r)) if r == "attachment_content_mismatch")
        );
        let text = write(source.path(), "notes.txt", b"hello");
        assert!(
            matches!(store_attachment(pages.path(), &text), Err(WenlanError::Validation(r)) if r == "attachment_type_unsupported")
        );
        assert!(
            matches!(store_attachment(pages.path(), &source.path().join("missing.png")), Err(WenlanError::Validation(r)) if r == "attachment_not_found")
        );
        assert!(
            !pages.path().join(ATTACHMENTS_DIR).exists()
                || fs::read_dir(pages.path().join(ATTACHMENTS_DIR))
                    .unwrap()
                    .next()
                    .is_none()
        );
    }

    #[test]
    fn accepts_common_video_containers() {
        let source = tempfile::tempdir().unwrap();
        let pages = tempfile::tempdir().unwrap();
        let mp4 = write(source.path(), "film.mp4", b"\0\0\0\x18ftypmp42\0\0\0\0");
        let webm = write(source.path(), "clip.webm", &[0x1A, 0x45, 0xDF, 0xA3, 0x9F]);
        assert_eq!(
            store_attachment(pages.path(), &mp4).unwrap().kind,
            AttachmentKind::Video
        );
        assert_eq!(
            store_attachment(pages.path(), &webm).unwrap().kind,
            AttachmentKind::Video
        );
    }
}
