// SPDX-License-Identifier: AGPL-3.0-only
//! Managed home for loose uploaded files (§2). The daemon indexes files in
//! place and stores no blobs, so the ONLY copies the app makes are loose
//! single-file uploads staged here for the daemon's 30s scheduler.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// `~/.wenlan/sources` — aligned under the daemon's knowledge home, next to
/// `pages/`.
/// ponytail: hardcoded to `~/.wenlan`; the daemon's knowledge_path can in
/// principle be customized but is not exposed via any API today. Verified
/// safe: this subdir is not a reserved ingest root and indexes normally.
pub fn sources_dir() -> PathBuf {
    // Through `identity_paths`, not `dirs`, because `place_upload` below
    // *creates* this directory: a unit test that reached it would file uploads
    // into the developer's real `~/.wenlan`. An isolated dev run keeps its
    // uploads in its own state directory for the same reason.
    if let Some(state) = crate::identity_paths::isolated_dev_state_dir() {
        return state.join("sources");
    }
    crate::identity_paths::home_base().join(".wenlan/sources")
}

/// Formats the daemon reads from the uploads folder, with the size it accepts
/// for each (matching `wenlan_core::sources::directory`). A bigger file would
/// be staged and then silently never read, so it is refused up front instead.
fn document_size_limit(ext: &str) -> Option<u64> {
    match ext {
        "md" | "txt" => Some(1024 * 1024),
        "pdf" => Some(10 * 1024 * 1024),
        "docx" => Some(wenlan_core::sources::docx::MAX_DOCX_SIZE),
        _ => None,
    }
}

/// A zip with more entries than this is not a folder of documents someone
/// zipped up; nothing in it is read.
const MAX_ZIP_ENTRIES: usize = 20_000;

fn lower_ext(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// What one round of picking files did, so the app can say it once at the end.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadOutcome {
    /// Files now in the uploads folder that were not there before.
    pub added: Vec<String>,
    /// Files whose exact content was already there (picked twice, or both on
    /// their own and inside a zip): kept once, not copied again.
    pub already: Vec<String>,
    /// Files left out, each with why, so nothing goes missing silently.
    pub skipped: Vec<SkippedUpload>,
    /// A zip that is an AI chat export (ChatGPT/Claude `conversations.json`):
    /// those go through the past-chats import, not here.
    pub chat_exports: Vec<String>,
}

/// Why a picked file was left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkipReason {
    /// Not PDF, Word, Markdown or TXT (or an unreadable zip).
    Format,
    /// Bigger than the daemon reads (1 MB text, 10 MB PDF/Word).
    TooBig,
    /// A PDF or Word file with no text to read, such as a scanned PDF.
    NoText,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SkippedUpload {
    pub name: String,
    pub reason: SkipReason,
}

/// The daemon drops a document with fewer meaningful words than this
/// (`MIN_EXTRACTED_WORDS` in `wenlan_core::sources::directory`).
const MIN_WORDS: usize = 5;

/// Whether a PDF or Word file has text the daemon will keep. A scanned PDF
/// (pictures of pages) has none: without this check it would be copied and
/// then never show up, with nothing said.
fn has_readable_text(ext: &str, bytes: &[u8]) -> bool {
    let text = match ext {
        "pdf" => wenlan_core::sources::directory::extract_pdf_text(bytes).ok(),
        "docx" => wenlan_core::sources::docx::extract_docx_text(bytes).ok(),
        _ => return true,
    };
    text.is_some_and(|t| wenlan_core::quality_gate::meaningful_word_count(&t) >= MIN_WORDS)
}

/// A name in `folder` for a new document called `name`. Two different files
/// that share a name (two `paper.pdf` from different folders) both stay:
/// the later one becomes `paper (2).pdf` instead of replacing the first.
fn free_name(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| folder.join(format!("{stem} ({n}){ext}")))
        .find(|candidate| !candidate.exists())
        .expect("an unused name exists")
}

impl UploadOutcome {
    fn skip(&mut self, name: String, reason: SkipReason) {
        self.skipped.push(SkippedUpload { name, reason });
    }
}

/// Content fingerprints of every file already in the uploads folder, so the
/// same document picked again is kept once, whatever it is called.
fn existing_fingerprints(dir: &Path) -> HashSet<[u8; 32]> {
    let mut seen = HashSet::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file()
                && document_size_limit(&lower_ext(&name.to_string_lossy())).is_some()
            {
                if let Ok(bytes) = fs::read(&path) {
                    seen.insert(fingerprint(&bytes));
                }
            }
        }
    }
    seen
}

fn fingerprint(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}

/// Write `bytes` to `dest` atomically (temp name in the same folder, then
/// rename), so the daemon's scheduler never reads a half-written file.
fn write_atomically(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = dest.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "destination has no folder",
        )
    })?;
    fs::create_dir_all(parent)?;
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = parent.join(format!(".{name}.tmp"));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, dest)
}

/// A folder name for a zip's contents: the zip's own name, without anything
/// that would hide it from the daemon's scan or leave the uploads folder.
fn zip_folder_name(zip: &Path) -> String {
    let stem = zip
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '\0') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    if cleaned.is_empty() {
        "zip".to_string()
    } else {
        cleaned
    }
}

/// Stage everything picked in one go: loose documents, and the documents
/// inside any zip (kept in a folder named after the zip, with its own
/// folders). The same content is kept once. Nothing is registered here; the
/// caller registers the uploads folder once afterwards.
pub fn stage_uploads(sources_dir: &Path, picked: &[PathBuf]) -> std::io::Result<UploadOutcome> {
    fs::create_dir_all(sources_dir)?;
    let gitignore = sources_dir.join(".gitignore");
    if !gitignore.exists() {
        fs::write(&gitignore, "*\n")?;
    }
    let mut seen = existing_fingerprints(sources_dir);
    let mut outcome = UploadOutcome::default();
    for path in picked {
        let display = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let ext = lower_ext(&display);
        if ext == "zip" {
            stage_zip(sources_dir, path, &display, &mut seen, &mut outcome)?;
            continue;
        }
        let Some(limit) = document_size_limit(&ext) else {
            outcome.skip(display, SkipReason::Format);
            continue;
        };
        let Ok(meta) = fs::metadata(path) else {
            outcome.skip(display, SkipReason::Format);
            continue;
        };
        if !meta.is_file() {
            outcome.skip(display, SkipReason::Format);
            continue;
        }
        if meta.len() > limit {
            outcome.skip(display, SkipReason::TooBig);
            continue;
        }
        let bytes = fs::read(path)?;
        let print = fingerprint(&bytes);
        if seen.contains(&print) {
            outcome.already.push(display);
            continue;
        }
        if !has_readable_text(&ext, &bytes) {
            outcome.skip(display, SkipReason::NoText);
            continue;
        }
        seen.insert(print);
        let dest = free_name(sources_dir, &display);
        write_atomically(&dest, &bytes)?;
        outcome.added.push(
            dest.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(display),
        );
    }
    Ok(outcome)
}

fn stage_zip(
    sources_dir: &Path,
    zip_path: &Path,
    display: &str,
    seen: &mut HashSet<[u8; 32]>,
    outcome: &mut UploadOutcome,
) -> std::io::Result<()> {
    use std::io::Read;
    let Ok(file) = fs::File::open(zip_path) else {
        outcome.skip(display.to_string(), SkipReason::Format);
        return Ok(());
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        outcome.skip(display.to_string(), SkipReason::Format);
        return Ok(());
    };
    if archive.len() > MAX_ZIP_ENTRIES {
        outcome.skip(display.to_string(), SkipReason::TooBig);
        return Ok(());
    }
    // A ChatGPT or Claude export keeps its chats in a top-level
    // conversations.json; reading its stray files as documents would bury
    // the chats, so the whole zip goes to the past-chats import instead.
    if archive
        .file_names()
        .any(|n| n.trim_start_matches("./") == "conversations.json")
    {
        outcome.chat_exports.push(display.to_string());
        return Ok(());
    }
    let folder = sources_dir.join(zip_folder_name(zip_path));
    for i in 0..archive.len() {
        let Ok(mut entry) = archive.by_index(i) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        // `enclosed_name` refuses absolute paths and `..`, so nothing in the
        // zip can land outside its folder.
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let hidden = relative.components().any(|c| {
            let part = c.as_os_str().to_string_lossy();
            part.starts_with('.') || part == "__MACOSX"
        });
        if hidden {
            continue;
        }
        let name = relative
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let shown = format!(
            "{display}/{}",
            relative.to_string_lossy().replace('\\', "/")
        );
        let ext = lower_ext(&name);
        let Some(limit) = document_size_limit(&ext) else {
            outcome.skip(shown, SkipReason::Format);
            continue;
        };
        if entry.size() > limit {
            outcome.skip(shown, SkipReason::TooBig);
            continue;
        }
        // The declared size can lie; never read more than the limit.
        let mut bytes = Vec::new();
        if (&mut entry)
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() as u64 > limit
        {
            outcome.skip(shown, SkipReason::TooBig);
            continue;
        }
        let print = fingerprint(&bytes);
        if seen.contains(&print) {
            outcome.already.push(shown);
            continue;
        }
        if !has_readable_text(&ext, &bytes) {
            outcome.skip(shown, SkipReason::NoText);
            continue;
        }
        seen.insert(print);
        // Re-importing the same zip with an edited file updates that file in
        // place: the zip's folder mirrors the zip.
        write_atomically(&folder.join(&relative), &bytes)?;
        outcome.added.push(shown);
    }
    Ok(())
}

/// Delete one uploaded file, and any folder a zip left that is now empty.
/// Only a file inside the uploads folder can go: a document read from one of
/// the person's own folders is never deleted from here.
pub fn remove_upload(sources_dir: &Path, file: &Path) -> std::io::Result<()> {
    let root = fs::canonicalize(sources_dir)?;
    let target = fs::canonicalize(file)?;
    if target == root || !target.starts_with(&root) || !target.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "only a file that was added with Add your material can be deleted here",
        ));
    }
    fs::remove_file(&target)?;
    let mut parent = target.parent().map(Path::to_path_buf);
    while let Some(dir) = parent {
        if dir == root || fs::remove_dir(&dir).is_err() {
            break;
        }
        parent = dir.parent().map(Path::to_path_buf);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        for (name, body) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut zip, body).unwrap();
        }
        zip.finish().unwrap();
    }

    /// A one-page PDF whose page says `text` (Helvetica, no compression).
    fn pdf_with_text(text: &str) -> Vec<u8> {
        let stream = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for o in offsets {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        out.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .bytes(),
        );
        out
    }

    #[test]
    fn three_pdfs_with_the_same_name_all_stay() {
        // Three different PDFs, all called paper.pdf (downloaded into
        // different folders): before, each replaced the last and one showed.
        let tmp = tempfile::tempdir().unwrap();
        let sources = tmp.path().join("sources");
        let mut picked = Vec::new();
        for (i, words) in [
            "alpha beta gamma delta epsilon",
            "one two three four five six",
            "red green blue yellow purple",
        ]
        .iter()
        .enumerate()
        {
            let dir = tmp.path().join(format!("d{i}"));
            std::fs::create_dir_all(&dir).unwrap();
            let p = dir.join("paper.pdf");
            std::fs::write(&p, pdf_with_text(words)).unwrap();
            picked.push(p);
        }
        let out = stage_uploads(&sources, &picked).unwrap();
        assert_eq!(
            out.added,
            vec!["paper.pdf", "paper (2).pdf", "paper (3).pdf"],
            "{out:?}"
        );
        assert!(out.skipped.is_empty());
        for name in &out.added {
            assert!(sources.join(name).is_file());
        }
    }

    #[test]
    fn a_pdf_with_no_text_is_named_not_lost() {
        let tmp = tempfile::tempdir().unwrap();
        let sources = tmp.path().join("sources");
        let scanned = tmp.path().join("scan.pdf");
        std::fs::write(&scanned, pdf_with_text("")).unwrap();
        let out = stage_uploads(&sources, &[scanned]).unwrap();
        assert_eq!(
            out.skipped,
            vec![SkippedUpload {
                name: "scan.pdf".into(),
                reason: SkipReason::NoText
            }]
        );
        assert!(!sources.join("scan.pdf").exists());
    }

    #[test]
    fn several_files_and_a_zip_come_in_together_and_each_document_once() {
        let tmp = tempfile::tempdir().unwrap();
        let sources = tmp.path().join("sources");
        let a = tmp.path().join("a.md");
        let b = tmp.path().join("b.txt");
        let c = tmp.path().join("c.png");
        std::fs::write(&a, b"# A\nsome notes here").unwrap();
        std::fs::write(&b, b"plain text notes").unwrap();
        std::fs::write(&c, b"png").unwrap();
        let z = tmp.path().join("Course notes.zip");
        write_zip(
            &z,
            &[
                ("week1/intro.md", b"# Intro"),
                ("week1/copy-of-a.md", b"# A\nsome notes here"),
                ("week1/photo.jpg", b"jpg"),
                ("__MACOSX/week1/._intro.md", b"junk"),
                (".hidden/x.md", b"hidden"),
            ],
        );

        let out = stage_uploads(&sources, &[a.clone(), b, c, z.clone()]).unwrap();
        assert_eq!(
            out.added,
            vec!["a.md", "b.txt", "Course notes.zip/week1/intro.md"]
        );
        assert_eq!(out.already, vec!["Course notes.zip/week1/copy-of-a.md"]);
        let skipped: Vec<_> = out
            .skipped
            .iter()
            .map(|s| (s.name.as_str(), s.reason))
            .collect();
        assert_eq!(
            skipped,
            vec![
                ("c.png", SkipReason::Format),
                ("Course notes.zip/week1/photo.jpg", SkipReason::Format)
            ]
        );
        assert!(sources.join("Course notes/week1/intro.md").is_file());
        assert!(!sources.join("Course notes/week1/copy-of-a.md").exists());
        assert!(!sources.join("__MACOSX").exists());

        // Picking the same things again adds nothing and makes no copies.
        let again = stage_uploads(&sources, &[a, z]).unwrap();
        assert!(again.added.is_empty());
        assert_eq!(again.already.len(), 3);
        assert!(!sources.join("a (2).md").exists());
    }

    #[test]
    fn zip_entries_cannot_leave_their_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let sources = tmp.path().join("sources");
        let z = tmp.path().join("evil.zip");
        write_zip(&z, &[("../../escape.md", b"# out"), ("/abs.md", b"# abs")]);
        let out = stage_uploads(&sources, &[z]).unwrap();
        assert!(out.added.is_empty());
        assert!(!tmp.path().join("escape.md").exists());
    }

    #[test]
    fn an_ai_chat_export_zip_is_handed_to_the_chat_import() {
        let tmp = tempfile::tempdir().unwrap();
        let sources = tmp.path().join("sources");
        let z = tmp.path().join("export.zip");
        write_zip(&z, &[("conversations.json", b"[]"), ("notes.md", b"# n")]);
        let out = stage_uploads(&sources, &[z]).unwrap();
        assert_eq!(out.chat_exports, vec!["export.zip"]);
        assert!(out.added.is_empty());
    }

    #[test]
    fn too_big_text_is_left_out_not_staged() {
        let tmp = tempfile::tempdir().unwrap();
        let sources = tmp.path().join("sources");
        let big = tmp.path().join("big.txt");
        std::fs::write(&big, vec![b'a'; 1024 * 1024 + 1]).unwrap();
        let out = stage_uploads(&sources, &[big]).unwrap();
        assert_eq!(
            out.skipped,
            vec![SkippedUpload {
                name: "big.txt".into(),
                reason: SkipReason::TooBig
            }]
        );
        assert!(!sources.join("big.txt").exists());
    }

    #[test]
    fn an_uploaded_file_can_be_deleted_but_nothing_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let sources = tmp.path().join("sources");
        let z = tmp.path().join("Notes.zip");
        write_zip(&z, &[("a/b/only.md", b"# only one here")]);
        stage_uploads(&sources, &[z]).unwrap();
        let staged = sources.join("Notes/a/b/only.md");
        assert!(staged.is_file());

        remove_upload(&sources, &staged).unwrap();
        assert!(!staged.exists());
        assert!(!sources.join("Notes").exists(), "empty zip folders go too");
        assert!(sources.is_dir(), "the uploads folder itself stays");

        let mine = tmp.path().join("mine.md");
        std::fs::write(&mine, b"# mine").unwrap();
        assert!(remove_upload(&sources, &mine).is_err());
        assert!(mine.exists());
        let sneaky = sources.join("../mine.md");
        assert!(remove_upload(&sources, &sneaky).is_err());
        assert!(mine.exists());
    }
}
