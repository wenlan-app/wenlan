// SPDX-License-Identifier: Apache-2.0
//! Artifact originals (migration 147): the files a chat import kept, and
//! the record of where each came from.
//!
//! Each file is copied from `<data root>/<file_path>` to
//! `backstage/<file_path>`, the same place under the backstage, since the
//! backstage is the data root (spec section 2). The records go to
//! `backstage/originals/artifacts.json`, ordered by page id. The page each
//! artifact has is a page file like any other.

use serde_json::json;

use super::{ExportDirs, LeftOut, Manifest};
use crate::db::MemoryDB;
use crate::error::WenlanError;

pub const RECORDS_PATH: &str = "backstage/originals/artifacts.json";

pub async fn export_artifacts(
    db: &MemoryDB,
    data_root: &std::path::Path,
    dirs: &ExportDirs,
    manifest: &mut Manifest,
) -> Result<usize, WenlanError> {
    let records = db.export_original_records().await?;
    let mut copied = 0;
    let mut listed = Vec::new();
    for record in &records {
        listed.push(json!({
            "page": record.page_id,
            "vendor": record.vendor,
            "conversation": record.conversation_id,
            "artifact": record.artifact_id,
            "kind": record.kind,
            "title": record.title,
            "file": record.file_path,
            "url": record.conversation_url,
            "written": record.written_at,
            "imported": record.imported_at,
        }));
        // Only a plain relative path under the data root is read.
        let fine = !record.file_path.is_empty()
            && !record.file_path.contains('\\')
            && record
                .file_path
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..");
        let bytes = fine
            .then(|| std::fs::read(data_root.join(&record.file_path)).ok())
            .flatten();
        match bytes {
            Some(bytes) => {
                manifest.files.push(dirs.write_bytes(
                    &format!("backstage/{}", record.file_path),
                    &record.page_id,
                    "artifact",
                    &bytes,
                )?);
                copied += 1;
            }
            None => manifest.left_out.push(LeftOut {
                id: record.page_id.clone(),
                field: "artifact file".to_string(),
                why: format!("{} is missing or unreadable", record.file_path),
            }),
        }
    }
    if !records.is_empty() {
        let text = serde_json::to_string_pretty(&listed)
            .map_err(|error| WenlanError::Validation(format!("artifacts: {error}")))?;
        manifest.files.push(dirs.write(
            RECORDS_PATH,
            "artifacts",
            "artifact_records",
            &(text + "\n"),
        )?);
    }
    Ok(copied)
}
