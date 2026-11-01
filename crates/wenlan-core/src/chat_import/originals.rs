// SPDX-License-Identifier: Apache-2.0
//! Keep the originals of an imported chat: its artifacts and canvases.
//!
//! The chat text becomes memories (see `bulk_ingest`). What the AI *made*
//! in the chat — a Claude artifact, a ChatGPT canvas — is kept as it was:
//! one file per artifact under
//! `<data root>/originals/<vendor>/<conversation id>/<artifact id>.<ext>`,
//! a `manifest.json` beside them, and one page per artifact whose
//! `creation_kind` is `original`. The page shows the original, links back to
//! the chat (its memory is the page's source, and the conversation's URL in
//! the vendor's app is on the page and in the `originals` table) and says
//! when the AI wrote it. A machine never rewrites such a page
//! (the page-update gate refuses it); later facts can only annotate it.
//!
//! Chats imported before this existed cannot be given their originals back:
//! the import kept text only and did not keep the export, so the artifacts
//! are not anywhere Wenlan can read them. Importing the same export again
//! does not help either, because a conversation that is already imported is
//! skipped whole. Only conversations imported from now on keep originals.

use std::path::{Path, PathBuf};

use crate::chat_import::types::{ParsedArtifact, ParsedConversation};
use crate::db::originals::OriginalRecord;
use crate::db::MemoryDB;
use crate::error::WenlanError;
use wenlan_types::requests::CreateConceptRequest;

/// The folder under the data root that holds every kept original.
pub const ORIGINALS_DIR: &str = "originals";

/// The page creation kind for a kept original.
pub const ORIGINAL_CREATION_KIND: &str = "original";

/// One original that was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptOriginal {
    pub artifact_id: String,
    pub page_id: String,
    /// Relative to the data root, with `/` separators.
    pub file_path: String,
}

/// A path segment made from an export id: ASCII letters, digits, `-`, `_`
/// and `.` are kept, anything else becomes `_`, and a segment that would be
/// empty or only dots becomes `_`. Export ids are UUID-like, so this is
/// normally the id itself.
pub fn safe_segment(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .take(128)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '.') {
        "_".to_string()
    } else {
        cleaned
    }
}

/// The folder for one conversation's originals, relative to the data root.
pub fn conversation_dir(conv: &ParsedConversation) -> String {
    format!(
        "{ORIGINALS_DIR}/{}/{}",
        conv.vendor.as_str(),
        safe_segment(&conv.external_id)
    )
}

/// Write `bytes` to `path` through a temporary file in the same folder, so a
/// crash never leaves a half-written original.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!(
        "{}.tmp-{}",
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// A Markdown fence longer than any backtick run in `content`.
fn fence_for(content: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for c in content.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

/// The page body for one original: where it came from, then the original
/// itself (Markdown as it is, anything else in a fenced block).
pub fn original_page_body(
    conv: &ParsedConversation,
    artifact: &ParsedArtifact,
    file: &str,
) -> String {
    let vendor = conv.vendor.display_name();
    let chat = conv
        .title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("Untitled chat");
    let url = conv.vendor.conversation_url(&conv.external_id);
    let when = artifact
        .created_at
        .or(conv.created_at)
        .map(|t| format!(", written {}", t.format("%Y-%m-%d %H:%M UTC")))
        .unwrap_or_default();
    let mut body = format!(
        "Kept from the {vendor} chat \"{chat}\" ([open in {vendor}]({url})){when}. \
         Original file: `{file}`.\n\n"
    );
    if artifact.is_markdown() {
        body.push_str(artifact.content.trim_end());
    } else {
        let fence = fence_for(&artifact.content);
        body.push_str(&fence);
        body.push_str(artifact.fence_language());
        body.push('\n');
        body.push_str(artifact.content.trim_end_matches('\n'));
        body.push('\n');
        body.push_str(&fence);
    }
    body.push('\n');
    body
}

/// A title no other note has, so the same-title rule never attaches this
/// original to someone else's page.
async fn unused_title(
    db: &MemoryDB,
    conv: &ParsedConversation,
    artifact: &ParsedArtifact,
) -> Result<String, WenlanError> {
    let base = artifact
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("Untitled artifact")
        .to_string();
    let chat = conv
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("chat");
    let mut candidates = vec![
        base.clone(),
        format!("{base} ({} {chat})", conv.vendor.display_name()),
    ];
    candidates
        .extend((2..=20).map(|n| format!("{base} ({} {chat} {n})", conv.vendor.display_name())));
    for candidate in &candidates {
        let taken = crate::page_titles::strict_same_title_note(
            db,
            &crate::read_scope::ReadScope::Global,
            None,
            candidate,
            false,
        )
        .await?;
        if taken.is_none() {
            return Ok(candidate.clone());
        }
    }
    Ok(format!("{base} ({})", safe_segment(&artifact.id)))
}

#[derive(serde::Serialize)]
struct Manifest<'a> {
    vendor: &'a str,
    conversation_id: &'a str,
    conversation_url: String,
    title: Option<&'a str>,
    artifacts: Vec<ManifestEntry>,
}

#[derive(serde::Serialize)]
struct ManifestEntry {
    id: String,
    title: Option<String>,
    kind: String,
    file: String,
    page_id: String,
    written_at: Option<i64>,
}

/// Keep every artifact of one already-imported conversation: write its
/// files and manifest under `data_root`, make one `original` page per
/// artifact citing the conversation's memory `source_id`, and record each in
/// `originals`. Artifacts this conversation already kept are skipped, so a
/// retry after a crash finishes the rest. A conversation without artifacts
/// writes nothing, not even its folder.
pub async fn keep_conversation_originals(
    db: &MemoryDB,
    data_root: &Path,
    knowledge_path: Option<&Path>,
    conv: &ParsedConversation,
) -> Result<Vec<KeptOriginal>, WenlanError> {
    if conv.artifacts.is_empty() {
        return Ok(Vec::new());
    }
    let vendor = conv.vendor.as_str();
    let already: std::collections::HashSet<String> = db
        .originals_for_conversation(vendor, &conv.external_id)
        .await?
        .into_iter()
        .map(|record| record.artifact_id)
        .collect();
    let dir_rel = conversation_dir(conv);
    let dir: PathBuf = data_root.join(&dir_rel);
    std::fs::create_dir_all(&dir)?;
    let source_id = conv.vendor.build_source_id(&conv.external_id);
    let url = conv.vendor.conversation_url(&conv.external_id);
    let mut kept = Vec::new();
    let mut used_names = std::collections::HashSet::new();
    for artifact in &conv.artifacts {
        let mut stem = safe_segment(&artifact.id);
        while !used_names.insert(stem.clone()) {
            stem.push('_');
        }
        let file_rel = format!("{dir_rel}/{stem}.{}", artifact.extension());
        if already.contains(&artifact.id) {
            continue;
        }
        write_atomically(&data_root.join(&file_rel), artifact.content.as_bytes())?;
        let title = unused_title(db, conv, artifact).await?;
        let created = crate::post_write::create_page(
            db,
            CreateConceptRequest {
                title,
                content: original_page_body(conv, artifact, &file_rel),
                summary: None,
                entity_id: None,
                space: Option::<String>::None.into(),
                source_memory_ids: vec![source_id.clone()],
                creation_kind: Some(ORIGINAL_CREATION_KIND.to_string()),
                workspace: None,
            },
            "chat_import",
            knowledge_path,
        )
        .await?;
        if !created.wrote {
            return Err(WenlanError::Validation(format!(
                "original {} of {vendor} conversation {} did not get its own page: {}",
                artifact.id,
                conv.external_id,
                created.warnings.join("; ")
            )));
        }
        db.insert_original(&OriginalRecord {
            page_id: created.id.clone(),
            vendor: vendor.to_string(),
            conversation_id: conv.external_id.clone(),
            artifact_id: artifact.id.clone(),
            kind: artifact.kind.clone(),
            title: artifact.title.clone(),
            file_path: file_rel.clone(),
            conversation_url: url.clone(),
            written_at: artifact.created_at.map(|t| t.timestamp()),
            imported_at: chrono::Utc::now().timestamp(),
        })
        .await?;
        kept.push(KeptOriginal {
            artifact_id: artifact.id.clone(),
            page_id: created.id,
            file_path: file_rel,
        });
    }
    // The manifest lists everything this conversation keeps, from the
    // table, so a retry that kept only the rest still lists all of it.
    let manifest = Manifest {
        vendor,
        conversation_id: &conv.external_id,
        conversation_url: url,
        title: conv.title.as_deref(),
        artifacts: db
            .originals_for_conversation(vendor, &conv.external_id)
            .await?
            .into_iter()
            .map(|record| ManifestEntry {
                id: record.artifact_id,
                title: record.title,
                kind: record.kind,
                file: record.file_path,
                page_id: record.page_id,
                written_at: record.written_at,
            })
            .collect(),
    };
    write_atomically(
        &dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?.as_slice(),
    )?;
    Ok(kept)
}

/// Where `page_id`'s original came from, or `None` for a page that was not
/// made from a kept original.
pub async fn page_original(
    db: &MemoryDB,
    page_id: &str,
) -> Result<Option<wenlan_types::originals::PageOriginalResponse>, WenlanError> {
    Ok(db.get_original(page_id).await?.map(|record| {
        wenlan_types::originals::PageOriginalResponse {
            page_id: record.page_id,
            vendor: record.vendor,
            conversation_id: record.conversation_id,
            conversation_url: record.conversation_url,
            artifact_id: record.artifact_id,
            kind: record.kind,
            title: record.title,
            file_path: record.file_path,
            written_at: record.written_at,
            imported_at: record.imported_at,
        }
    }))
}

#[cfg(test)]
#[path = "originals_test.rs"]
mod tests;
