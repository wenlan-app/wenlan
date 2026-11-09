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
    let chat = match conv
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Some(title) => format!("the {vendor} chat \"{title}\""),
        None => format!("a {vendor} chat"),
    };
    let url = conv.vendor.conversation_url(&conv.external_id);
    let when = artifact
        .created_at
        .or(conv.created_at)
        .map(|t| format!(", written {}", t.format("%Y-%m-%d %H:%M UTC")))
        .unwrap_or_default();
    let mut body = format!(
        "Kept from {chat} ([open in {vendor}]({url})){when}. \
         Original file: `{file}`.\n\n"
    );
    push_original(
        &mut body,
        &artifact.content,
        artifact.is_markdown(),
        artifact.fence_language(),
    );
    body
}

/// The original itself: Markdown as it is, anything else in a fenced block
/// the app shows as the page it was (HTML and SVG) or as code.
fn push_original(body: &mut String, content: &str, markdown: bool, language: &str) {
    if markdown {
        body.push_str(content.trim_end());
    } else {
        let fence = fence_for(content);
        body.push_str(&fence);
        body.push_str(language);
        body.push('\n');
        body.push_str(content.trim_end_matches('\n'));
        body.push('\n');
        body.push_str(&fence);
    }
    body.push('\n');
}

/// A title no other note has, so the same-title rule never attaches this
/// original to someone else's page.
async fn unused_title(
    db: &MemoryDB,
    conv: &ParsedConversation,
    artifact: &ParsedArtifact,
) -> Result<String, WenlanError> {
    let vendor = conv.vendor.display_name();
    let chat = conv
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let base = artifact
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .or_else(|| chat.map(str::to_string))
        .unwrap_or_else(|| format!("{vendor} artifact"));
    let context = format!("{vendor} {}", chat.unwrap_or("chat"));
    if let Some(title) = first_unused_title(db, &base, &context).await? {
        return Ok(title);
    }
    Ok(format!("{base} ({})", safe_segment(&artifact.id)))
}

/// `base`, else `base (<context>)`, else `base (<context> 2)` up to 20, or
/// `None` when every one is taken.
async fn first_unused_title(
    db: &MemoryDB,
    base: &str,
    context: &str,
) -> Result<Option<String>, WenlanError> {
    let mut candidates = vec![base.to_string(), format!("{base} ({context})")];
    candidates.extend((2..=20).map(|n| format!("{base} ({context} {n})")));
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
            return Ok(Some(candidate.clone()));
        }
    }
    Ok(None)
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
            replaces: None,
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

/// What an AI tool made in a live session that it asks Wenlan to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOriginalFormat {
    Html,
    Svg,
    Markdown,
}

impl SessionOriginalFormat {
    /// The format for a file name or a kind the caller named.
    pub fn from_name(name: &str) -> Option<Self> {
        let lower = name.trim().to_ascii_lowercase();
        let ext = lower.rsplit('.').next().unwrap_or(&lower);
        match ext {
            "html" | "htm" | "text/html" => Some(Self::Html),
            "svg" | "image/svg+xml" => Some(Self::Svg),
            "md" | "markdown" | "text/markdown" => Some(Self::Markdown),
            _ => None,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Svg => "svg",
            Self::Markdown => "md",
        }
    }

    fn kind(self) -> &'static str {
        match self {
            Self::Html => "text/html",
            Self::Svg => "image/svg+xml",
            Self::Markdown => "text/markdown",
        }
    }
}

/// One thing an AI tool made in a live session, to keep as an original.
#[derive(Debug, Clone)]
pub struct SessionOriginal<'a> {
    /// The session that made it; `None` when the caller named none.
    pub session: Option<&'a wenlan_types::sessions::SessionRef>,
    pub title: &'a str,
    pub content: &'a str,
    pub format: SessionOriginalFormat,
    /// The Space the page lands in, already resolved by the caller.
    pub space: wenlan_types::WriteSpaceTarget,
    /// The resolved Space name, for the page's workspace axis.
    pub workspace: Option<String>,
    /// Memories the page cites: the diagram's structure as text, kept in the
    /// session so the night's tidy and later sessions read it, not the HTML.
    pub source_memory_ids: Vec<String>,
    /// The older version this one replaces: its page id, or its title as it
    /// was kept. A version that was itself replaced since resolves to the
    /// newest one, so versions stay one line.
    pub replaces: Option<&'a str>,
}

/// The newest version of the original `named` (a page id or a kept title),
/// following later versions forward. `original_replaces_unknown` when no
/// kept original matches.
async fn newest_version_of(db: &MemoryDB, named: &str) -> Result<OriginalRecord, WenlanError> {
    let found = match db.get_original(named).await? {
        Some(record) if db.get_page(&record.page_id).await?.is_some() => Some(record),
        _ => db.latest_original_titled(named).await?,
    };
    let Some(mut record) = found else {
        return Err(WenlanError::Validation("original_replaces_unknown".into()));
    };
    // A chain is short; the bound only stops a corrupted cycle.
    for _ in 0..64 {
        match db
            .originals_replacing(&record.page_id)
            .await?
            .into_iter()
            .next()
        {
            Some(newer) => record = newer,
            None => break,
        }
    }
    Ok(record)
}

/// The page a new version replaces: the newest version of the original
/// `named` (a page id or a kept title). `original_replaces_unknown` when no
/// kept original matches.
pub async fn original_to_replace(db: &MemoryDB, named: &str) -> Result<String, WenlanError> {
    Ok(newest_version_of(db, named.trim()).await?.page_id)
}

/// One version of a kept original: its page, title and when it was kept.
async fn version_ref(
    db: &MemoryDB,
    record: &OriginalRecord,
) -> Result<Option<wenlan_types::originals::OriginalVersion>, WenlanError> {
    Ok(db
        .get_page(&record.page_id)
        .await?
        .map(|page| wenlan_types::originals::OriginalVersion {
            page_id: record.page_id.clone(),
            title: page.title,
            kept_at: record.imported_at,
        }))
}

/// The id a session original's content gets: a prefix of its SHA-256.
fn content_id(content: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(content.as_bytes()))[..16].to_string()
}

/// The page this session already kept for exactly this content, if any.
pub async fn find_session_original(
    db: &MemoryDB,
    session: Option<&wenlan_types::sessions::SessionRef>,
    content: &str,
) -> Result<Option<KeptOriginal>, WenlanError> {
    let (vendor, conversation_id) = session_key(session);
    let artifact_id = content_id(content);
    let Some(record) = db
        .originals_for_conversation(&vendor, &conversation_id)
        .await?
        .into_iter()
        .find(|record| record.artifact_id == artifact_id)
    else {
        return Ok(None);
    };
    // Only a page that still exists counts as kept.
    if db.get_page(&record.page_id).await?.is_none() {
        return Ok(None);
    }
    Ok(Some(KeptOriginal {
        artifact_id,
        page_id: record.page_id,
        file_path: record.file_path,
    }))
}

fn session_key(session: Option<&wenlan_types::sessions::SessionRef>) -> (String, String) {
    match session {
        Some(session) => (session.tool.clone(), session.external_id.clone()),
        None => ("agent".to_string(), "no-session".to_string()),
    }
}

/// The largest original a session may keep (a self-contained HTML page with
/// inline data is rarely more than a few MB).
pub const MAX_SESSION_ORIGINAL_BYTES: usize = 10 * 1024 * 1024;

/// How a session's tool is named in a page's first line.
fn session_tool_name(tool: &str) -> String {
    match tool {
        "claude-code" => "Claude Code".to_string(),
        "codex" => "Codex".to_string(),
        "claude" => "Claude".to_string(),
        "chatgpt" => "ChatGPT".to_string(),
        other => other.to_string(),
    }
}

/// Keep what an AI tool made in a live session (UX-ART-1..3): the file as it
/// is under `originals/<tool>/<session>/`, one `original` page that shows it,
/// and a row in `originals`. The page names the session and when it was
/// kept; the session itself is stamped on the page's history by the caller.
/// Keeping the same content again in the same session returns the page it
/// already has, so a retry never makes a second copy. A changed version is
/// new content and gets its own page; the first one stays as it was.
pub async fn keep_session_original(
    db: &MemoryDB,
    data_root: &Path,
    knowledge_path: Option<&Path>,
    input: SessionOriginal<'_>,
) -> Result<(KeptOriginal, bool), WenlanError> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err(WenlanError::Validation("original_title_missing".into()));
    }
    if input.content.trim().is_empty() {
        return Err(WenlanError::Validation("original_empty".into()));
    }
    if input.content.len() > MAX_SESSION_ORIGINAL_BYTES {
        return Err(WenlanError::Validation("original_too_large".into()));
    }
    let (vendor, conversation_id) = session_key(input.session);
    if let Some(kept) = find_session_original(db, input.session, input.content).await? {
        return Ok((kept, false));
    }
    let replaced = match input
        .replaces
        .map(str::trim)
        .filter(|named| !named.is_empty())
    {
        Some(named) => {
            let record = newest_version_of(db, named).await?;
            let title = db
                .get_page(&record.page_id)
                .await?
                .map(|page| page.title)
                .unwrap_or_default();
            Some((record.page_id, title))
        }
        None => None,
    };
    let mut artifact_id = content_id(input.content);
    // A row whose page was destroyed keeps its id, so the new copy takes another.
    if db
        .originals_for_conversation(&vendor, &conversation_id)
        .await?
        .iter()
        .any(|record| record.artifact_id == artifact_id)
    {
        artifact_id = format!("{artifact_id}-{}", uuid::Uuid::new_v4().simple());
    }
    let dir_rel = format!(
        "{ORIGINALS_DIR}/{}/{}",
        safe_segment(&vendor),
        safe_segment(&conversation_id)
    );
    std::fs::create_dir_all(data_root.join(&dir_rel))?;
    let file_rel = format!("{dir_rel}/{artifact_id}.{}", input.format.extension());
    write_atomically(&data_root.join(&file_rel), input.content.as_bytes())?;

    let now = chrono::Utc::now();
    let tool = session_tool_name(&vendor);
    let from = if input.session.is_some() {
        format!("Saved from a {tool} session")
    } else {
        "Saved by an AI tool".to_string()
    };
    let newer_than = replaced
        .as_ref()
        .map(|(_, title)| format!(" New version of \u{201c}{title}\u{201d}."))
        .unwrap_or_default();
    let mut body = format!(
        "{from}, {}.{newer_than} Original file: `{file_rel}`.\n\n",
        now.format("%Y-%m-%d %H:%M UTC")
    );
    let language = match input.format {
        SessionOriginalFormat::Html => "html",
        SessionOriginalFormat::Svg => "svg",
        SessionOriginalFormat::Markdown => "markdown",
    };
    push_original(
        &mut body,
        input.content,
        input.format == SessionOriginalFormat::Markdown,
        language,
    );
    let page_title = first_unused_title(db, title, &format!("{tool} {}", now.format("%Y-%m-%d")))
        .await?
        .unwrap_or_else(|| format!("{title} ({artifact_id})"));
    let created = crate::post_write::create_page(
        db,
        CreateConceptRequest {
            title: page_title,
            content: body,
            summary: None,
            entity_id: None,
            space: input.space,
            source_memory_ids: input.source_memory_ids,
            creation_kind: Some(ORIGINAL_CREATION_KIND.to_string()),
            workspace: input.workspace,
        },
        "agent",
        knowledge_path,
    )
    .await?;
    if !created.wrote {
        return Err(WenlanError::Validation(format!(
            "original from {vendor} session {conversation_id} did not get its own page: {}",
            created.warnings.join("; ")
        )));
    }
    db.insert_original(&OriginalRecord {
        page_id: created.id.clone(),
        vendor,
        conversation_id,
        artifact_id: artifact_id.clone(),
        kind: input.format.kind().to_string(),
        title: Some(title.to_string()),
        file_path: file_rel.clone(),
        conversation_url: String::new(),
        written_at: Some(now.timestamp()),
        imported_at: now.timestamp(),
        replaces: replaced.map(|(page_id, _)| page_id),
    })
    .await?;
    Ok((
        KeptOriginal {
            artifact_id,
            page_id: created.id,
            file_path: file_rel,
        },
        true,
    ))
}

/// Where `page_id`'s original came from, or `None` for a page that was not
/// made from a kept original.
pub async fn page_original(
    db: &MemoryDB,
    page_id: &str,
) -> Result<Option<wenlan_types::originals::PageOriginalResponse>, WenlanError> {
    let Some(record) = db.get_original(page_id).await? else {
        return Ok(None);
    };
    // The newest version, when a later one replaced this.
    let mut newer = None;
    let mut at = record.page_id.clone();
    for _ in 0..64 {
        match db.originals_replacing(&at).await?.into_iter().next() {
            Some(next) => {
                at = next.page_id.clone();
                newer = Some(next);
            }
            None => break,
        }
    }
    let newer = match newer {
        Some(next) => version_ref(db, &next).await?,
        None => None,
    };
    // Every older version, newest first.
    let mut older = Vec::new();
    let mut back = record.replaces.clone();
    while let Some(id) = back.take() {
        if older.len() >= 64 || id == record.page_id {
            break;
        }
        let Some(prior) = db.get_original(&id).await? else {
            break;
        };
        if let Some(version) = version_ref(db, &prior).await? {
            older.push(version);
        }
        back = prior.replaces.clone();
    }
    Ok(Some(wenlan_types::originals::PageOriginalResponse {
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
        newer,
        older,
    }))
}

#[cfg(test)]
#[path = "originals_test.rs"]
mod tests;
