// SPDX-License-Identifier: Apache-2.0
//! Manual-only synchronization for local Zotero collections.

use super::{SkippedFile, SyncStatsResponse, SKIPPED_FILES_LIMIT};
use crate::{
    error::ServerError,
    state::{ServerState, SharedState},
};
use anyhow::Context;
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Json},
    Extension,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::RwLock;
use wenlan_core::{
    config,
    sources::{
        zotero::{
            self, FulltextStatus, ZoteroClient, ZoteroCollection, ZoteroFulltext, ZoteroItem,
        },
        Source,
    },
};
use wenlan_types::{
    zotero::{
        AddZoteroSourceRequest, ZoteroAttachmentResponse, ZoteroCollectionSummary,
        ZoteroCollectionsResponse, ZoteroDocumentResponse,
    },
    RawDocument, SourceType, SyncStatus, WriteSpaceTarget,
};

const LOCATOR_PREFIX: &str = "zotero-local";

trait ZoteroApi {
    fn server_id(&self) -> Option<&str>;
    fn list_collections(
        &mut self,
    ) -> impl Future<Output = anyhow::Result<zotero::ZoteroPageSet<ZoteroCollection>>> + Send;
    fn collection_items(
        &mut self,
        key: &str,
    ) -> impl Future<Output = anyhow::Result<zotero::ZoteroPageSet<ZoteroItem>>> + Send;
    fn children(
        &mut self,
        key: &str,
    ) -> impl Future<Output = anyhow::Result<zotero::ZoteroPageSet<ZoteroItem>>> + Send;
    fn fulltext(
        &mut self,
        key: &str,
    ) -> impl Future<Output = anyhow::Result<ZoteroFulltext>> + Send;
    fn pdf_bytes(&mut self, key: &str) -> impl Future<Output = anyhow::Result<Vec<u8>>> + Send;
}

impl ZoteroApi for ZoteroClient {
    fn server_id(&self) -> Option<&str> {
        ZoteroClient::server_id(self)
    }
    async fn list_collections(
        &mut self,
    ) -> anyhow::Result<zotero::ZoteroPageSet<ZoteroCollection>> {
        ZoteroClient::list_collections(self).await
    }
    async fn collection_items(
        &mut self,
        key: &str,
    ) -> anyhow::Result<zotero::ZoteroPageSet<ZoteroItem>> {
        ZoteroClient::collection_items(self, key).await
    }
    async fn children(&mut self, key: &str) -> anyhow::Result<zotero::ZoteroPageSet<ZoteroItem>> {
        ZoteroClient::children(self, key).await
    }
    async fn fulltext(&mut self, key: &str) -> anyhow::Result<ZoteroFulltext> {
        ZoteroClient::fulltext(self, key).await
    }
    async fn pdf_bytes(&mut self, key: &str) -> anyhow::Result<Vec<u8>> {
        ZoteroClient::pdf_bytes(self, key).await
    }
}

pub async fn handle_list_collections(
    client: Option<Extension<ZoteroClient>>,
) -> Result<Json<ZoteroCollectionsResponse>, ServerError> {
    // Only isolated test routers install this extension. Production uses the
    // fixed local Zotero endpoint, with no user-configurable URL.
    let mut client = match client {
        Some(Extension(client)) => client,
        None => ZoteroClient::new().map_err(|e| ServerError::Upstream(e.to_string()))?,
    };
    let page = client
        .list_collections()
        .await
        .map_err(|e| ServerError::Upstream(e.to_string()))?;
    if !page.complete {
        return Err(ServerError::Upstream(
            "Zotero collection listing was incomplete".into(),
        ));
    }
    let mut collections = Vec::with_capacity(page.items.len());
    for collection in page.items {
        collections.push(summary(collection)?);
    }
    Ok(Json(ZoteroCollectionsResponse {
        server_id: client.server_id().map(str::to_owned),
        collections,
    }))
}

pub async fn handle_add_source(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(body): Json<AddZoteroSourceRequest>,
) -> Result<Json<Source>, ServerError> {
    validate_collection_key(&body.collection_key)?;
    let requested_space = body.space.trim();
    if requested_space.is_empty() {
        return Err(ServerError::ValidationError(
            "Choose an explicit destination Space for the Zotero collection".into(),
        ));
    }

    let mut client = ZoteroClient::with_server_id(body.expected_server_id.clone())
        .map_err(|e| ServerError::Upstream(e.to_string()))?;
    let page = client
        .list_collections()
        .await
        .map_err(|e| ServerError::Upstream(e.to_string()))?;
    if !page.complete {
        return Err(ServerError::Upstream(
            "Zotero collection listing was incomplete".into(),
        ));
    }
    let collection = page
        .items
        .into_iter()
        .find(|collection| collection.key == body.collection_key)
        .ok_or_else(|| {
            ServerError::ValidationError(format!(
                "Zotero collection {} is no longer available",
                body.collection_key
            ))
        })?;
    let server_id = client.server_id().map(str::to_owned);
    if server_id.is_none() {
        return Err(ServerError::ValidationError(
            "This Zotero server does not expose a stable Zotero-Server-ID; update Zotero before connecting it".into(),
        ));
    }

    let db = {
        let guard = state.read().await;
        guard.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let resolved = db
        .resolve_write_space(&WriteSpaceTarget::Named(requested_space.to_owned()), None)
        .await?;
    let space_name = resolved.space_name.clone().ok_or_else(|| {
        ServerError::ValidationError("Choose a registered destination Space".into())
    })?;

    let locator = locator(&server_id, &body.collection_key);
    let snapshot = config::load_config();
    if let Some(existing) = snapshot.sources.iter().find(|source| {
        source.source_type == SourceType::Zotero
            && source.path == std::path::Path::new(&locator)
            && source.space.as_deref() == Some(space_name.as_str())
    }) {
        return Ok(Json(existing.clone()));
    }

    let mut hasher = Sha256::new();
    hasher.update(space_name.as_bytes());
    let space_hash = format!("{:x}", hasher.finalize());
    let base_id = format!(
        "zotero-{}-{}",
        wenlan_core::export::obsidian::slugify(&collection.data.name),
        &space_hash[..10]
    );
    let source = config::update_config(|config| {
        if let Some(existing) = config.sources.iter().find(|source| {
            source.source_type == SourceType::Zotero
                && source.path == std::path::Path::new(&locator)
                && source.space.as_deref() == Some(space_name.as_str())
        }) {
            return Ok(existing.clone());
        }
        let mut id = base_id.clone();
        let mut suffix = 2;
        while config.sources.iter().any(|source| source.id == id) {
            id = format!("{base_id}-{suffix}");
            suffix += 1;
        }
        let source = Source {
            id,
            source_type: SourceType::Zotero,
            path: PathBuf::from(&locator),
            status: SyncStatus::Paused,
            last_sync: None,
            file_count: 0,
            memory_count: 0,
            last_sync_errors: 0,
            last_sync_error_detail: None,
            space: Some(space_name.clone()),
            queued_files: 0,
            waiting_files: 0,
        };
        config.sources.push(source.clone());
        Ok(source)
    })?;
    Ok(Json(source))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ZoteroDocumentQuery {
    source_id: String,
    space: Option<String>,
}

pub async fn handle_get_document(
    State(state): State<SharedState>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Query(query): Query<ZoteroDocumentQuery>,
) -> Result<(HeaderMap, Json<ZoteroDocumentResponse>), ServerError> {
    let (sidecar, selected_space) =
        authorized_document(&state, &query, header_space.as_deref()).await?;
    let current = sidecar.current.ok_or_else(|| {
        ServerError::NotFound(format!("Zotero document not found: {}", query.source_id))
    })?;
    if current.source_id != query.source_id {
        return Err(ServerError::NotFound(format!(
            "Zotero document not found: {}",
            query.source_id
        )));
    }
    let attachments = current
        .attachments
        .iter()
        .map(|attachment| ZoteroAttachmentResponse {
            key: attachment.key.clone(),
            title: attachment.title.clone(),
            available: attachment
                .cached_pdf_hash
                .as_deref()
                .is_some_and(cached_pdf_is_valid),
        })
        .collect();
    Ok((
        source_scope_headers(&selected_space)?,
        Json(ZoteroDocumentResponse {
            source_id: current.source_id,
            title: current.title,
            attachments,
            bibtex: current.bibtex,
            warnings: current.warnings,
        }),
    ))
}

pub async fn handle_get_pdf(
    State(state): State<SharedState>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Query(query): Query<PdfQuery>,
) -> Result<impl IntoResponse, ServerError> {
    let document_query = ZoteroDocumentQuery {
        source_id: query.source_id.clone(),
        space: query.space.clone(),
    };
    let (sidecar, selected_space) =
        authorized_document(&state, &document_query, header_space.as_deref()).await?;
    let current = sidecar.current.ok_or_else(|| {
        ServerError::NotFound(format!("Zotero document not found: {}", query.source_id))
    })?;
    if current.source_id != query.source_id {
        return Err(ServerError::NotFound("Zotero document not found".into()));
    }
    let attachment = current
        .attachments
        .iter()
        .find(|attachment| attachment.key == query.attachment_key)
        .ok_or_else(|| ServerError::NotFound("Zotero attachment not found".into()))?;
    let hash = attachment
        .cached_pdf_hash
        .as_deref()
        .ok_or_else(|| ServerError::NotFound("Zotero PDF is not cached".into()))?;
    let bytes = read_cached_pdf(hash)
        .await?
        .ok_or_else(|| ServerError::NotFound("Zotero PDF is not cached".into()))?;
    let filename = HeaderValue::from_str(&format!("inline; filename=\"{}.pdf\"", attachment.key))
        .map_err(|error| ServerError::Internal(error.to_string()))?;
    let mut headers = source_scope_headers(&selected_space)?;
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/pdf"),
    );
    headers.insert(header::CONTENT_DISPOSITION, filename);
    Ok((headers, bytes))
}

#[derive(Debug, Deserialize)]
pub(crate) struct PdfQuery {
    source_id: String,
    attachment_key: String,
    space: Option<String>,
}

async fn authorized_document(
    state: &SharedState,
    query: &ZoteroDocumentQuery,
    header_space: Option<&str>,
) -> Result<(ZoteroSidecarHistory, String), ServerError> {
    let query_space = query
        .space
        .as_deref()
        .map(str::trim)
        .filter(|space| !space.is_empty());
    let header_space = header_space
        .map(str::trim)
        .filter(|space| !space.is_empty());
    if query_space
        .zip(header_space)
        .is_some_and(|(query, header)| query != header)
    {
        return Err(ServerError::BadRequest(
            "Space query conflicts with Space header".into(),
        ));
    }
    let raw_space = query_space
        .or(header_space)
        .ok_or_else(|| ServerError::BadRequest("A Zotero document Space is required".into()))?;
    let db = {
        let guard = state.read().await;
        guard.db.clone().ok_or(ServerError::DbNotInitialized)?
    };
    let scope = wenlan_core::read_scope::resolve_read_scope(&db, Some(raw_space))
        .await
        .map_err(|error| ServerError::ValidationError(error.to_string()))?;
    if !db
        .has_exact_source_identity_scoped(&query.source_id, "file", "zotero", &scope)
        .await?
    {
        return Err(ServerError::NotFound("Zotero document not found".into()));
    }
    let path = sidecar_path(&query.source_id);
    let bytes = tokio::fs::read(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ServerError::NotFound("Zotero document not found".into())
        } else {
            ServerError::Internal(format!("read Zotero metadata: {error}"))
        }
    })?;
    let sidecar: ZoteroSidecarHistory = serde_json::from_slice(&bytes)
        .map_err(|error| ServerError::Internal(format!("parse Zotero metadata: {error}")))?;
    let Some(current) = &sidecar.current else {
        return Err(ServerError::NotFound("Zotero document not found".into()));
    };
    if current.source_id != query.source_id
        || !scope_matches_space(&scope, current.space.as_deref())
    {
        return Err(ServerError::NotFound("Zotero document not found".into()));
    }
    Ok((sidecar, raw_space.to_owned()))
}

fn source_scope_headers(space: &str) -> Result<HeaderMap, ServerError> {
    let encoded: String = space
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect();
    let value = HeaderValue::from_str(&encoded)
        .map_err(|error| ServerError::Internal(format!("encode Zotero Space scope: {error}")))?;
    let mut headers = HeaderMap::new();
    headers.insert("x-wenlan-source-scope", value);
    Ok(headers)
}

fn scope_matches_space(scope: &wenlan_core::read_scope::ReadScope, space: Option<&str>) -> bool {
    use wenlan_core::read_scope::ReadScope;
    match scope {
        ReadScope::Global => true,
        ReadScope::Space(name) => space == Some(name.as_str()),
        ReadScope::Uncategorized => space.is_none(),
    }
}

fn cached_pdf_is_valid(hash: &str) -> bool {
    let Ok(path) = pdf_cache_path(hash) else {
        return false;
    };
    let Ok(metadata) = std::fs::symlink_metadata(&path) else {
        return false;
    };
    metadata.file_type().is_file() && metadata.len() >= 5
}

async fn read_cached_pdf(hash: &str) -> Result<Option<Vec<u8>>, ServerError> {
    let path = pdf_cache_path(hash)?;
    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ServerError::Internal(format!(
                "inspect Zotero PDF: {error}"
            )));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(ServerError::Internal(
            "cached Zotero PDF is not a regular file".into(),
        ));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|error| ServerError::Internal(format!("read Zotero PDF: {error}")))?;
    if sha256_hex(&bytes) != hash || !bytes.starts_with(b"%PDF-") {
        return Ok(None);
    }
    Ok(Some(bytes))
}

pub(crate) async fn sync_source(
    db: Arc<wenlan_core::db::MemoryDB>,
    source: &Source,
) -> Result<SyncStatsResponse, ServerError> {
    let (expected_server_id, _) = parse_locator(&source.path)?;
    let mut client = ZoteroClient::with_server_id(expected_server_id)
        .map_err(|e| ServerError::Upstream(e.to_string()))?;
    sync_source_with_client(db, source, &mut client).await
}

async fn sync_source_with_client<C: ZoteroApi>(
    db: Arc<wenlan_core::db::MemoryDB>,
    source: &Source,
    client: &mut C,
) -> Result<SyncStatsResponse, ServerError> {
    static SYNC_LANE: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _sync_guard = SYNC_LANE
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let (expected_server_id, collection_key) = parse_locator(&source.path)?;
    if expected_server_id
        .as_deref()
        .is_some_and(|expected| client.server_id() != Some(expected))
    {
        return Err(ServerError::Upstream(
            "Zotero server identity does not match the registered connection".into(),
        ));
    }
    let space_name = source.space.as_deref().ok_or_else(|| {
        ServerError::ValidationError("Zotero source has no destination Space".into())
    })?;
    let write_space = db
        .resolve_write_space(&WriteSpaceTarget::Named(space_name.to_owned()), None)
        .await?;
    let collection_page = client
        .list_collections()
        .await
        .map_err(|e| ServerError::Upstream(e.to_string()))?;
    if client.server_id().is_none() {
        return Err(ServerError::Upstream(
            "Zotero did not provide a stable Zotero-Server-ID; this library cannot be synchronized safely".into(),
        ));
    }
    if !collection_page.complete
        || !collection_page
            .items
            .iter()
            .any(|collection| collection.key == collection_key)
    {
        return Err(ServerError::Upstream(format!(
            "Zotero collection {collection_key} is unavailable or the listing was incomplete"
        )));
    }
    let item_page = client
        .collection_items(&collection_key)
        .await
        .map_err(|e| ServerError::Upstream(e.to_string()))?;
    if !item_page.complete {
        return Err(ServerError::Upstream(
            "Zotero item listing was incomplete; no documents were changed".into(),
        ));
    }

    let mut documents = Vec::new();
    let mut sync_receipts = Vec::new();
    let mut sidecars = Vec::new();
    let mut skipped_files = Vec::new();
    let mut errors = 0usize;
    let mut seen = HashSet::new();
    let write_scope = wenlan_core::read_scope::ReadScope::Space(
        write_space
            .space_name
            .clone()
            .unwrap_or_else(|| space_name.to_owned()),
    );
    // `memory_count` reports the currently indexed unique items visible from
    // this collection in its destination Space. Two collection sources can
    // share the same canonical item ID without double-counting that item here.
    let mut indexed_source_ids = HashSet::new();
    for item in item_page
        .items
        .iter()
        .filter(|item| item.data.item_type != "attachment" || item.data.parent_item.is_none())
    {
        let source_id = canonical_source_id(
            client.server_id(),
            &item.key,
            write_space.space_id.as_deref(),
        );
        if db
            .get_chunks_scoped(&source_id, &write_scope)
            .await?
            .is_some_and(|chunks| !chunks.is_empty())
        {
            indexed_source_ids.insert(source_id);
        }
    }
    for item in &item_page.items {
        if item.data.item_type == "attachment" && item.data.parent_item.is_some() {
            continue;
        }
        match prepare_item(client, item).await {
            Ok(prepared) => {
                let transient_fulltext_error = prepared.transient_fulltext_error;
                let mut attachment_metadata = prepared.attachment_metadata;
                let prepared_attachments = prepared.attachments;
                let prepared = prepared.document;
                if !seen.insert(prepared.key.clone()) {
                    continue;
                }
                let source_id = canonical_source_id(
                    client.server_id(),
                    &prepared.key,
                    write_space.space_id.as_deref(),
                );
                let existing_document = indexed_source_ids.contains(&source_id);
                if transient_fulltext_error && existing_document {
                    errors += 1;
                    add_skipped(
                        &mut skipped_files,
                        &prepared.title,
                        &prepared.key,
                        "Zotero full-text retrieval failed; kept the prior document snapshot",
                    );
                    continue;
                }
                let previous = match read_current_sidecar(&source_id) {
                    Ok(previous) => previous,
                    Err(error) => {
                        errors += 1;
                        add_skipped(
                            &mut skipped_files,
                            &prepared.title,
                            &prepared.key,
                            &error.to_string(),
                        );
                        continue;
                    }
                };
                let mut research = research_metadata(
                    &source_id,
                    &prepared,
                    attachment_metadata.clone(),
                    client.server_id(),
                    write_space.space_name.as_deref(),
                    item,
                );
                let mut cached_pdfs = Vec::new();
                for attachment in prepared_attachments {
                    let cached_pdf_hash = attachment.pdf_bytes.as_deref().map(sha256_hex);
                    research.attachments.push(ZoteroAttachmentSidecar {
                        key: attachment.key.clone(),
                        title: attachment.title,
                        content_type: attachment.content_type,
                        cached_pdf_hash: cached_pdf_hash.clone(),
                    });
                    if let (Some(hash), Some(bytes)) = (cached_pdf_hash, attachment.pdf_bytes) {
                        cached_pdfs.push((hash, bytes));
                    }
                }
                if let Some(previous) = previous {
                    for attachment in &mut research.attachments {
                        if attachment.cached_pdf_hash.is_none() {
                            attachment.cached_pdf_hash = previous
                                .attachments
                                .iter()
                                .find(|old| old.key == attachment.key)
                                .and_then(|old| old.cached_pdf_hash.clone());
                        }
                    }
                }
                let retained_hashes = research
                    .attachments
                    .iter()
                    .filter_map(|attachment| attachment.cached_pdf_hash.as_deref())
                    .collect::<Vec<_>>();
                if !retained_hashes.is_empty() {
                    let hashes = retained_hashes.join(" | ");
                    attachment_metadata.insert("zotero_cached_pdf_hashes".into(), hashes);
                }
                let hash = prepared_hash(&prepared, &attachment_metadata, item);
                match db.get_sync_state(&source.id, &prepared.key).await {
                    Ok(Some(old)) if old.content_hash == hash && existing_document => {
                        sidecars.push((source_id, research, cached_pdfs));
                        continue;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        errors += 1;
                        add_skipped(
                            &mut skipped_files,
                            &prepared.title,
                            &prepared.key,
                            &error.to_string(),
                        );
                        continue;
                    }
                }
                sidecars.push((source_id.clone(), research, cached_pdfs));
                let mut metadata = prepared.metadata;
                metadata.insert("zotero_item_key".into(), prepared.key.clone());
                for (key, value) in &item.data.extra {
                    if let Some(value) = value.as_str() {
                        metadata.insert(format!("zotero_{key}"), value.to_owned());
                    } else if !value.is_null() {
                        metadata.insert(format!("zotero_{key}"), value.to_string());
                    }
                }
                if !prepared.warnings.is_empty() {
                    metadata.insert("zotero_warnings".into(), prepared.warnings.join("; "));
                }
                for (key, value) in attachment_metadata {
                    metadata.insert(key, value);
                }
                sync_receipts.push((prepared.key.clone(), hash.clone()));
                documents.push((
                    RawDocument {
                        source: "file".into(),
                        source_id,
                        title: prepared.title,
                        content: prepared.content,
                        url: prepared.url,
                        last_modified: chrono::Utc::now().timestamp(),
                        metadata,
                        space: write_space.space_name.clone(),
                        source_agent: Some("zotero".into()),
                        content_hash: Some(hash),
                        ..RawDocument::default()
                    },
                    Some(write_space.clone()),
                ));
            }
            Err(error) => {
                errors += 1;
                add_skipped(
                    &mut skipped_files,
                    &item.data.title,
                    &item.key,
                    &error.to_string(),
                );
            }
        }
    }
    let files_found = item_page
        .items
        .iter()
        .filter(|item| item.data.item_type != "attachment" || item.data.parent_item.is_none())
        .count();
    let changed_count = documents.len();
    let changed_source_ids = documents
        .iter()
        .map(|(document, _)| document.source_id.clone())
        .collect::<Vec<_>>();
    if changed_count > 0 {
        db.upsert_documents_with_write_spaces(documents).await?;
        indexed_source_ids.extend(changed_source_ids);
    }
    let ingested = changed_count;
    // Record only successfully persisted documents. An interrupted or failed
    // fetch never marks an item as complete, so the next manual sync retries.
    for (source_id, sidecar, cached_pdfs) in sidecars {
        let item_key = sidecar.item_key.clone();
        let item_title = sidecar.title.clone();
        if let Err(error) = write_research_metadata(&source_id, sidecar, cached_pdfs) {
            errors += 1;
            add_skipped(
                &mut skipped_files,
                &item_title,
                &item_key,
                &error.to_string(),
            );
        }
    }
    for (key, hash) in sync_receipts {
        db.upsert_sync_state(&source.id, &key, 0, &hash).await?;
    }

    let skipped = files_found.saturating_sub(ingested);
    let detail = (errors > 0).then(|| "zotero_item_errors".to_string());
    let _ = config::update_config(|config| {
        if let Some(stored) = config
            .sources
            .iter_mut()
            .find(|stored| stored.id == source.id)
        {
            stored.last_sync = Some(chrono::Utc::now().timestamp());
            stored.file_count = files_found as u64;
            stored.memory_count = indexed_source_ids.len() as u64;
            stored.last_sync_errors = errors as u64;
            stored.last_sync_error_detail = detail.clone();
            stored.status = SyncStatus::Paused;
        }
        Ok(())
    });
    Ok(SyncStatsResponse {
        files_found,
        ingested,
        skipped,
        errors,
        error_detail: detail,
        paused: None,
        queued_files: None,
        waiting_files: None,
        skipped_files,
    })
}

async fn prepare_item<C: ZoteroApi>(
    client: &mut C,
    item: &ZoteroItem,
) -> anyhow::Result<PreparedZoteroDocument> {
    let children_page = client.children(&item.key).await?;
    if !children_page.complete {
        anyhow::bail!("Zotero child item listing was incomplete");
    }
    let mut children = children_page.items;
    let mut all_children = children.clone();
    let mut fulltexts = Vec::new();
    let mut transient_fulltext_error = false;
    let mut attachment_keys = Vec::new();
    let mut attachment_types = Vec::new();
    let mut attachments = Vec::new();
    let mut pdf_warnings = Vec::new();
    if is_pdf_attachment(item) {
        match client.pdf_bytes(&item.key).await {
            Ok(bytes) => attachments.push(PreparedAttachment {
                key: item.key.clone(),
                title: item.data.title.clone(),
                content_type: item.data.content_type.clone(),
                pdf_bytes: Some(bytes),
            }),
            Err(error) => {
                pdf_warnings.push(format!(
                    "PDF attachment {} could not be cached: {error}",
                    item.key
                ));
                attachments.push(PreparedAttachment {
                    key: item.key.clone(),
                    title: item.data.title.clone(),
                    content_type: item.data.content_type.clone(),
                    pdf_bytes: None,
                });
            }
        }
        match client.fulltext(&item.key).await {
            Ok(text) => fulltexts.push(text),
            Err(error) => {
                transient_fulltext_error = true;
                pdf_warnings.push(format!(
                    "Zotero full-text fetch failed for {}: {error}",
                    item.key
                ));
            }
        }
    }
    for child in children.drain(..) {
        if child.data.item_type == "attachment" {
            attachment_keys.push(child.key.clone());
            if let Some(content_type) = &child.data.content_type {
                attachment_types.push(content_type.clone());
            }
            let pdf_bytes = if is_pdf_attachment(&child) {
                match client.pdf_bytes(&child.key).await {
                    Ok(bytes) => Some(bytes),
                    Err(error) => {
                        pdf_warnings.push(format!(
                            "PDF attachment {} could not be cached: {error}",
                            child.key
                        ));
                        None
                    }
                }
            } else {
                None
            };
            attachments.push(PreparedAttachment {
                key: child.key.clone(),
                title: child.data.title.clone(),
                content_type: child.data.content_type.clone(),
                pdf_bytes,
            });
        }
        if is_pdf_attachment(&child) {
            match client.fulltext(&child.key).await {
                Ok(text) => fulltexts.push(text),
                Err(error) => {
                    transient_fulltext_error = true;
                    pdf_warnings.push(format!(
                        "Zotero full-text fetch failed for {}: {error}",
                        child.key
                    ));
                }
            }
            let nested = client.children(&child.key).await?;
            if !nested.complete {
                anyhow::bail!("Zotero annotation listing was incomplete");
            }
            all_children.extend(nested.items);
        }
    }
    let fulltext = combine_fulltexts(fulltexts);
    let mut prepared = zotero::prepare_document(item, &all_children, &fulltext);
    prepared.warnings.extend(pdf_warnings);
    let mut attachment_metadata = HashMap::new();
    if !attachment_keys.is_empty() {
        attachment_metadata.insert("zotero_attachment_keys".into(), attachment_keys.join(" | "));
        attachment_metadata.insert(
            "zotero_attachment_types".into(),
            attachment_types.join(" | "),
        );
    }
    if prepared.url.is_none() {
        prepared.url = Some(format!("zotero://select/library/items/{}", item.key));
    }
    Ok(PreparedZoteroDocument {
        document: prepared,
        attachment_metadata,
        attachments,
        transient_fulltext_error,
    })
}

struct PreparedZoteroDocument {
    document: zotero::PreparedDocument,
    attachment_metadata: std::collections::HashMap<String, String>,
    attachments: Vec<PreparedAttachment>,
    transient_fulltext_error: bool,
}

struct PreparedAttachment {
    key: String,
    title: String,
    content_type: Option<String>,
    pdf_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ZoteroDocumentSidecar {
    source_id: String,
    space: Option<String>,
    server_id: Option<String>,
    item_key: String,
    title: String,
    rendered_content: String,
    bibliography: HashMap<String, String>,
    attachments: Vec<ZoteroAttachmentSidecar>,
    bibtex: String,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ZoteroAttachmentSidecar {
    key: String,
    title: String,
    content_type: Option<String>,
    cached_pdf_hash: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ZoteroSidecarHistory {
    current: Option<ZoteroDocumentSidecar>,
    #[serde(default)]
    snapshots: Vec<ZoteroDocumentSidecar>,
}

fn combine_fulltexts(texts: Vec<ZoteroFulltext>) -> ZoteroFulltext {
    let had_results = !texts.is_empty();
    let mut combined = String::new();
    let mut indexed = 0usize;
    let mut total = 0usize;
    let mut has_indexed = false;
    let mut has_total = false;
    let mut complete = true;
    for text in texts {
        if let Some(body) = text.text {
            if !combined.is_empty() {
                combined.push_str("\n\n---\n\n");
            }
            combined.push_str(&body);
        }
        if let Some(value) = text.indexed_pages {
            indexed += value;
            has_indexed = true;
        }
        if let Some(value) = text.total_pages {
            total += value;
            has_total = true;
        }
        complete &= text.complete;
    }
    let status = if combined.is_empty() {
        FulltextStatus::Missing
    } else {
        FulltextStatus::Present
    };
    ZoteroFulltext {
        status,
        text: (!combined.is_empty()).then_some(combined),
        indexed_pages: has_indexed.then_some(indexed),
        total_pages: has_total.then_some(total),
        complete: had_results && complete,
    }
}

fn is_pdf_attachment(item: &ZoteroItem) -> bool {
    item.data.item_type == "attachment"
        && item
            .data
            .content_type
            .as_deref()
            .is_some_and(|content_type| content_type.eq_ignore_ascii_case("application/pdf"))
}

fn prepared_hash(
    document: &zotero::PreparedDocument,
    extra_metadata: &HashMap<String, String>,
    item: &ZoteroItem,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(document.content.as_bytes());
    hasher.update(document.url.as_deref().unwrap_or_default().as_bytes());
    let mut metadata: Vec<_> = document.metadata.iter().collect();
    metadata.sort_unstable_by_key(|(key, _)| *key);
    for (key, value) in metadata {
        hasher.update(key.as_bytes());
        hasher.update([0]);
        hasher.update(value.as_bytes());
        hasher.update([0xff]);
    }
    let mut extra: Vec<_> = extra_metadata.iter().collect();
    extra.sort_unstable_by_key(|(key, _)| *key);
    for (key, value) in extra {
        hasher.update(key.as_bytes());
        hasher.update([0]);
        hasher.update(value.as_bytes());
        hasher.update([0xff]);
    }
    let mut zotero_extra: Vec<_> = item.data.extra.iter().collect();
    zotero_extra.sort_unstable_by_key(|(key, _)| *key);
    for (key, value) in zotero_extra {
        hasher.update(key.as_bytes());
        hasher.update([0]);
        hasher.update(value.to_string().as_bytes());
        hasher.update([0xfd]);
    }
    for warning in &document.warnings {
        hasher.update(warning.as_bytes());
        hasher.update([0xfe]);
    }
    format!("{:x}", hasher.finalize())
}

fn research_metadata(
    source_id: &str,
    document: &zotero::PreparedDocument,
    extra_metadata: HashMap<String, String>,
    server_id: Option<&str>,
    space: Option<&str>,
    item: &ZoteroItem,
) -> ZoteroDocumentSidecar {
    let mut bibliography = document.metadata.clone();
    for (key, value) in &item.data.extra {
        if let Some(value) = value.as_str() {
            bibliography.insert(key.clone(), value.to_owned());
        } else if !value.is_null() {
            bibliography.insert(key.clone(), value.to_string());
        }
    }
    bibliography.extend(extra_metadata);
    ZoteroDocumentSidecar {
        source_id: source_id.to_owned(),
        space: space.map(str::to_owned),
        server_id: server_id.map(str::to_owned),
        item_key: item.key.clone(),
        title: document.title.clone(),
        rendered_content: document.content.clone(),
        bibtex: bibliography_bibtex(item, &bibliography),
        bibliography,
        attachments: Vec::new(),
        warnings: document.warnings.clone(),
    }
}

fn bibliography_bibtex(item: &ZoteroItem, fields: &HashMap<String, String>) -> String {
    let kind = match item.data.item_type.as_str() {
        "journalArticle" => "article",
        "conferencePaper" => "inproceedings",
        "book" | "bookSection" => "book",
        "thesis" => "phdthesis",
        _ => "misc",
    };
    let author = item
        .data
        .creators
        .iter()
        .map(|creator| {
            if !creator.name.trim().is_empty() {
                creator.name.trim().to_owned()
            } else {
                format!("{} {}", creator.first_name.trim(), creator.last_name.trim())
                    .trim()
                    .to_owned()
            }
        })
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(" and ");
    let first_author = item
        .data
        .creators
        .first()
        .map(|creator| {
            if !creator.name.trim().is_empty() {
                creator.name.trim().to_owned()
            } else {
                creator.last_name.trim().to_owned()
            }
        })
        .unwrap_or_default();
    let citekey_name = first_author
        .rsplit_once(' ')
        .map(|(_, last)| last)
        .unwrap_or(&first_author);
    let year = bibtex_year(fields.get("date").map(String::as_str).unwrap_or_default());
    let citekey = format!(
        "{}{}{}",
        citekey_name
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .collect::<String>(),
        year,
        item.key
    );
    let ordered_fields = [
        ("author", None),
        ("title", None),
        ("year", None),
        ("journal", Some("publicationTitle")),
        ("booktitle", Some("proceedingsTitle")),
        ("volume", Some("volume")),
        ("number", Some("issue")),
        ("pages", Some("pages")),
        ("publisher", Some("publisher")),
        ("url", Some("url")),
        ("doi", Some("DOI")),
    ];
    let mut result = format!("@{kind}{{{citekey},\n");
    for (bib_field, key) in ordered_fields {
        let value = match (bib_field, key) {
            ("author", _) => Some(author.as_str()),
            ("title", _) => Some(item.data.title.as_str()),
            ("year", _) => (!year.is_empty()).then_some(year.as_str()),
            (_, Some(key)) => fields.get(key).map(String::as_str),
            ("doi", _) => fields.get("DOI").map(String::as_str),
            _ => None,
        };
        if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
            result.push_str(&format!("  {bib_field} = {{{}}},\n", bibtex_escape(value)));
        }
    }
    result.push('}');
    result
}

fn bibtex_year(date: &str) -> String {
    let bytes = date.as_bytes();
    bytes
        .windows(4)
        .find(|window| window.iter().all(u8::is_ascii_digit))
        .and_then(|window| std::str::from_utf8(window).ok())
        .filter(|year| matches!(year.parse::<u16>(), Ok(1000..=2999)))
        .unwrap_or_default()
        .to_owned()
}

fn bibtex_escape(value: &str) -> String {
    value
        .replace('\\', r"\textbackslash{}")
        .replace('{', r"\{")
        .replace('}', r"\}")
        .replace('&', r"\&")
        .replace('%', r"\%")
        .replace('$', r"\$")
        .replace('#', r"\#")
        .replace('_', r"\_")
}

fn zotero_root() -> PathBuf {
    config::data_root().join("zotero")
}

fn source_cache_key(source_id: &str) -> String {
    sha256_hex(source_id.as_bytes())
}

fn sidecar_path(source_id: &str) -> PathBuf {
    zotero_root()
        .join("documents")
        .join(format!("{}.json", source_cache_key(source_id)))
}

fn read_current_sidecar(source_id: &str) -> anyhow::Result<Option<ZoteroDocumentSidecar>> {
    let bytes = match std::fs::read(sidecar_path(source_id)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("read Zotero metadata sidecar"),
    };
    let sidecar: ZoteroSidecarHistory =
        serde_json::from_slice(&bytes).context("parse Zotero metadata sidecar")?;
    let current = sidecar.current;
    if current
        .as_ref()
        .is_some_and(|current| current.source_id != source_id)
    {
        anyhow::bail!("Zotero sidecar source identity did not match its managed key");
    }
    Ok(current)
}

fn pdf_cache_path(hash: &str) -> Result<PathBuf, ServerError> {
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ServerError::ValidationError(
            "Invalid cached PDF hash".into(),
        ));
    }
    Ok(zotero_root().join("pdfs").join(format!("{hash}.pdf")))
}

fn write_research_metadata(
    source_id: &str,
    snapshot: ZoteroDocumentSidecar,
    cached_pdfs: Vec<(String, Vec<u8>)>,
) -> anyhow::Result<()> {
    for (hash, bytes) in cached_pdfs {
        write_cached_pdf(&hash, &bytes)?;
    }
    let path = sidecar_path(source_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create Zotero metadata directory")?;
    }
    let mut history = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<ZoteroSidecarHistory>(&bytes)
            .context("read prior Zotero metadata snapshots")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ZoteroSidecarHistory::default()
        }
        Err(error) => return Err(error).context("read Zotero metadata sidecar"),
    };
    if let Some(previous) = history.current.take() {
        if previous != snapshot {
            history.snapshots.push(previous);
        }
    }
    history.current = Some(snapshot);
    atomic_json_write(&path, &history)
}

fn write_cached_pdf(hash: &str, bytes: &[u8]) -> anyhow::Result<()> {
    if sha256_hex(bytes) != hash {
        anyhow::bail!("PDF bytes did not match their content hash");
    }
    let path = pdf_cache_path(hash).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("PDF cache path has no parent"))?;
    std::fs::create_dir_all(parent).context("create Zotero PDF cache directory")?;
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            if std::fs::read(&path).is_ok_and(|existing| sha256_hex(&existing) == hash) {
                return Ok(());
            }
        }
        Ok(_) => anyhow::bail!("cached PDF path is not a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect cached Zotero PDF"),
    }
    atomic_bytes_write(&path, bytes)
}

fn atomic_json_write(path: &std::path::Path, value: &impl Serialize) -> anyhow::Result<()> {
    atomic_bytes_write(path, &serde_json::to_vec_pretty(value)?)
}

fn atomic_bytes_write(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("managed file has no parent"))?;
    for _ in 0..32 {
        let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".wenlan-zotero-{}-{suffix}.tmp",
            std::process::id()
        ));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create managed Zotero temp file"),
        };
        use std::io::Write;
        file.write_all(bytes).context("write managed Zotero file")?;
        file.sync_all().context("sync managed Zotero file")?;
        drop(file);
        std::fs::rename(&temporary, path).context("atomically replace managed Zotero file")?;
        return Ok(());
    }
    anyhow::bail!("could not allocate a unique Zotero temporary file")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn canonical_source_id(server_id: Option<&str>, item_key: &str, space_id: Option<&str>) -> String {
    format!(
        "zotero::{}::users/0::{}::{}",
        server_id.map(hex).unwrap_or_else(|| "legacy".into()),
        item_key,
        space_id.unwrap_or("uncategorized")
    )
}

fn locator(server_id: &Option<String>, collection_key: &str) -> String {
    let identity = server_id
        .as_deref()
        .map(hex)
        .unwrap_or_else(|| "legacy".into());
    format!("{LOCATOR_PREFIX}/{identity}/users/0/collections/{collection_key}")
}

fn parse_locator(path: &std::path::Path) -> Result<(Option<String>, String), ServerError> {
    let text = path
        .to_str()
        .ok_or_else(|| ServerError::ValidationError("Zotero source locator is not UTF-8".into()))?;
    let parts: Vec<_> = text.split('/').collect();
    if parts.len() != 6
        || parts[0] != LOCATOR_PREFIX
        || parts[2] != "users"
        || parts[3] != "0"
        || parts[4] != "collections"
    {
        return Err(ServerError::ValidationError(
            "Zotero source locator is malformed".into(),
        ));
    }
    validate_collection_key(parts[5])?;
    let server_id = if parts[1] == "legacy" {
        None
    } else {
        Some(unhex(parts[1]).ok_or_else(|| {
            ServerError::ValidationError("Zotero source identity is malformed".into())
        })?)
    };
    Ok((server_id, parts[5].to_owned()))
}

fn summary(collection: ZoteroCollection) -> Result<ZoteroCollectionSummary, ServerError> {
    let item_count = collection.meta.num_items.ok_or_else(|| {
        ServerError::Upstream(format!(
            "Zotero did not report an item count for collection {}",
            collection.key
        ))
    })?;
    Ok(ZoteroCollectionSummary {
        key: collection.key,
        name: collection.data.name,
        item_count,
    })
}

fn validate_collection_key(key: &str) -> Result<(), ServerError> {
    if key.is_empty() || key.len() > 16 || !key.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(ServerError::ValidationError(
            "Invalid Zotero collection key".into(),
        ));
    }
    Ok(())
}

fn hex(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn unhex(value: &str) -> Option<String> {
    if value.is_empty()
        || !value.len().is_multiple_of(2)
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

fn add_skipped(files: &mut Vec<SkippedFile>, title: &str, key: &str, reason: &str) {
    if files.len() < SKIPPED_FILES_LIMIT {
        files.push(SkippedFile {
            path: format!("{} [{key}]", title.trim()).trim().to_owned(),
            reason: reason.to_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wenlan_core::{
        read_scope::ReadScope,
        sources::zotero::{CollectionData, Creator, ItemData, ZoteroPageSet},
    };

    #[derive(Clone)]
    struct FakeZotero {
        identity: Option<String>,
        collection: ZoteroCollection,
        items: Vec<ZoteroItem>,
        fail_children_for: HashSet<String>,
    }

    impl ZoteroApi for FakeZotero {
        fn server_id(&self) -> Option<&str> {
            self.identity.as_deref()
        }

        fn list_collections(
            &mut self,
        ) -> impl Future<Output = anyhow::Result<zotero::ZoteroPageSet<ZoteroCollection>>> + Send
        {
            let collection = self.collection.clone();
            async move { Ok(page(vec![collection])) }
        }

        fn collection_items(
            &mut self,
            key: &str,
        ) -> impl Future<Output = anyhow::Result<zotero::ZoteroPageSet<ZoteroItem>>> + Send
        {
            let valid = key == self.collection.key;
            let items = self.items.clone();
            async move {
                if !valid {
                    anyhow::bail!("unexpected collection key")
                }
                Ok(page(items))
            }
        }

        fn children(
            &mut self,
            key: &str,
        ) -> impl Future<Output = anyhow::Result<zotero::ZoteroPageSet<ZoteroItem>>> + Send
        {
            let fail = self.fail_children_for.contains(key);
            async move {
                if fail {
                    anyhow::bail!("fixture child request failed")
                }
                Ok(page(Vec::new()))
            }
        }

        async fn fulltext(&mut self, _key: &str) -> anyhow::Result<ZoteroFulltext> {
            Ok(ZoteroFulltext {
                status: FulltextStatus::Missing,
                text: None,
                indexed_pages: None,
                total_pages: None,
                complete: false,
            })
        }
        async fn pdf_bytes(&mut self, _key: &str) -> anyhow::Result<Vec<u8>> {
            anyhow::bail!("no fixture PDF")
        }
    }

    fn page<T>(items: Vec<T>) -> ZoteroPageSet<T> {
        ZoteroPageSet {
            total_results: Some(items.len()),
            total_pages: Some(1),
            items,
            complete: true,
        }
    }

    fn article(key: &str, title: &str, abstract_note: &str) -> ZoteroItem {
        ZoteroItem {
            key: key.to_owned(),
            version: Some(1),
            data: ItemData {
                item_type: "journalArticle".into(),
                title: title.to_owned(),
                abstract_note: abstract_note.to_owned(),
                date: "2025".into(),
                doi: "10.1234/example".into(),
                creators: vec![Creator {
                    first_name: "Ada".into(),
                    last_name: "Lovelace".into(),
                    creator_type: "author".into(),
                    ..Creator::default()
                }],
                tags: vec![wenlan_core::sources::zotero::Tag {
                    tag: "zotero-only".into(),
                    r#type: None,
                }],
                ..ItemData::default()
            },
        }
    }

    fn fake(items: Vec<ZoteroItem>, fail_children_for: &[&str]) -> FakeZotero {
        FakeZotero {
            identity: Some("test-server".into()),
            collection: ZoteroCollection {
                key: "COLLECT1".into(),
                version: Some(1),
                data: CollectionData {
                    name: "Research".into(),
                    parent_collection: None,
                },
                meta: zotero::CollectionMeta {
                    num_items: Some(items.len()),
                },
            },
            items,
            fail_children_for: fail_children_for.iter().map(|key| (*key).into()).collect(),
        }
    }

    fn source() -> Source {
        Source {
            id: "zotero-research-0123456789".into(),
            source_type: SourceType::Zotero,
            path: PathBuf::from(locator(&Some("test-server".into()), "COLLECT1")),
            status: SyncStatus::Paused,
            last_sync: None,
            file_count: 0,
            memory_count: 0,
            last_sync_errors: 0,
            last_sync_error_detail: None,
            space: Some("Research".into()),
            queued_files: 0,
            waiting_files: 0,
        }
    }

    async fn data_dir_lock() -> tokio::sync::MutexGuard<'static, ()> {
        crate::TEST_DATA_DIR_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await
    }

    #[test]
    fn locator_round_trips_server_identity_and_legacy_source() {
        let path = PathBuf::from(locator(&Some("Zotero/7.0".into()), "AB12CD34"));
        assert_eq!(
            parse_locator(&path).unwrap(),
            (Some("Zotero/7.0".into()), "AB12CD34".into())
        );
        assert_eq!(
            parse_locator(&PathBuf::from(locator(&None, "AB12CD34"))).unwrap(),
            (None, "AB12CD34".into())
        );
        assert!(parse_locator(&PathBuf::from("/tmp/users/0/collections/AB12CD34")).is_err());
    }

    #[test]
    fn item_identity_is_stable_across_collections_but_scoped_by_server_and_space() {
        let first = canonical_source_id(Some("server"), "ITEMKEY1", Some("space-1"));
        assert_eq!(
            first,
            canonical_source_id(Some("server"), "ITEMKEY1", Some("space-1"))
        );
        assert_ne!(
            first,
            canonical_source_id(Some("other"), "ITEMKEY1", Some("space-1"))
        );
        assert_ne!(
            first,
            canonical_source_id(Some("server"), "ITEMKEY1", Some("space-2"))
        );
    }

    #[test]
    fn prepared_hash_includes_metadata_with_deterministic_order() {
        let mut a = zotero::PreparedDocument {
            key: "ITEMKEY1".into(),
            title: "title".into(),
            content: "body".into(),
            url: None,
            metadata: Default::default(),
            warnings: vec![],
        };
        a.metadata.insert("z".into(), "last".into());
        a.metadata.insert("a".into(), "first".into());
        let mut b = a.clone();
        b.metadata = [("a".into(), "first".into()), ("z".into(), "last".into())]
            .into_iter()
            .collect();
        assert_eq!(
            prepared_hash(&a, &HashMap::new(), &article("ITEM0001", "title", "")),
            prepared_hash(&b, &HashMap::new(), &article("ITEM0001", "title", ""))
        );
        b.metadata.insert("z".into(), "changed".into());
        assert_ne!(
            prepared_hash(&a, &HashMap::new(), &article("ITEM0001", "title", "")),
            prepared_hash(&b, &HashMap::new(), &article("ITEM0001", "title", ""))
        );
        let item = article("ITEM0001", "title", "");
        let mut changed_item = item.clone();
        changed_item
            .data
            .extra
            .insert("publicationTitle".into(), serde_json::json!("A journal"));
        assert_ne!(
            prepared_hash(&a, &HashMap::new(), &item),
            prepared_hash(&a, &HashMap::new(), &changed_item)
        );
    }

    #[test]
    fn bibtex_uses_bibtex_authors_and_year_and_escapes_special_characters() {
        let mut item = article("ITEM0001", "Rules & 100%", "");
        item.data.date = "November 3, 2026".into();
        item.data.creators[0].last_name = "Lovelace_Research".into();
        item.data.creators.push(Creator {
            first_name: "Grace".into(),
            last_name: "Hopper".into(),
            creator_type: "author".into(),
            ..Creator::default()
        });
        let mut fields = HashMap::new();
        fields.insert("date".into(), item.data.date.clone());
        fields.insert("DOI".into(), item.data.doi.clone());
        let bibtex = bibliography_bibtex(&item, &fields);
        assert!(bibtex.contains("author = {Ada Lovelace\\_Research and Grace Hopper}"));
        assert!(bibtex.contains("title = {Rules \\& 100\\%}"));
        assert!(bibtex.contains("year = {2026}"));
        assert!(!bibtex.contains("date = {"));
    }

    #[tokio::test]
    async fn document_and_pdf_require_live_exact_zotero_file_without_collision() {
        let _lock = data_dir_lock().await;
        let _data_root = super::super::tests::DataDirGuard::new();
        let (db, db_dir) = super::super::tests::new_test_db().await;
        db.create_space("Research", None, false).await.unwrap();
        let source_id = canonical_source_id(Some("test-server"), "ITEM0001", Some("Research"));
        let document = RawDocument {
            source: "file".into(),
            source_id: source_id.clone(),
            source_agent: Some("zotero".into()),
            title: "Paper".into(),
            content: "Original indexed paper".into(),
            space: Some("Research".into()),
            ..RawDocument::default()
        };
        db.upsert_documents(vec![document.clone()]).await.unwrap();
        let pdf = b"%PDF-1.7\nfixture".to_vec();
        let hash = sha256_hex(&pdf);
        write_research_metadata(
            &source_id,
            ZoteroDocumentSidecar {
                source_id: source_id.clone(),
                space: Some("Research".into()),
                server_id: Some("test-server".into()),
                item_key: "ITEM0001".into(),
                title: "Paper".into(),
                rendered_content: document.content.clone(),
                bibliography: HashMap::new(),
                bibtex: String::new(),
                warnings: vec![],
                attachments: vec![ZoteroAttachmentSidecar {
                    key: "ATTACH01".into(),
                    title: "paper.pdf".into(),
                    content_type: Some("application/pdf".into()),
                    cached_pdf_hash: Some(hash.clone()),
                }],
            },
            vec![(hash, pdf)],
        )
        .unwrap();
        let state = Arc::new(RwLock::new(ServerState {
            db: Some(db.clone()),
            ..ServerState::default()
        }));
        let doc_query = ZoteroDocumentQuery {
            source_id: source_id.clone(),
            space: Some("Research".into()),
        };
        assert!(authorized_document(&state, &doc_query, None).await.is_ok());
        assert!(handle_get_pdf(
            State(state.clone()),
            crate::space_header::SpaceHeader(None),
            Query(PdfQuery {
                source_id: source_id.clone(),
                attachment_key: "ATTACH01".into(),
                space: Some("Research".into())
            })
        )
        .await
        .is_ok());
        let fixture = libsql::Builder::new_local(db_dir.path().join("origin_memory.db"))
            .build()
            .await
            .unwrap();
        fixture.connect().unwrap().execute(
            "INSERT INTO memories (id,source,source_id,title,content,chunk_index,last_modified,chunk_type,space)
             VALUES ('collision-memory-row','memory',?1,'Memory','Unrelated ordinary memory',0,1,'text','Research')",
            libsql::params![source_id.as_str()],
        ).await.unwrap();
        for remove_file in [false, true] {
            if remove_file {
                db.delete_by_source_id("file", &source_id).await.unwrap();
            }
            // The old generic reader still resolves this ID; it cannot authorize a Zotero sidecar.
            assert!(db
                .get_chunks_scoped(&source_id, &ReadScope::Space("Research".into()))
                .await
                .unwrap()
                .is_some());
            assert!(matches!(
                authorized_document(&state, &doc_query, None).await,
                Err(ServerError::NotFound(_))
            ));
            assert!(matches!(
                handle_get_pdf(
                    State(state.clone()),
                    crate::space_header::SpaceHeader(None),
                    Query(PdfQuery {
                        source_id: source_id.clone(),
                        attachment_key: "ATTACH01".into(),
                        space: Some("Research".into())
                    })
                )
                .await,
                Err(ServerError::NotFound(_))
            ));
        }
        db.delete_by_source_id("memory", &source_id).await.unwrap();
        db.upsert_documents(vec![RawDocument {
            source_agent: Some("folder".into()),
            ..document
        }])
        .await
        .unwrap();
        assert!(matches!(
            authorized_document(&state, &doc_query, None).await,
            Err(ServerError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn fake_sync_is_repeatable_scoped_partial_and_disconnect_preserves_documents() {
        let _lock = data_dir_lock().await;
        let _data_root = super::super::tests::DataDirGuard::new();
        let (db, _db_dir) = super::super::tests::new_test_db().await;
        db.create_space("Research", None, false).await.unwrap();
        db.create_space("Other", None, false).await.unwrap();
        let source = source();
        config::save_config(&config::Config {
            sources: vec![source.clone()],
            ..config::Config::default()
        })
        .unwrap();

        let items = (1..=21)
            .map(|number| {
                article(
                    &format!("ITEM{number:04}"),
                    &format!("Fixture paper {number}"),
                    if number == 1 {
                        "First stable abstract."
                    } else {
                        "Stable fixture abstract."
                    },
                )
            })
            .collect::<Vec<_>>();
        let mut client = fake(items, &[]);
        let first = sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .unwrap();
        assert_eq!(
            (first.files_found, first.ingested, first.errors),
            (21, 21, 0)
        );
        assert!(first.skipped_files.is_empty());
        let canonical = canonical_source_id(
            Some("test-server"),
            "ITEM0001",
            db.get_space("Research")
                .await
                .unwrap()
                .as_ref()
                .map(|space| space.id.as_str()),
        );
        let before = db
            .get_chunks_scoped(&canonical, &ReadScope::Space("Research".into()))
            .await
            .unwrap()
            .unwrap();
        assert!(!before.is_empty());
        assert!(db
            .get_chunks_scoped(
                &canonical_source_id(
                    Some("test-server"),
                    "ITEM0021",
                    db.get_space("Research")
                        .await
                        .unwrap()
                        .as_ref()
                        .map(|space| space.id.as_str())
                ),
                &ReadScope::Space("Research".into()),
            )
            .await
            .unwrap()
            .is_some());

        let repeated = sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .unwrap();
        assert_eq!(
            repeated.ingested, 0,
            "unchanged Zotero text should not upsert again"
        );
        assert_eq!(
            config::load_config().sources[0].memory_count,
            21,
            "repeat sync must not grow source memory_count"
        );
        assert_eq!(
            db.get_chunks_scoped(&canonical, &ReadScope::Space("Research".into()))
                .await
                .unwrap()
                .unwrap()
                .len(),
            before.len()
        );

        db.set_document_tags("file", &canonical, vec!["my curated tag".into()])
            .await
            .unwrap();
        client.items[0].data.abstract_note = "The Zotero abstract changed.".into();
        client.fail_children_for.insert("ITEM0021".into());
        let changed = sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .unwrap();
        assert_eq!(changed.ingested, 1);
        assert_eq!(changed.errors, 1);
        assert_eq!(config::load_config().sources[0].memory_count, 21);
        assert!(db
            .get_chunks_scoped(
                &canonical_source_id(
                    Some("test-server"),
                    "ITEM0021",
                    db.get_space("Research")
                        .await
                        .unwrap()
                        .as_ref()
                        .map(|space| space.id.as_str()),
                ),
                &ReadScope::Space("Research".into()),
            )
            .await
            .unwrap()
            .is_some());
        assert_eq!(
            db.get_document_tags("file", &canonical).await.unwrap(),
            vec!["my curated tag"]
        );
        let snapshots = std::fs::read(sidecar_path(&canonical)).unwrap();
        let snapshots: ZoteroSidecarHistory = serde_json::from_slice(&snapshots).unwrap();
        assert_eq!(snapshots.snapshots.len(), 1);
        assert!(snapshots.snapshots[0]
            .rendered_content
            .contains("First stable abstract."));

        client.items.push(article(
            "ITEM0022",
            "New fixture paper",
            "New indexed item.",
        ));
        let with_new_article = sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .unwrap();
        assert_eq!(
            (with_new_article.files_found, with_new_article.ingested),
            (22, 1)
        );
        assert_eq!(config::load_config().sources[0].memory_count, 22);

        let state = Arc::new(RwLock::new(ServerState {
            db: Some(db.clone()),
            ..ServerState::default()
        }));
        let response = handle_get_document(
            State(state.clone()),
            crate::space_header::SpaceHeader(None),
            Query(ZoteroDocumentQuery {
                source_id: canonical.clone(),
                space: Some("Research".into()),
            }),
        )
        .await
        .unwrap();
        let (scope_headers, response) = response;
        assert_eq!(scope_headers["x-wenlan-source-scope"], "Research");
        assert!(response.bibtex.contains("@article{"));
        assert_eq!(response.source_id, canonical);
        let pdf = b"%PDF-1.7\nfixture PDF bytes\n".to_vec();
        let pdf_hash = sha256_hex(&pdf);
        let mut sidecar = read_current_sidecar(&canonical).unwrap().unwrap();
        sidecar.attachments.push(ZoteroAttachmentSidecar {
            key: "ATTACH01".into(),
            title: "fixture.pdf".into(),
            content_type: Some("application/pdf".into()),
            cached_pdf_hash: Some(pdf_hash.clone()),
        });
        write_research_metadata(&canonical, sidecar, vec![(pdf_hash, pdf.clone())]).unwrap();
        let response_with_pdf = handle_get_document(
            State(state.clone()),
            crate::space_header::SpaceHeader(None),
            Query(ZoteroDocumentQuery {
                source_id: canonical.clone(),
                space: Some("Research".into()),
            }),
        )
        .await
        .unwrap()
        .1
         .0;
        assert_eq!(response_with_pdf.attachments[0].key, "ATTACH01");
        assert!(response_with_pdf.attachments[0].available);
        let pdf_response = handle_get_pdf(
            State(state.clone()),
            crate::space_header::SpaceHeader(None),
            Query(PdfQuery {
                source_id: canonical.clone(),
                attachment_key: "ATTACH01".into(),
                space: Some("Research".into()),
            }),
        )
        .await
        .unwrap()
        .into_response();
        assert_eq!(
            pdf_response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "application/pdf"
        );
        let pdf_body = axum::body::to_bytes(pdf_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(pdf_body.as_ref(), pdf.as_slice());
        assert!(handle_get_document(
            State(state.clone()),
            crate::space_header::SpaceHeader(None),
            Query(ZoteroDocumentQuery {
                source_id: canonical.clone(),
                space: Some("Other".into()),
            }),
        )
        .await
        .is_err());
        assert!(matches!(
            handle_get_document(
                State(state.clone()),
                crate::space_header::SpaceHeader(Some("Research".into())),
                Query(ZoteroDocumentQuery {
                    source_id: canonical.clone(),
                    space: Some("Other".into()),
                }),
            )
            .await,
            Err(ServerError::BadRequest(_))
        ));
        assert!(matches!(
            handle_get_pdf(
                State(state.clone()),
                crate::space_header::SpaceHeader(Some("Research".into())),
                Query(PdfQuery {
                    source_id: canonical.clone(),
                    attachment_key: "ATTACH01".into(),
                    space: Some("Other".into()),
                }),
            )
            .await,
            Err(ServerError::BadRequest(_))
        ));

        let mismatch = fake(vec![article("ITEM0001", "Fixture paper", "changed")], &[]);
        let mut mismatch = FakeZotero {
            identity: Some("another-server".into()),
            ..mismatch
        };
        assert!(matches!(
            sync_source_with_client(db.clone(), &source, &mut mismatch).await,
            Err(ServerError::Upstream(_))
        ));

        let corrupt_sidecar_id = canonical_source_id(
            Some("test-server"),
            "ITEM0002",
            db.get_space("Research")
                .await
                .unwrap()
                .as_ref()
                .map(|space| space.id.as_str()),
        );
        std::fs::write(sidecar_path(&corrupt_sidecar_id), b"not-json").unwrap();
        client.fail_children_for.clear();
        let sidecar_error = sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .unwrap();
        assert_eq!(
            (
                sidecar_error.files_found,
                sidecar_error.ingested,
                sidecar_error.errors
            ),
            (22, 0, 1)
        );
        assert!(db
            .get_chunks_scoped(&corrupt_sidecar_id, &ReadScope::Space("Research".into()),)
            .await
            .unwrap()
            .is_some());

        let _ = super::super::handle_remove_source(
            State(state),
            axum::extract::Path(source.id.clone()),
        )
        .await
        .unwrap();
        assert!(!db
            .get_chunks_scoped(&canonical, &ReadScope::Space("Research".into()))
            .await
            .unwrap()
            .unwrap()
            .is_empty());
    }
}
