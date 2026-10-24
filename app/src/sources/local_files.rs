// SPDX-License-Identifier: AGPL-3.0-only
//! Local filesystem source connector.
//! Moved from origin-core::sources::local_files; app-only after Phase 5-D PR2.
use crate::error::AppError;
use crate::sources::data_source::DataSource;
use async_trait::async_trait;
use std::any::Any;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use wenlan_core::sources::docx::{read_docx_file, MAX_DOCX_SIZE};
use wenlan_types::sources::{RawDocument, SourceStatus};

const DOCUMENT_EXTENSIONS: &[&str] = &["txt", "md", "csv", "log", "pdf", "docx", "rtf"];

const SOURCE_CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "tsx", "jsx", "json", "toml", "yaml", "yml", "go", "java", "c", "cpp",
    "h", "css", "html", "sh", "rb", "php", "swift", "kt", "scala",
];

const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".next",
    "__pycache__",
    ".venv",
    "venv",
    "dist",
    "build",
    ".cache",
];

fn max_file_size(ext: &str) -> u64 {
    match ext {
        "docx" => MAX_DOCX_SIZE,
        "pdf" => 10_485_760, // 10 MB
        _ => 1_048_576,      // 1 MB
    }
}

#[derive(Default)]
pub struct LocalFilesSource {
    watch_paths: Vec<PathBuf>,
    last_sync: Option<i64>,
    document_count: u64,
}

impl LocalFilesSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_watch_path(&mut self, path: PathBuf) {
        if !self.watch_paths.contains(&path) {
            self.watch_paths.push(path);
        }
    }

    pub fn remove_watch_path(&mut self, path: &Path) {
        self.watch_paths.retain(|p| p != path);
    }

    /// Scan a directory recursively and collect all indexable files.
    pub fn scan_directory(dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let skip_dirs: HashSet<&str> = SKIP_DIRS.iter().copied().collect();
        Self::scan_recursive(dir, &skip_dirs, &mut files);
        files
    }

    fn scan_recursive(dir: &Path, skip_dirs: &HashSet<&str>, files: &mut Vec<PathBuf>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();

            if name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                if !skip_dirs.contains(name.as_ref()) {
                    Self::scan_recursive(&path, skip_dirs, files);
                }
                continue;
            }

            if Self::is_indexable(&path) {
                files.push(path);
            }
        }
    }

    pub fn is_indexable(path: &Path) -> bool {
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_lowercase(),
            None => return false,
        };

        let supported = DOCUMENT_EXTENSIONS
            .iter()
            .chain(SOURCE_CODE_EXTENSIONS.iter())
            .any(|&e| e == ext);

        if !supported {
            return false;
        }

        match std::fs::metadata(path) {
            Ok(meta) => meta.len() <= max_file_size(&ext),
            Err(_) => false,
        }
    }

    /// Read a file and create a RawDocument, dispatching on extension for binary formats.
    pub fn read_file(path: &Path) -> Result<RawDocument, AppError> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        let content = match ext.as_str() {
            "pdf" => {
                let bytes = std::fs::read(path).map_err(|e| AppError::Source {
                    source_name: "local_files".to_string(),
                    message: format!("Failed to read {}: {}", path.display(), e),
                })?;
                extract_pdf_text(&bytes, path)?
            }
            "docx" => read_docx_file(path).map_err(|e| AppError::Source {
                source_name: "local_files".to_string(),
                message: format!("Failed to read DOCX {}: {}", path.display(), e),
            })?,
            "rtf" => {
                let bytes = std::fs::read(path).map_err(|e| AppError::Source {
                    source_name: "local_files".to_string(),
                    message: format!("Failed to read {}: {}", path.display(), e),
                })?;
                extract_rtf_text(&bytes)
            }
            _ => std::fs::read_to_string(path).map_err(|e| AppError::Source {
                source_name: "local_files".to_string(),
                message: format!("Failed to read {}: {}", path.display(), e),
            })?,
        };

        if content.is_empty() {
            return Err(AppError::Source {
                source_name: "local_files".to_string(),
                message: format!("No text content extracted from {}", path.display()),
            });
        }

        let metadata = std::fs::metadata(path)?;
        let last_modified = metadata
            .modified()
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64
            })
            .unwrap_or(0);

        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_string())
            .unwrap_or_default();

        let mut metadata_map = std::collections::HashMap::new();
        if !ext.is_empty() {
            metadata_map.insert("extension".to_string(), ext);
        }

        Ok(RawDocument {
            source: "local_files".to_string(),
            source_id: path.to_string_lossy().to_string(),
            title,
            summary: None,
            content,
            url: Some(format!("file://{}", path.display())),
            last_modified,
            metadata: metadata_map,
            memory_type: None,
            source_agent: None,
            space: None,
            confidence: None,
            confirmed: None,
            supersedes: None,
            pending_revision: false,
            ..Default::default()
        })
    }
}

/// Extract text from a PDF through wenlan-core's extractor, which isolates
/// parser panics and scopes Form XObject font names before decoding (#617).
fn extract_pdf_text(bytes: &[u8], path: &Path) -> Result<String, AppError> {
    match wenlan_core::sources::directory::extract_pdf_text(bytes) {
        Ok(text) => {
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() {
                log::warn!(
                    "PDF has no extractable text (image-only?): {}",
                    path.display()
                );
            }
            Ok(trimmed)
        }
        Err(e) => {
            log::warn!("Failed to extract text from PDF {}: {}", path.display(), e);
            Ok(String::new())
        }
    }
}

/// Extract text from an RTF file using a simple state-machine stripper.
fn extract_rtf_text(bytes: &[u8]) -> String {
    let input = String::from_utf8_lossy(bytes);
    let mut result = String::new();
    let mut chars = input.chars().peekable();
    let mut depth: i32 = 0;
    let mut skip_depth: Option<i32> = None;

    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                depth += 1;
            }
            '}' => {
                if skip_depth == Some(depth) {
                    skip_depth = None;
                }
                depth -= 1;
                if depth < 0 {
                    depth = 0;
                }
            }
            _ if skip_depth.is_some() => continue,
            '\\' => {
                if let Some(&next) = chars.peek() {
                    if next.is_ascii_alphabetic() {
                        let mut word = String::new();
                        while let Some(&c) = chars.peek() {
                            if c.is_ascii_alphabetic() {
                                word.push(c);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        if let Some(&c) = chars.peek() {
                            if c == '-' || c.is_ascii_digit() {
                                chars.next();
                                while let Some(&d) = chars.peek() {
                                    if d.is_ascii_digit() {
                                        chars.next();
                                    } else {
                                        break;
                                    }
                                }
                            }
                        }
                        if chars.peek() == Some(&' ') {
                            chars.next();
                        }
                        match word.as_str() {
                            "fonttbl" | "colortbl" | "stylesheet" | "info" | "pict" | "header"
                            | "footer" | "headerl" | "headerr" | "footerl" | "footerr" => {
                                skip_depth = Some(depth);
                            }
                            "par" | "line" => result.push('\n'),
                            "tab" => result.push('\t'),
                            _ => {}
                        }
                    } else if next == '\'' {
                        chars.next();
                        let h1 = chars.next().unwrap_or('0');
                        let h2 = chars.next().unwrap_or('0');
                        if let Ok(byte) = u8::from_str_radix(&format!("{}{}", h1, h2), 16) {
                            if byte >= 0x20 {
                                result.push(byte as char);
                            }
                        }
                    } else if next == '\\' || next == '{' || next == '}' {
                        result.push(next);
                        chars.next();
                    } else {
                        chars.next();
                    }
                }
            }
            '\n' | '\r' => {}
            _ => {
                if depth >= 1 {
                    result.push(ch);
                }
            }
        }
    }

    result.trim().to_string()
}

#[async_trait]
impl DataSource for LocalFilesSource {
    fn name(&self) -> &str {
        "local_files"
    }

    fn requires_auth(&self) -> bool {
        false
    }

    async fn is_connected(&self) -> bool {
        !self.watch_paths.is_empty()
    }

    async fn connect(&mut self) -> Result<(), AppError> {
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), AppError> {
        self.watch_paths.clear();
        Ok(())
    }

    async fn fetch_updates(&mut self) -> Result<Vec<RawDocument>, AppError> {
        self.full_sync().await
    }

    async fn full_sync(&mut self) -> Result<Vec<RawDocument>, AppError> {
        let mut docs = Vec::new();
        for watch_path in &self.watch_paths {
            let files = Self::scan_directory(watch_path);
            for file_path in files {
                match Self::read_file(&file_path) {
                    Ok(doc) => docs.push(doc),
                    Err(e) => {
                        log::warn!("Skipping file {}: {}", file_path.display(), e);
                    }
                }
            }
        }
        Ok(docs)
    }

    async fn status(&self) -> SourceStatus {
        SourceStatus {
            name: "local_files".to_string(),
            connected: !self.watch_paths.is_empty(),
            requires_auth: false,
            last_sync: self.last_sync,
            document_count: self.document_count,
            error: None,
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{AppError, LocalFilesSource, MAX_DOCX_SIZE};
    use std::io::Write;
    use std::path::Path;

    fn write_docx(path: &Path, body: &str) {
        let file = std::fs::File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        for (name, xml) in [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
            ),
        ] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(xml.as_bytes()).unwrap();
        }
        archive
            .start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        write!(
            archive,
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#
        )
        .unwrap();
        archive.finish().unwrap();
    }

    fn assert_docx_source_error(path: &Path) -> String {
        match LocalFilesSource::read_file(path).unwrap_err() {
            AppError::Source {
                source_name,
                message,
            } => {
                assert_eq!(source_name, "local_files");
                assert!(message.contains(path.to_str().unwrap()), "{message}");
                message
            }
            error => panic!("Expected a local-files source error, got {error}"),
        }
    }

    #[test]
    fn docx_text_and_source_metadata_are_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("中文筆記.DOCX");
        write_docx(
            &path,
            r#"<w:p><w:r><w:t>中文 &amp; &lt;資料&gt;</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>欄一</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>欄二</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
        );

        assert!(LocalFilesSource::is_indexable(&path));
        assert_eq!(
            LocalFilesSource::scan_directory(tmp.path()),
            vec![path.clone()]
        );
        let document = LocalFilesSource::read_file(&path).unwrap();

        assert_eq!(document.content, "中文 & <資料>\n欄一\t欄二");
        assert_eq!(document.source, "local_files");
        assert_eq!(document.source_id, path.to_string_lossy());
        assert_eq!(document.title, "中文筆記.DOCX");
        assert_eq!(document.url, Some(format!("file://{}", path.display())));
        assert_eq!(document.metadata.get("extension").unwrap(), "DOCX");
        assert!(document.last_modified > 0);
    }

    #[test]
    fn corrupt_docx_is_a_source_error_with_its_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("corrupt.docx");
        std::fs::write(&path, b"not a ZIP archive").unwrap();

        let message = assert_docx_source_error(&path);
        assert!(message.contains("invalid DOCX ZIP"), "{message}");
    }

    #[test]
    fn empty_docx_is_a_source_error_with_its_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("empty.docx");
        write_docx(&path, "<w:p><w:r><w:t> </w:t></w:r></w:p>");

        let message = assert_docx_source_error(&path);
        assert!(message.contains("no extractable"), "{message}");
    }

    #[test]
    fn oversized_docx_is_excluded_from_scan_and_direct_read() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("oversized.docx");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_DOCX_SIZE + 1).unwrap();

        assert!(!LocalFilesSource::is_indexable(&path));
        assert!(LocalFilesSource::scan_directory(tmp.path()).is_empty());
        let message = assert_docx_source_error(&path);
        assert!(message.contains("10 MiB file size"), "{message}");
    }
}
