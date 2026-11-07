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
use wenlan_types::zotero::{ZoteroDisconnectPreview, ZoteroRegistrationIdentity};

pub(super) static ZOTERO_MUTATION_LANE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
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
        ZoteroCollectionsResponse, ZoteroDocumentResponse, ZoteroItemSummary, ZoteroItemsResponse,
        ZoteroPdfAnnotation, ZoteroPdfPageResponse,
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
    fn attachment_annotations(
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
    async fn attachment_annotations(
        &mut self,
        key: &str,
    ) -> anyhow::Result<zotero::ZoteroPageSet<ZoteroItem>> {
        ZoteroClient::attachment_annotations(self, key).await
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

const MAX_SELECTED_ITEMS: usize = 1000;

#[derive(Debug, Deserialize)]
pub(crate) struct ZoteroItemsQuery {
    collection_key: String,
    expected_server_id: String,
}

pub async fn handle_list_items(
    client: Option<Extension<ZoteroClient>>,
    Query(query): Query<ZoteroItemsQuery>,
) -> Result<Json<ZoteroItemsResponse>, ServerError> {
    let mut client = match client {
        Some(Extension(client)) => client,
        None => ZoteroClient::with_server_id(Some(query.expected_server_id.clone()))
            .map_err(|e| ServerError::Upstream(e.to_string()))?,
    };
    Ok(Json(
        list_selectable_items(
            &mut client,
            &query.collection_key,
            &query.expected_server_id,
        )
        .await?,
    ))
}

fn is_selectable_item(item: &ZoteroItem) -> bool {
    item.data.parent_item.is_none()
        && !matches!(
            item.data.item_type.as_str(),
            "attachment" | "note" | "annotation"
        )
}

fn canonical_item_keys(keys: &[String]) -> Result<Vec<String>, ServerError> {
    if keys.is_empty()
        || keys.len() > MAX_SELECTED_ITEMS
        || keys.iter().any(|key| {
            key.len() != 8
                || !key
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        })
    {
        return Err(ServerError::ValidationError(
            "Zotero item selection must contain 1..=1000 exact eight-character uppercase item keys"
                .into(),
        ));
    }
    let mut keys = keys.to_vec();
    keys.sort();
    keys.dedup();
    Ok(keys)
}

fn validate_listed_item(item: &ZoteroItem, collection_key: &str) -> Result<(), ServerError> {
    canonical_item_keys(std::slice::from_ref(&item.key))?;
    if item.data.item_type.trim().is_empty()
        || item
            .data
            .extra
            .get("key")
            .is_some_and(|key| key.as_str() != Some(item.key.as_str()))
    {
        return Err(ServerError::Upstream(
            "Zotero item listing contained malformed or conflicting identities".into(),
        ));
    }
    if let Some(collections) = item.data.extra.get("collections") {
        if collections.as_array().is_none_or(|values| {
            !values
                .iter()
                .any(|value| value.as_str() == Some(collection_key))
        }) {
            return Err(ServerError::Upstream(
                "Zotero item listing contained an item outside the registered collection".into(),
            ));
        }
    }
    Ok(())
}

async fn list_selectable_items<C: ZoteroApi>(
    client: &mut C,
    collection_key: &str,
    expected_server: &str,
) -> Result<ZoteroItemsResponse, ServerError> {
    validate_collection_key(collection_key)?;
    if expected_server.trim().is_empty()
        || client.server_id().is_some_and(|id| id != expected_server)
    {
        return Err(ServerError::Upstream(
            "Zotero server identity does not match the reviewed connection".into(),
        ));
    }
    let collections = client
        .list_collections()
        .await
        .map_err(|e| ServerError::Upstream(e.to_string()))?;
    if !collections.complete || !collections.items.iter().any(|c| c.key == collection_key) {
        return Err(ServerError::Upstream(
            "Zotero collection is unavailable or listing was incomplete".into(),
        ));
    }
    let page = client
        .collection_items(collection_key)
        .await
        .map_err(|e| ServerError::Upstream(e.to_string()))?;
    if !page.complete || client.server_id() != Some(expected_server) {
        return Err(ServerError::Upstream(
            "Zotero item listing was incomplete or server identity changed".into(),
        ));
    }
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for item in page.items.into_iter().filter(is_selectable_item) {
        validate_listed_item(&item, collection_key)?;
        if !seen.insert(item.key.clone()) {
            return Err(ServerError::Upstream(
                "Zotero item listing contained duplicate identities".into(),
            ));
        }
        items.push(ZoteroItemSummary {
            key: item.key,
            title: item.data.title,
            item_type: item.data.item_type,
        });
    }
    items.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(ZoteroItemsResponse {
        server_id: expected_server.to_owned(),
        collection_key: collection_key.to_owned(),
        items,
        complete: true,
    })
}

async fn validate_selection<C: ZoteroApi>(
    client: &mut C,
    collection: &str,
    expected_server: &str,
    keys: &[String],
) -> Result<Vec<String>, ServerError> {
    let keys = canonical_item_keys(keys)?;
    let listed = list_selectable_items(client, collection, expected_server).await?;
    if keys
        .iter()
        .any(|key| !listed.items.iter().any(|item| &item.key == key))
    {
        return Err(ServerError::ValidationError("Zotero selection contains an item no longer present in the reviewed collection; reload its article list".into()));
    }
    Ok(keys)
}

pub async fn handle_add_source(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(body): Json<AddZoteroSourceRequest>,
) -> Result<Json<Source>, ServerError> {
    validate_collection_key(&body.collection_key)?;
    if let Some(keys) = &body.item_keys {
        canonical_item_keys(keys)?;
        if body
            .expected_server_id
            .as_deref()
            .is_none_or(|id| id.trim().is_empty())
        {
            return Err(ServerError::ValidationError(
                "A selected article import requires the reviewed Zotero server identity".into(),
            ));
        }
    }
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

    let item_keys = match body.item_keys.as_deref() {
        Some(keys) => Some(
            validate_selection(
                &mut client,
                &body.collection_key,
                server_id.as_deref().expect("checked stable identity"),
                keys,
            )
            .await?,
        ),
        None => None,
    };
    let locator = scoped_locator(&server_id, &body.collection_key, item_keys.as_deref());
    let _mutation = ZOTERO_MUTATION_LANE.lock().await;
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
        "zotero-{}-{}-{}",
        wenlan_core::export::obsidian::slugify(&collection.data.name),
        &space_hash[..10],
        uuid::Uuid::new_v4().simple()
    );
    let (source, created) = config::update_config(|config| {
        if let Some(existing) = config.sources.iter().find(|source| {
            source.source_type == SourceType::Zotero
                && source.path == std::path::Path::new(&locator)
                && source.space.as_deref() == Some(space_name.as_str())
        }) {
            return Ok((existing.clone(), false));
        }
        let mut id = base_id.clone();
        let mut suffix = 2;
        while config.sources.iter().any(|source| source.id == id) {
            id = format!("{base_id}-{suffix}");
            suffix += 1;
        }
        let source = Source {
            display_name: Some(collection.data.name.clone()),
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
        Ok((source, true))
    })?;
    let identity = registration_identity(&db, &source).await?;
    if created {
        db.bind_zotero_registration(&identity).await?;
    } else {
        db.bind_zotero_registration_baseline(&identity).await?;
    }
    Ok(Json(source))
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ZoteroDocumentQuery {
    source_id: String,
    space: Option<String>,
    #[serde(default)]
    attachment_key: Option<String>,
    #[serde(default)]
    expected_digest: Option<String>,
    #[serde(default)]
    allow_retained: bool,
}

pub async fn handle_get_document(
    State(state): State<SharedState>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Query(query): Query<ZoteroDocumentQuery>,
) -> Result<(HeaderMap, Json<ZoteroDocumentResponse>), ServerError> {
    let (sidecar, selected_space) =
        authorized_document(&state, &query, header_space.as_deref()).await?;
    let (current, historical) = selected_pdf_generation(
        &sidecar,
        query.attachment_key.as_deref(),
        query.expected_digest.as_deref(),
        query.allow_retained,
    )?;
    let current = current.clone();
    let registered_source_id = current.import_registration.as_ref().and_then(|provenance| {
        config::load_config()
            .sources
            .into_iter()
            .find(|source| {
                source.id == provenance.id
                    && source.source_type == SourceType::Zotero
                    && source.path == std::path::Path::new(&provenance.locator)
                    && source.space == current.space
                    && parse_locator(&source.path)
                        .ok()
                        .is_some_and(|locator| locator.server_id == current.server_id)
            })
            .map(|source| source.id)
    });
    let attachments = current
        .attachments
        .iter()
        .map(|attachment| ZoteroAttachmentResponse {
            key: attachment.key.clone(),
            title: attachment.title.clone(),
            pdf_digest: attachment.cached_pdf_hash.clone(),
            available: attachment
                .cached_pdf_hash
                .as_deref()
                .is_some_and(cached_pdf_is_valid),
        })
        .collect();
    Ok((
        source_scope_headers(&selected_space)?,
        Json(ZoteroDocumentResponse {
            historical,
            source_id: current.source_id,
            title: current.title,
            attachments,
            bibtex: current.bibtex,
            warnings: current.warnings,
            annotations: current.annotations,
            source_url: current
                .bibliography
                .get("url")
                .and_then(|url| safe_public_url(url)),
            registered_source_id,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub(crate) struct PdfPageQuery {
    source_id: String,
    attachment_key: String,
    space: Option<String>,
    page: u32,
}

pub async fn handle_get_pdf_page(
    State(state): State<SharedState>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Query(query): Query<PdfPageQuery>,
) -> Result<(HeaderMap, Json<ZoteroPdfPageResponse>), ServerError> {
    let (history, selected_space) = authorized_document(
        &state,
        &ZoteroDocumentQuery {
            source_id: query.source_id.clone(),
            space: query.space,
            ..Default::default()
        },
        header_space.as_deref(),
    )
    .await?;
    let current = history
        .current
        .ok_or_else(|| ServerError::NotFound("Zotero document missing".into()))?;
    let attachment = current
        .attachments
        .iter()
        .find(|a| a.key == query.attachment_key)
        .ok_or_else(|| ServerError::NotFound("Zotero attachment missing".into()))?;
    let hash = attachment
        .cached_pdf_hash
        .as_ref()
        .ok_or_else(|| ServerError::NotFound("Zotero PDF not cached".into()))?;
    let bytes = read_cached_pdf(hash)
        .await?
        .ok_or_else(|| ServerError::NotFound("Zotero PDF not cached".into()))?;
    let (text, page_count) = tokio::task::spawn_blocking(move || {
        wenlan_core::sources::directory::extract_pdf_page(&bytes, query.page)
    })
    .await
    .map_err(|e| ServerError::Internal(e.to_string()))?
    .map_err(ServerError::ValidationError)?;
    // Never return a prefix as a complete page: callers can choose a smaller page or report coverage.
    if text.chars().count() > 64_000 {
        return Err(ServerError::ValidationError(
            "PDF page exceeds 64000 character read bound".into(),
        ));
    }
    Ok((
        source_scope_headers(&selected_space)?,
        Json(ZoteroPdfPageResponse {
            source_lifecycle: {
                let db = {
                    state
                        .read()
                        .await
                        .db
                        .clone()
                        .ok_or(ServerError::DbNotInitialized)?
                };
                db.source_lifecycle_summary(Some(&selected_space), "file", &query.source_id)
                    .await?
            },
            source_id: query.source_id,
            attachment_key: query.attachment_key,
            pdf_digest: hash.clone(),
            page: query.page,
            page_count,
            text,
            extraction_complete: true,
        }),
    ))
}

/// Validate immutable PDF identities before a page or proposal is stored.
/// A later sync may replace the current attachment; this target still names the exact old bytes.
pub(crate) async fn attach_pdf_targets(
    citations_json: &mut String,
    targets: &[wenlan_types::pages::PdfCitationInput],
    source_ids: &[String],
    scope: &wenlan_core::read_scope::ReadScope,
) -> Result<(), ServerError> {
    if targets.len() > 256 {
        return Err(ServerError::ValidationError(
            "too many PDF citation targets".into(),
        ));
    }
    let mut citations: Vec<wenlan_types::pages::PageCitation> =
        serde_json::from_str(citations_json).map_err(|e| ServerError::Internal(e.to_string()))?;
    let mut seen = HashSet::new();
    for input in targets {
        let fail = || {
            ServerError::ValidationError("invalid_pdf_target: occurrence, source, attachment, digest, page or exact quote does not match".into())
        };
        if !seen.insert(input.occurrence) {
            return Err(fail());
        }
        let citation = citations
            .iter_mut()
            .find(|c| c.occurrence == input.occurrence)
            .ok_or_else(fail)?;
        if citation.source_kind != "memory" || !source_ids.contains(&citation.locator) {
            return Err(fail());
        }
        let target = &input.target;
        if target.quote.trim().is_empty()
            || target.quote.len() > 16_000
            || target.pdf_digest.len() != 64
        {
            return Err(fail());
        }
        let current = read_current_sidecar(&citation.locator)
            .map_err(|_| fail())?
            .ok_or_else(fail)?;
        if !scope_matches_space(scope, current.space.as_deref()) {
            return Err(fail());
        }
        if !current.attachments.iter().any(|a| {
            a.key == target.attachment_key
                && a.cached_pdf_hash.as_deref() == Some(&target.pdf_digest)
        }) {
            return Err(fail());
        }
        citation.pdf_target = Some(target.clone());
    }
    wenlan_core::pdf_citations::validate_pdf_targets(&citations, scope).await?;
    *citations_json =
        serde_json::to_string(&citations).map_err(|e| ServerError::Internal(e.to_string()))?;
    Ok(())
}

pub async fn handle_get_pdf(
    State(state): State<SharedState>,
    crate::space_header::SpaceHeader(header_space): crate::space_header::SpaceHeader,
    Query(query): Query<PdfQuery>,
) -> Result<impl IntoResponse, ServerError> {
    let document_query = ZoteroDocumentQuery {
        source_id: query.source_id.clone(),
        space: query.space.clone(),
        ..Default::default()
    };
    let (sidecar, selected_space) =
        authorized_document(&state, &document_query, header_space.as_deref()).await?;
    let (current, historical) = selected_pdf_generation(
        &sidecar,
        query
            .expected_digest
            .as_ref()
            .map(|_| query.attachment_key.as_str()),
        query.expected_digest.as_deref(),
        query.allow_retained,
    )?;
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
    headers.insert(
        "x-wenlan-pdf-digest",
        HeaderValue::from_str(hash).map_err(|e| ServerError::Internal(e.to_string()))?,
    );
    headers.insert(
        "x-wenlan-pdf-generation",
        HeaderValue::from_static(if historical { "retained" } else { "current" }),
    );
    Ok((headers, bytes))
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PdfQuery {
    #[serde(default)]
    allow_retained: bool,
    #[serde(default)]
    expected_digest: Option<String>,
    source_id: String,
    attachment_key: String,
    space: Option<String>,
}

/// Authorization always comes from the live exact source in `authorized_document`.
/// A hash selects retained bytes only when immutable metadata binds it to that
/// same source, Space, server, item and attachment; the cache itself grants no access.
fn selected_pdf_generation<'a>(
    history: &'a ZoteroSidecarHistory,
    attachment_key: Option<&str>,
    expected_digest: Option<&str>,
    allow_retained: bool,
) -> Result<(&'a ZoteroDocumentSidecar, bool), ServerError> {
    let current = history
        .current
        .as_ref()
        .ok_or_else(|| ServerError::NotFound("Zotero document not found".into()))?;
    if expected_digest.is_none() {
        if allow_retained || attachment_key.is_some() {
            return Err(ServerError::BadRequest(
                "retained PDF selection requires an exact attachment and digest".into(),
            ));
        }
        return Ok((current, false));
    }
    let expected = expected_digest.unwrap();
    let key = attachment_key
        .filter(|key| !key.is_empty())
        .ok_or_else(|| {
            ServerError::BadRequest("expected PDF digest requires an attachment key".into())
        })?;
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ServerError::BadRequest(
            "invalid expected PDF digest".into(),
        ));
    }
    let matches = |generation: &ZoteroDocumentSidecar| {
        generation
            .attachments
            .iter()
            .any(|a| a.key == key && a.cached_pdf_hash.as_deref() == Some(expected))
    };
    if matches(current) {
        return Ok((current, false));
    }
    if allow_retained {
        if let Some(retained) = history.snapshots.iter().rev().find(|old| {
            old.source_id == current.source_id
                && old.space == current.space
                && old.server_id == current.server_id
                && old.item_key == current.item_key
                && matches(old)
        }) {
            return Ok((retained, true));
        }
    }
    Err(ServerError::Conflict("zotero_pdf_stale: requested PDF generation is not available for this source and attachment; reload or retain the original citation".into()))
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

fn fence_registration(source: &Source) -> Result<(), ServerError> {
    config::update_config(|config| {
        super::require_unambiguous_source(config, &source.id)?;
        if !config.sources.iter().any(|s| {
            s.id == source.id
                && s.path == source.path
                && s.space == source.space
                && s.source_type == SourceType::Zotero
        }) {
            return Err(wenlan_core::WenlanError::Conflict(
                "Zotero registration changed; reload Sources".into(),
            ));
        }
        Ok(())
    })?;
    Ok(())
}

async fn registration_identity(
    db: &wenlan_core::db::MemoryDB,
    source: &Source,
) -> Result<ZoteroRegistrationIdentity, ServerError> {
    let identity = disconnect_identity(db, source).await?;
    if identity.server_id.is_empty() {
        return Err(ServerError::ValidationError(
            "Reconnect this legacy Zotero source to bind a stable server identity before syncing"
                .into(),
        ));
    }
    Ok(identity)
}

async fn disconnect_identity(
    db: &wenlan_core::db::MemoryDB,
    source: &Source,
) -> Result<ZoteroRegistrationIdentity, ServerError> {
    let selected = parse_locator(&source.path)?;
    let server_id = selected.server_id.unwrap_or_default();
    let space = source
        .space
        .as_deref()
        .ok_or_else(|| ServerError::ValidationError("Zotero destination Space missing".into()))?;
    let resolved = db
        .resolve_write_space(&WriteSpaceTarget::Named(space.to_owned()), None)
        .await?;
    Ok(ZoteroRegistrationIdentity {
        registration_id: source.id.clone(),
        locator: source
            .path
            .to_str()
            .ok_or_else(|| ServerError::ValidationError("Zotero locator is not UTF-8".into()))?
            .into(),
        server_id,
        space_id: resolved.space_id.ok_or_else(|| {
            ServerError::ValidationError("Zotero destination Space missing".into())
        })?,
    })
}

pub async fn handle_disconnect_preview(
    State(state): State<SharedState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<ZoteroDisconnectPreview>, ServerError> {
    let _lane = ZOTERO_MUTATION_LANE.lock().await;
    let db = {
        state
            .read()
            .await
            .db
            .clone()
            .ok_or(ServerError::DbNotInitialized)?
    };
    let config = config::load_config();
    super::require_unambiguous_source(&config, &id)?;
    let identity = if let Some(source) = config
        .sources
        .iter()
        .find(|s| s.id == id && s.source_type == SourceType::Zotero)
    {
        disconnect_identity(&db, source).await?
    } else {
        db.zotero_retained_registration(&id)
            .await?
            .ok_or_else(|| ServerError::NotFound("Zotero registration not found".into()))?
    };
    Ok(Json(db.zotero_disconnect_preview(&identity).await?))
}

pub(super) async fn disconnect_source(
    db: Arc<wenlan_core::db::MemoryDB>,
    id: &str,
    expected: Option<(u64, &str)>,
) -> Result<(), ServerError> {
    let _lane = ZOTERO_MUTATION_LANE.lock().await;
    let config = config::load_config();
    super::require_unambiguous_source(&config, id)?;
    let source = config.sources.iter().find(|s| s.id == id).cloned();
    if source
        .as_ref()
        .is_some_and(|s| s.source_type != SourceType::Zotero)
    {
        return Err(ServerError::Conflict(
            "Zotero disconnect cannot remove a registration whose source type changed".into(),
        ));
    }
    let identity = if let Some(source) = &source {
        disconnect_identity(&db, source).await?
    } else {
        db.zotero_retained_registration(id)
            .await?
            .ok_or_else(|| ServerError::NotFound("Zotero registration not found".into()))?
    };
    if let Some(expected) = expected {
        let current = db.zotero_disconnect_preview(&identity).await?;
        if expected.0 != current.revision || expected.1 != current.identity_digest {
            return Err(ServerError::Conflict(
                "Zotero disconnect impact changed; reload its preview before disconnecting".into(),
            ));
        }
    }
    // Legacy/new unsynchronized registrations start with an empty positive-import baseline.
    // Existing pending intents must remain retryable, so bind only when not pending/done.
    let prior = db.zotero_disconnect_preview(&identity).await?;
    if !prior.pending_disconnect && !prior.disconnected {
        db.bind_zotero_registration_baseline(&identity).await?;
    }
    db.begin_zotero_disconnect(&identity).await?;
    config::update_config(|config| {
        super::require_unambiguous_source(config, id)?;
        if let Some(stored) = config.sources.iter().find(|s| s.id == id) {
            if source.as_ref().is_none_or(|old| {
                stored.path != old.path
                    || stored.space != old.space
                    || stored.source_type != SourceType::Zotero
            }) {
                return Err(wenlan_core::WenlanError::Conflict(
                    "Zotero disconnect pending: registration changed; reload Sources".into(),
                ));
            }
        }
        config.sources.retain(|s| s.id != id);
        Ok(())
    })
    .map_err(|e| {
        ServerError::Internal(format!("Zotero disconnect pending; retry disconnect: {e}"))
    })?;
    db.finish_zotero_disconnect(&identity).await.map_err(|e| {
        ServerError::Internal(format!(
            "Zotero sync disabled; impact finalization pending; retry disconnect: {e}"
        ))
    })?;
    Ok(())
}

pub(crate) async fn sync_source(
    db: Arc<wenlan_core::db::MemoryDB>,
    source: &Source,
) -> Result<SyncStatsResponse, ServerError> {
    let expected_server_id = parse_locator(&source.path)?.server_id;
    let mut client = ZoteroClient::with_server_id(expected_server_id)
        .map_err(|e| ServerError::Upstream(e.to_string()))?;
    sync_source_with_client(db, source, &mut client).await
}

async fn sync_source_with_client<C: ZoteroApi>(
    db: Arc<wenlan_core::db::MemoryDB>,
    source: &Source,
    client: &mut C,
) -> Result<SyncStatsResponse, ServerError> {
    let _sync_guard = ZOTERO_MUTATION_LANE.lock().await;
    fence_registration(source)?;
    let identity = registration_identity(&db, source).await?;
    db.bind_zotero_registration_baseline(&identity).await?;
    let selection = parse_locator(&source.path)?;
    let expected_server_id = selection.server_id;
    let collection_key = selection.collection_key;
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
    let collection_display_name = collection_page
        .items
        .iter()
        .find(|collection| collection.key == collection_key)
        .expect("complete collection listing checked above")
        .data
        .name
        .clone();
    let mut item_page = client
        .collection_items(&collection_key)
        .await
        .map_err(|e| ServerError::Upstream(e.to_string()))?;
    if !item_page.complete
        || expected_server_id
            .as_deref()
            .is_some_and(|id| client.server_id() != Some(id))
    {
        return Err(ServerError::Upstream(
            "Zotero item listing was incomplete; no documents were changed".into(),
        ));
    }

    let mut listed = std::collections::BTreeSet::new();
    for item in item_page
        .items
        .iter()
        .filter(|item| is_selectable_item(item))
    {
        validate_listed_item(item, &collection_key)?;
        if !listed.insert(item.key.clone()) {
            return Err(ServerError::Upstream(
                "Zotero item listing contained duplicate identities".into(),
            ));
        }
    }
    fence_registration(source)?;
    db.observe_zotero_selection(&identity, &listed, &std::collections::BTreeMap::new())
        .await?;

    let missing_selected: Vec<String> = selection
        .item_keys
        .as_ref()
        .map(|keys| {
            keys.iter()
                .filter(|key| {
                    !item_page
                        .items
                        .iter()
                        .any(|item| &item.key == *key && is_selectable_item(item))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if let Some(keys) = &selection.item_keys {
        item_page
            .items
            .retain(|item| keys.binary_search(&item.key).is_ok() && is_selectable_item(item));
    }
    let mut documents = Vec::new();
    let mut sync_receipts = Vec::new();
    let mut sidecars = Vec::new();
    let mut skipped_files = Vec::new();
    let mut errors = 0usize;
    let mut seen = HashSet::new();
    for key in &missing_selected {
        errors += 1;
        add_skipped(&mut skipped_files,key,key,"Selected Zotero item is missing from the collection; kept any prior document and human classification");
    }
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
    for key in &missing_selected {
        let source_id =
            canonical_source_id(client.server_id(), key, write_space.space_id.as_deref());
        if db
            .get_chunks_scoped(&source_id, &write_scope)
            .await?
            .is_some_and(|chunks| !chunks.is_empty())
        {
            indexed_source_ids.insert(source_id);
        }
    }
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
                let prepared_annotations = prepared.annotations;
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
                research.import_registration = Some(ZoteroImportRegistration {
                    id: source.id.clone(),
                    locator: source.path.to_string_lossy().into_owned(),
                });
                research.annotations = prepared_annotations;
                let mut cached_pdfs = Vec::new();
                let mut fresh_pdf_keys = HashSet::new();
                for attachment in prepared_attachments {
                    if attachment.pdf_bytes.is_some() {
                        fresh_pdf_keys.insert(attachment.key.clone());
                    }
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
                research
                    .annotations
                    .retain(|annotation| fresh_pdf_keys.contains(&annotation.attachment_key));
                if let Some(previous) = previous {
                    // Failed PDF retrieval retains its exact old bytes and old annotations together.
                    // New annotation coordinates must never be drawn over an unknown older PDF.
                    research.annotations.extend(
                        previous
                            .annotations
                            .iter()
                            .filter(|annotation| {
                                !fresh_pdf_keys.contains(&annotation.attachment_key)
                                    && research
                                        .attachments
                                        .iter()
                                        .any(|a| a.key == annotation.attachment_key)
                            })
                            .cloned(),
                    );
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
                        sidecars.push((source_id, research, cached_pdfs, hash.clone()));
                        sync_receipts.push((prepared.key.clone(), hash.clone()));
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
                sidecars.push((source_id.clone(), research, cached_pdfs, hash.clone()));
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
    let files_found = selection.item_keys.as_ref().map_or_else(
        || {
            item_page
                .items
                .iter()
                .filter(|item| {
                    item.data.item_type != "attachment" || item.data.parent_item.is_none()
                })
                .count()
        },
        Vec::len,
    );
    let changed_count = documents.len();
    let changed_source_ids = documents
        .iter()
        .map(|(document, _)| document.source_id.clone())
        .collect::<Vec<_>>();
    fence_registration(source)?;
    if registration_identity(&db, source).await? != identity
        || client.server_id() != Some(identity.server_id.as_str())
    {
        return Err(ServerError::ValidationError(
            "Zotero registration/server/Space changed during sync".into(),
        ));
    }
    if changed_count > 0 {
        db.upsert_documents_with_write_spaces(documents).await?;
        indexed_source_ids.extend(changed_source_ids);
    }
    let ingested = changed_count;
    // Record only successfully persisted documents. An interrupted or failed
    // fetch never marks an item as complete, so the next manual sync retries.
    for (source_id, sidecar, cached_pdfs, import_content_hash) in sidecars {
        let bibliography = write_space.space_id.clone().map(|space_id| {
            wenlan_types::page_export::PublishExportBibliography {
                space_id,
                source_id: source_id.clone(),
                import_content_hash,
                title: sidecar.title.clone(),
                bibtex: sidecar.bibtex.clone(),
                url: sidecar.bibliography.get("url").cloned(),
            }
        });
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
        } else if let Some(bibliography) = bibliography {
            if let Err(error) = db.publish_export_bibliography(bibliography).await {
                errors += 1;
                add_skipped(
                    &mut skipped_files,
                    &item_title,
                    &item_key,
                    &error.to_string(),
                );
            }
        }
    }
    let mut imported = std::collections::BTreeMap::new();
    for (key, hash) in sync_receipts {
        db.upsert_sync_state(&source.id, &key, 0, &hash).await?;
        imported.insert(
            key.clone(),
            canonical_source_id(Some(&identity.server_id), &key, Some(&identity.space_id)),
        );
    }
    fence_registration(source)?;
    db.observe_zotero_selection(&identity, &listed, &imported)
        .await?;

    if registration_identity(&db, source).await? != identity
        || client.server_id() != Some(identity.server_id.as_str())
    {
        return Err(ServerError::Conflict(
            "Zotero registration changed; reload Sources".into(),
        ));
    }
    let skipped = files_found.saturating_sub(ingested);
    let detail = (errors > 0).then(|| "zotero_item_errors".to_string());
    config::update_config(|config| {
        super::require_unambiguous_source(config, &source.id)?;
        let stored = config
            .sources
            .iter_mut()
            .find(|stored| {
                stored.id == source.id
                    && stored.source_type == SourceType::Zotero
                    && stored.path == source.path
                    && stored.space == source.space
            })
            .ok_or_else(|| {
                wenlan_core::WenlanError::Conflict(
                    "Zotero registration changed; reload Sources".into(),
                )
            })?;
        stored.last_sync = Some(chrono::Utc::now().timestamp());
        stored.file_count = files_found as u64;
        stored.memory_count = indexed_source_ids.len() as u64;
        stored.last_sync_errors = errors as u64;
        stored.last_sync_error_detail = detail.clone();
        stored.status = SyncStatus::Paused;
        if errors == 0 {
            stored.display_name = Some(collection_display_name.clone());
        }
        Ok(())
    })?;
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

async fn attachment_annotations<C: ZoteroApi>(
    client: &mut C,
    key: &str,
) -> anyhow::Result<Vec<ZoteroItem>> {
    let listed = client.attachment_annotations(key).await?;
    if !listed.complete {
        anyhow::bail!("Zotero annotation listing was incomplete");
    }
    if listed.items.iter().any(|item| {
        item.data.item_type != "annotation" || item.data.parent_item.as_deref() != Some(key)
    }) {
        anyhow::bail!("Zotero annotation listing contained an unexpected item or parent");
    }
    Ok(listed.items)
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
        all_children.extend(attachment_annotations(client, &item.key).await?);
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
            all_children.extend(attachment_annotations(client, &child.key).await?);
        }
    }
    let annotations = read_annotations(&all_children, &attachments);
    let fulltext = combine_fulltexts(fulltexts);
    let mut prepared = zotero::prepare_document(item, &all_children, &fulltext);
    if all_children
        .iter()
        .filter(|item| item.data.item_type == "annotation")
        .count()
        > annotations.len()
    {
        prepared.warnings.push("Some Zotero annotations have unsupported or invalid shapes and are not displayed on the PDF".into());
    }
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
        annotations,
    })
}

struct PreparedZoteroDocument {
    document: zotero::PreparedDocument,
    attachment_metadata: std::collections::HashMap<String, String>,
    attachments: Vec<PreparedAttachment>,
    transient_fulltext_error: bool,
    annotations: Vec<ZoteroPdfAnnotation>,
}

struct PreparedAttachment {
    key: String,
    title: String,
    content_type: Option<String>,
    pdf_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ZoteroDocumentSidecar {
    source_id: String,
    space: Option<String>,
    server_id: Option<String>,
    item_key: String,
    title: String,
    rendered_content: String,
    bibliography: HashMap<String, String>,
    attachments: Vec<ZoteroAttachmentSidecar>,
    #[serde(default)]
    annotations: Vec<ZoteroPdfAnnotation>,
    #[serde(default)]
    import_registration: Option<ZoteroImportRegistration>,
    bibtex: String,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ZoteroImportRegistration {
    id: String,
    locator: String,
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

fn safe_public_url(value: &str) -> Option<String> {
    let url = reqwest::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    match url.host()? {
        url::Host::Domain(host) => {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            if !host.contains('.')
                || [
                    ".localhost",
                    ".local",
                    ".internal",
                    ".lan",
                    ".home",
                    ".test",
                    ".invalid",
                ]
                .iter()
                .any(|suffix| host.ends_with(suffix))
            {
                return None;
            }
        }
        url::Host::Ipv4(ip) => {
            let o = ip.octets();
            if ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || o[0] == 0
                || o[0] >= 240
                || (o[0] == 100 && (64..=127).contains(&o[1]))
            {
                return None;
            }
        }
        url::Host::Ipv6(ip) => {
            let o = ip.octets();
            if ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (o[0] & 0xfe == 0xfc)
                || (o[0] == 0xfe && o[1] & 0xc0 == 0x80)
                || ip.to_ipv4_mapped().is_some()
            {
                return None;
            }
        }
    }
    let sensitive = |key: &str| {
        let key = key.to_ascii_lowercase().replace('-', "_");
        matches!(
            key.as_str(),
            "auth"
                | "authorization"
                | "password"
                | "passwd"
                | "secret"
                | "api_key"
                | "apikey"
                | "sig"
                | "signature"
                | "x_amz_credential"
                | "x_goog_credential"
                | "x_amz_signature"
                | "x_goog_signature"
        ) || key == "token"
            || key.ends_with("_token")
    };
    if url.query_pairs().any(|(key, _)| sensitive(&key))
        || url.fragment().is_some_and(|fragment| {
            url::form_urlencoded::parse(fragment.as_bytes()).any(|(key, _)| sensitive(&key))
        })
    {
        return None;
    }
    Some(url.to_string())
}

fn read_annotations(
    children: &[ZoteroItem],
    attachments: &[PreparedAttachment],
) -> Vec<ZoteroPdfAnnotation> {
    children
        .iter()
        .filter_map(|item| {
            let data = &item.data;
            if data.item_type != "annotation"
                || data.extra.get("annotationType")?.as_str()? != "highlight"
            {
                return None;
            }
            let parent = data.parent_item.as_ref()?;
            if !attachments
                .iter()
                .any(|a| &a.key == parent && a.content_type.as_deref() == Some("application/pdf"))
            {
                return None;
            }
            if item.key.len() != 8
                || !item
                    .key
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            {
                return None;
            }
            let position = data.annotation_position.as_ref()?;
            let page_index = u32::try_from(position.get("pageIndex")?.as_u64()?).ok()?;
            let raw = position.get("rects")?.as_array()?;
            if raw.is_empty() || raw.len() > 256 {
                return None;
            }
            let mut rects = Vec::new();
            for rect in raw {
                let values = rect.as_array()?;
                if values.len() != 4 {
                    return None;
                }
                let rect = [
                    values[0].as_f64()?,
                    values[1].as_f64()?,
                    values[2].as_f64()?,
                    values[3].as_f64()?,
                ];
                if !rect.iter().all(|n| n.is_finite() && n.abs() <= 1_000_000.0)
                    || rect[0] >= rect[2]
                    || rect[1] >= rect[3]
                {
                    return None;
                }
                rects.push(rect);
            }
            let color = data.extra.get("annotationColor")?.as_str()?;
            if color.len() != 7
                || !color.starts_with('#')
                || !color[1..].bytes().all(|b| b.is_ascii_hexdigit())
            {
                return None;
            }
            if data.annotation_text.len() > 64_000 || data.annotation_comment.len() > 64_000 {
                return None;
            }
            Some(ZoteroPdfAnnotation {
                key: item.key.clone(),
                attachment_key: parent.clone(),
                page_index,
                rects,
                text: data.annotation_text.clone(),
                comment: data.annotation_comment.clone(),
                color: color.to_owned(),
            })
        })
        .take(5000)
        .collect()
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
    let bibliography = research_metadata("", document, extra_metadata.clone(), None, None, item);
    hasher.update(bibliography.title.as_bytes());
    hasher.update([0xfb]);
    hasher.update(bibliography.bibtex.as_bytes());
    hasher.update([0xfc]);
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
    if !item.data.url.is_empty() {
        bibliography.insert("url".into(), item.data.url.clone());
    }
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
        annotations: Vec::new(),
        import_registration: None,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScopedZoteroLocator {
    server_id: Option<String>,
    collection_key: String,
    item_keys: Option<Vec<String>>,
}

fn scoped_locator(
    server_id: &Option<String>,
    collection_key: &str,
    keys: Option<&[String]>,
) -> String {
    let base = locator(server_id, collection_key);
    match keys {
        Some(keys) => format!("{base}/items/{}", keys.join(",")),
        None => base,
    }
}

fn parse_locator(path: &std::path::Path) -> Result<ScopedZoteroLocator, ServerError> {
    let text = path
        .to_str()
        .ok_or_else(|| ServerError::ValidationError("Zotero source locator is not UTF-8".into()))?;
    let parts: Vec<_> = text.split('/').collect();
    if !matches!(parts.len(), 6 | 8)
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
        Some(unhex(parts[1]).filter(|id| !id.is_empty()).ok_or_else(|| {
            ServerError::ValidationError("Zotero source identity is malformed".into())
        })?)
    };
    let item_keys = if parts.len() == 8 {
        if parts[6] != "items" {
            return Err(ServerError::ValidationError(
                "Zotero source selection is malformed".into(),
            ));
        }
        let raw: Vec<String> = parts[7].split(',').map(str::to_owned).collect();
        let canonical = canonical_item_keys(&raw)?;
        if canonical != raw {
            return Err(ServerError::ValidationError(
                "Zotero source selection is not canonical sorted unique keys".into(),
            ));
        }
        Some(canonical)
    } else {
        None
    };
    Ok(ScopedZoteroLocator {
        server_id,
        collection_key: parts[5].to_owned(),
        item_keys,
    })
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
        children_items: Vec<ZoteroItem>,
        annotation_items: Vec<ZoteroItem>,
        incomplete_items: bool,
        incomplete_collections: bool,
        replace_source_after_items: Option<Source>,
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
            let incomplete = self.incomplete_collections;
            async move {
                let mut listed = page(vec![collection]);
                listed.complete = !incomplete;
                Ok(listed)
            }
        }

        fn collection_items(
            &mut self,
            key: &str,
        ) -> impl Future<Output = anyhow::Result<zotero::ZoteroPageSet<ZoteroItem>>> + Send
        {
            if let Some(replacement) = self.replace_source_after_items.take() {
                config::update_config(|config| {
                    *config
                        .sources
                        .iter_mut()
                        .find(|source| source.id == replacement.id)
                        .unwrap() = replacement.clone();
                    Ok(())
                })
                .unwrap();
            }
            let valid = key == self.collection.key;
            let items = self.items.clone();
            let incomplete = self.incomplete_items;
            async move {
                if !valid {
                    anyhow::bail!("unexpected collection key")
                }
                let mut listed = page(items);
                listed.complete = !incomplete;
                Ok(listed)
            }
        }

        fn children(
            &mut self,
            key: &str,
        ) -> impl Future<Output = anyhow::Result<zotero::ZoteroPageSet<ZoteroItem>>> + Send
        {
            let fail = self.fail_children_for.contains(key);
            let items = self.children_items.clone();
            async move {
                if fail {
                    anyhow::bail!("fixture child request failed")
                }
                Ok(page(items))
            }
        }

        async fn attachment_annotations(
            &mut self,
            _key: &str,
        ) -> anyhow::Result<zotero::ZoteroPageSet<ZoteroItem>> {
            Ok(page(self.annotation_items.clone()))
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
            children_items: Vec::new(),
            annotation_items: Vec::new(),
            incomplete_items: false,
            incomplete_collections: false,
            replace_source_after_items: None,
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

    #[tokio::test]
    async fn prepare_item_ingests_attachment_scoped_string_position_annotation() {
        let attachment: ZoteroItem = serde_json::from_str(r#"{"key":"YXNSB3HI","version":1,"data":{"itemType":"attachment","title":"FiD","contentType":"application/pdf"}}"#).unwrap();
        // The identity, color and position are from the real Zotero 10.0.7 response.
        let annotation: ZoteroItem = serde_json::from_str(r##"{"key":"P78LVH7C","version":1,"data":{"itemType":"annotation","parentItem":"YXNSB3HI","annotationType":"highlight","annotationColor":"#ffd400","annotationPosition":"{\"pageIndex\":0,\"rects\":[[436.949,374.142,523.056,382.78]]}"}}"##).unwrap();
        let mut client = fake(vec![], &[]);
        client.children_items = vec![attachment];
        client.annotation_items = vec![annotation.clone()];
        let prepared = prepare_item(&mut client, &article("V4ZWXW4G", "FiD", ""))
            .await
            .unwrap();
        assert_eq!(prepared.annotations.len(), 1);
        assert_eq!(prepared.annotations[0].key, "P78LVH7C");
        assert_eq!(prepared.annotations[0].attachment_key, "YXNSB3HI");
        assert_eq!(prepared.annotations[0].page_index, 0);
        assert_eq!(
            prepared.annotations[0].rects[0],
            [436.949, 374.142, 523.056, 382.78]
        );
        client.annotation_items[0].data.parent_item = Some("FOREIGN1".into());
        assert!(prepare_item(&mut client, &article("V4ZWXW4G", "FiD", ""))
            .await
            .is_err());
    }

    fn source() -> Source {
        Source {
            display_name: None,
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
    fn public_source_url_preserves_identifiers_but_rejects_local_and_credentials() {
        assert_eq!(
            safe_public_url("https://arxiv.org/pdf?id=1234#page=2"),
            Some("https://arxiv.org/pdf?id=1234#page=2".into())
        );
        for value in [
            "file:///tmp/paper.pdf",
            "http://127.0.0.1:17889/api/zotero/pdf",
            "http://[::1]/paper",
            "https://user:pass@example.org/paper",
            "https://host.local/paper",
            "https://example.org/paper?access_token=x",
            "https://example.org/paper#access_token=x",
            "https://example.org/paper?X-Amz-Signature=x",
        ] {
            assert!(safe_public_url(value).is_none(), "{value}");
        }
    }

    #[test]
    fn annotation_rectangles_come_only_from_authoritative_supported_metadata() {
        let attachment = PreparedAttachment {
            key: "ATTACH01".into(),
            title: "Paper".into(),
            content_type: Some("application/pdf".into()),
            pdf_bytes: None,
        };
        let raw = serde_json::json!({"key":"ANN00001","version":1,"data":{
            "itemType":"annotation","parentItem":"ATTACH01","annotationType":"highlight",
            "annotationText":"Original text","annotationComment":"Human comment","annotationColor":"#ffdd00",
            "annotationPosition":{"pageIndex":2,"rects":[[10,20,100,40]]}
        }});
        let item: ZoteroItem = serde_json::from_value(raw.clone()).unwrap();
        let annotations = read_annotations(&[item], &[attachment]);
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].page_index, 2);
        assert_eq!(annotations[0].rects, vec![[10.0, 20.0, 100.0, 40.0]]);
        for (field, value) in [
            ("parentItem", serde_json::json!("FOREIGN1")),
            ("annotationType", serde_json::json!("ink")),
            ("annotationColor", serde_json::json!("red")),
            (
                "annotationPosition",
                serde_json::json!({"pageIndex":2,"rects":[[10,20,10,40]]}),
            ),
        ] {
            let mut bad = raw.clone();
            bad["data"][field] = value;
            let attachment = PreparedAttachment {
                key: "ATTACH01".into(),
                title: String::new(),
                content_type: Some("application/pdf".into()),
                pdf_bytes: None,
            };
            assert!(
                read_annotations(&[serde_json::from_value(bad).unwrap()], &[attachment]).is_empty()
            );
        }
    }

    #[test]
    fn selected_locator_is_strict_canonical_and_legacy_compatible() {
        let keys = vec!["ITEM0001".into(), "ITEM0003".into()];
        let path = scoped_locator(&Some("server/identity".into()), "COLLECT1", Some(&keys));
        let parsed = parse_locator(std::path::Path::new(&path)).unwrap();
        assert_eq!(parsed.server_id, Some("server/identity".into()));
        assert_eq!(parsed.item_keys, Some(keys.clone()));
        assert!(
            parse_locator(std::path::Path::new(&locator(&None, "COLLECT1")))
                .unwrap()
                .item_keys
                .is_none()
        );
        for suffix in [
            "/items/",
            "/items/ITEM0001,ITEM0001",
            "/items/ITEM0003,ITEM0001",
            "/items/item0001",
            "/items/../ITEM0001",
            "/other/ITEM0001",
            "/items/ITEM0001/",
        ] {
            assert!(
                parse_locator(std::path::Path::new(&format!(
                    "{}{suffix}",
                    locator(&None, "COLLECT1")
                )))
                .is_err(),
                "{suffix}"
            );
        }
        assert_eq!(
            canonical_item_keys(&["ITEM0003".into(), "ITEM0001".into(), "ITEM0003".into()])
                .unwrap(),
            keys
        );
        assert!(canonical_item_keys(&[]).is_err());
        let legacy: AddZoteroSourceRequest = serde_json::from_value(serde_json::json!({"collection_key":"COLLECT1","expected_server_id":"server","space":"Research"})).unwrap();
        assert!(legacy.item_keys.is_none());
    }

    #[tokio::test]
    async fn article_listing_and_registration_selection_reject_foreign_partial_and_changed_server()
    {
        let mut nested = article("ITEM0003", "Child", "Text");
        nested.data.parent_item = Some("ITEM0001".into());
        let mut note = article("ITEM0004", "Note", "Text");
        note.data.item_type = "note".into();
        let mut client = fake(
            vec![
                article("ITEM0002", "Second", "Text"),
                article("ITEM0001", "First", "Text"),
                nested,
                note,
            ],
            &[],
        );
        let listed = list_selectable_items(&mut client, "COLLECT1", "test-server")
            .await
            .unwrap();
        assert!(listed.complete);
        assert_eq!(
            listed
                .items
                .iter()
                .map(|i| i.key.as_str())
                .collect::<Vec<_>>(),
            vec!["ITEM0001", "ITEM0002"]
        );
        assert!(
            validate_selection(&mut client, "COLLECT1", "test-server", &["FOREIGN1".into()])
                .await
                .is_err()
        );
        assert!(
            list_selectable_items(&mut client, "FOREIGN1", "test-server")
                .await
                .is_err()
        );
        assert!(
            list_selectable_items(&mut client, "COLLECT1", "other-server")
                .await
                .is_err()
        );
        client.incomplete_items = true;
        assert!(
            list_selectable_items(&mut client, "COLLECT1", "test-server")
                .await
                .is_err()
        );
        assert!(
            validate_selection(&mut client, "COLLECT1", "test-server", &["ITEM0001".into()])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn verified_sync_refreshes_collection_display_name_without_changing_identity() {
        let _lock = data_dir_lock().await;
        let _data_root = super::super::tests::DataDirGuard::new();
        let (db, _db_dir) = super::super::tests::new_test_db().await;
        db.create_space("Research", None, false).await.unwrap();
        let source = source();
        assert!(
            source.display_name.is_none(),
            "legacy registrations have no label"
        );
        config::save_config(&config::Config {
            sources: vec![source.clone()],
            ..config::Config::default()
        })
        .unwrap();
        let mut client = fake(vec![article("ITEM0001", "Paper", "Text")], &[]);
        client.collection.data.name = "Reading · 方法".into();
        let result = sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .unwrap();
        assert_eq!(result.errors, 0);
        let stored = config::load_config().sources.remove(0);
        assert_eq!(stored.display_name.as_deref(), Some("Reading · 方法"));
        assert_eq!(
            (&stored.id, &stored.path, &stored.space),
            (&source.id, &source.path, &source.space)
        );
        client.collection.data.name = "Renamed collection".into();
        sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .unwrap();
        assert_eq!(
            config::load_config().sources[0].display_name.as_deref(),
            Some("Renamed collection")
        );

        client.collection.data.name = "Unverified replacement".into();
        client.incomplete_collections = true;
        assert!(sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .is_err());
        client.incomplete_collections = false;
        client.incomplete_items = true;
        assert!(sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .is_err());
        client.incomplete_items = false;
        client.identity = Some("other-server".into());
        assert!(sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .is_err());
        client.identity = Some("test-server".into());
        assert_eq!(
            config::load_config().sources[0].display_name.as_deref(),
            Some("Renamed collection")
        );

        client.fail_children_for.insert("ITEM0001".into());
        let failed = sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .unwrap();
        assert!(failed.errors > 0);
        assert_eq!(
            config::load_config().sources[0].display_name.as_deref(),
            Some("Renamed collection")
        );
        client.fail_children_for.clear();

        let mut replacement = stored;
        replacement.path = PathBuf::from(locator(&Some("different-server".into()), "COLLECT1"));
        replacement.display_name = Some("New connection label".into());
        client.replace_source_after_items = Some(replacement);
        assert!(sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .is_err());
        assert_eq!(
            config::load_config().sources[0].display_name.as_deref(),
            Some("New connection label")
        );
    }

    #[tokio::test]
    async fn selected_sync_filters_preserves_missing_human_tags_and_overlaps_without_duplicate_identity(
    ) {
        let _lock = data_dir_lock().await;
        let _data_root = super::super::tests::DataDirGuard::new();
        let (db, _db_dir) = super::super::tests::new_test_db().await;
        db.create_space("Research", None, false).await.unwrap();
        let mut first = source();
        first.path = PathBuf::from(scoped_locator(
            &Some("test-server".into()),
            "COLLECT1",
            Some(&["ITEM0001".into()]),
        ));
        config::save_config(&config::Config {
            sources: vec![first.clone()],
            ..config::Config::default()
        })
        .unwrap();
        let mut client = fake(
            vec![
                article("ITEM0001", "First paper", "Original abstract"),
                article("ITEM0002", "Second paper", "Second abstract"),
            ],
            &[],
        );
        let result = sync_source_with_client(db.clone(), &first, &mut client)
            .await
            .unwrap();
        assert_eq!(
            (result.files_found, result.ingested, result.errors),
            (1, 1, 0)
        );
        let space_id = db.get_space("Research").await.unwrap().unwrap().id;
        let first_id = canonical_source_id(Some("test-server"), "ITEM0001", Some(&space_id));
        let second_id = canonical_source_id(Some("test-server"), "ITEM0002", Some(&space_id));
        let scope = ReadScope::Space("Research".into());
        assert!(db
            .get_chunks_scoped(&second_id, &scope)
            .await
            .unwrap()
            .is_none());
        let original = db
            .get_chunks_scoped(&first_id, &scope)
            .await
            .unwrap()
            .unwrap();
        db.set_document_tags("file", &first_id, vec!["Human choice".into()])
            .await
            .unwrap();
        client.items.retain(|item| item.key != "ITEM0001");
        let missing = sync_source_with_client(db.clone(), &first, &mut client)
            .await
            .unwrap();
        assert_eq!(
            (missing.files_found, missing.ingested, missing.errors),
            (1, 0, 1)
        );
        assert!(missing
            .skipped_files
            .iter()
            .any(|item| item.reason.contains("Selected Zotero item is missing")));
        assert_eq!(config::load_config().sources[0].memory_count, 1);
        assert_eq!(
            db.get_document_tags("file", &first_id).await.unwrap(),
            vec!["human choice"]
        );
        assert_eq!(
            db.get_chunks_scoped(&first_id, &scope)
                .await
                .unwrap()
                .unwrap()[0]
                .content,
            original[0].content
        );
        client
            .items
            .push(article("ITEM0001", "First paper", "Original abstract"));
        let mut overlapping = first.clone();
        overlapping.id = "second-selection".into();
        overlapping.path = PathBuf::from(scoped_locator(
            &Some("test-server".into()),
            "COLLECT1",
            Some(&["ITEM0001".into(), "ITEM0002".into()]),
        ));
        config::update_config(|config| {
            config.sources.push(overlapping.clone());
            Ok(())
        })
        .unwrap();
        let overlap = sync_source_with_client(db.clone(), &overlapping, &mut client)
            .await
            .unwrap();
        assert_eq!(overlap.files_found, 2);
        assert!(db
            .get_chunks_scoped(&second_id, &scope)
            .await
            .unwrap()
            .is_some());
        assert_eq!(
            db.get_chunks_scoped(&first_id, &scope)
                .await
                .unwrap()
                .unwrap()
                .len(),
            original.len()
        );
        assert_eq!(
            db.get_document_tags("file", &first_id).await.unwrap(),
            vec!["human choice"]
        );
    }

    #[test]
    fn locator_round_trips_server_identity_and_legacy_source() {
        let path = PathBuf::from(locator(&Some("Zotero/7.0".into()), "AB12CD34"));
        assert_eq!(
            parse_locator(&path).unwrap(),
            ScopedZoteroLocator {
                server_id: Some("Zotero/7.0".into()),
                collection_key: "AB12CD34".into(),
                item_keys: None
            }
        );
        assert_eq!(
            parse_locator(&PathBuf::from(locator(&None, "AB12CD34"))).unwrap(),
            ScopedZoteroLocator {
                server_id: None,
                collection_key: "AB12CD34".into(),
                item_keys: None
            }
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
        let pdf = include_bytes!("../../../wenlan-core/tests/fixtures/folder/paper.pdf").to_vec();
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
                annotations: vec![],
                import_registration: None,
                attachments: vec![ZoteroAttachmentSidecar {
                    key: "ATTACH01".into(),
                    title: "paper.pdf".into(),
                    content_type: Some("application/pdf".into()),
                    cached_pdf_hash: Some(hash.clone()),
                }],
            },
            vec![(hash.clone(), pdf)],
        )
        .unwrap();
        let state = Arc::new(RwLock::new(ServerState {
            db: Some(db.clone()),
            ..ServerState::default()
        }));
        let doc_query = ZoteroDocumentQuery {
            source_id: source_id.clone(),
            space: Some("Research".into()),
            ..Default::default()
        };
        assert!(authorized_document(&state, &doc_query, None).await.is_ok());
        assert!(handle_get_pdf(
            State(state.clone()),
            crate::space_header::SpaceHeader(None),
            Query(PdfQuery {
                expected_digest: None,
                source_id: source_id.clone(),
                attachment_key: "ATTACH01".into(),
                space: Some("Research".into()),
                ..Default::default()
            })
        )
        .await
        .is_ok());
        let (_, Json(page)) = handle_get_pdf_page(
            State(state.clone()),
            crate::space_header::SpaceHeader(None),
            Query(PdfPageQuery {
                source_id: source_id.clone(),
                attachment_key: "ATTACH01".into(),
                space: Some("Research".into()),
                page: 1,
            }),
        )
        .await
        .unwrap();
        assert_eq!(page.pdf_digest, hash);
        assert_eq!(page.page, 1);
        assert_eq!(page.page_count, 1);
        assert!(page.extraction_complete && !page.text.trim().is_empty());
        let mut json = serde_json::json!([{"occurrence":1,"marker":1,"source_kind":"memory","locator":source_id,"score":0.0,"status":"unevaluated","scope":"sentence"}]).to_string();
        let input = wenlan_types::pages::PdfCitationInput {
            occurrence: 1,
            target: wenlan_types::pages::PdfCitationTarget {
                attachment_key: "ATTACH01".into(),
                pdf_digest: hash.clone(),
                page: 1,
                quote: page.text.trim().into(),
            },
        };
        let scope = ReadScope::Space("Research".into());
        attach_pdf_targets(
            &mut json,
            std::slice::from_ref(&input),
            std::slice::from_ref(&source_id),
            &scope,
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap()[0]["pdf_target"]["page"],
            1
        );
        let original = json.clone();
        assert!(attach_pdf_targets(
            &mut json,
            &[input.clone(), input],
            std::slice::from_ref(&source_id),
            &scope
        )
        .await
        .is_err());
        assert_eq!(
            json, original,
            "failed validation must not partially replace citation JSON"
        );
        let mut replaced = read_current_sidecar(&source_id).unwrap().unwrap();
        let replacement = b"%PDF-1.7 replacement bytes".to_vec();
        let replacement_hash = sha256_hex(&replacement);
        replaced.attachments[0].cached_pdf_hash = Some(replacement_hash.clone());
        write_research_metadata(
            &source_id,
            replaced,
            vec![(replacement_hash.clone(), replacement)],
        )
        .unwrap();
        assert!(
            matches!(
                handle_get_pdf(
                    State(state.clone()),
                    crate::space_header::SpaceHeader(None),
                    Query(PdfQuery {
                        source_id: source_id.clone(),
                        attachment_key: "ATTACH01".into(),
                        space: Some("Research".into()),
                        expected_digest: Some(hash.clone()),
                        ..Default::default()
                    })
                )
                .await,
                Err(ServerError::Conflict(_))
            ),
            "same attachment replaced after metadata read must refuse old expected hash"
        );
        let retained=handle_get_pdf(State(state.clone()),crate::space_header::SpaceHeader(None),Query(serde_json::from_value::<PdfQuery>(serde_json::json!({"source_id":source_id,"attachment_key":"ATTACH01","space":"Research","expected_digest":hash,"allow_retained":true})).unwrap())).await.expect("exact authorized retained PDF generation must remain readable").into_response();
        assert_eq!(
            retained.headers().get("x-wenlan-pdf-generation").unwrap(),
            "retained"
        );
        let retained_bytes = axum::body::to_bytes(retained.into_body(), 64 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(sha256_hex(&retained_bytes), hash);
        let (_,Json(retained_document))=handle_get_document(State(state.clone()),crate::space_header::SpaceHeader(None),Query(serde_json::from_value::<ZoteroDocumentQuery>(serde_json::json!({"source_id":source_id,"space":"Research","attachment_key":"ATTACH01","expected_digest":hash,"allow_retained":true})).unwrap())).await.unwrap();
        let retained_document = serde_json::to_value(retained_document).unwrap();
        assert_eq!(retained_document["historical"], true);
        assert_eq!(retained_document["attachments"][0]["pdf_digest"], hash);
        db.create_space("Other", None, false).await.unwrap();
        for (space, key, digest, expected_not_found) in [
            ("Other", "ATTACH01", hash.clone(), true),
            ("Research", "WRONGKEY", hash.clone(), false),
        ] {
            let result=handle_get_pdf(State(state.clone()),crate::space_header::SpaceHeader(None),Query(serde_json::from_value::<PdfQuery>(serde_json::json!({"source_id":source_id,"attachment_key":key,"space":space,"expected_digest":digest,"allow_retained":true})).unwrap())).await;
            assert!(
                if expected_not_found {
                    matches!(result, Err(ServerError::NotFound(_)))
                } else {
                    matches!(result, Err(ServerError::Conflict(_)))
                },
                "retained bytes require exact Space and attachment authorization"
            );
        }
        let unrelated = b"%PDF-1.7 unrelated cached bytes";
        let unrelated_hash = sha256_hex(unrelated);
        write_cached_pdf(&unrelated_hash, unrelated).unwrap();
        assert!(matches!(handle_get_pdf(State(state.clone()),crate::space_header::SpaceHeader(None),Query(serde_json::from_value::<PdfQuery>(serde_json::json!({"source_id":source_id,"attachment_key":"ATTACH01","space":"Research","expected_digest":unrelated_hash,"allow_retained":true})).unwrap())).await,Err(ServerError::Conflict(_))), "an existing cache blob is not proof that a source owns it");
        std::fs::remove_file(pdf_cache_path(&hash).unwrap()).unwrap();
        assert!(matches!(handle_get_pdf(State(state.clone()),crate::space_header::SpaceHeader(None),Query(serde_json::from_value::<PdfQuery>(serde_json::json!({"source_id":source_id,"attachment_key":"ATTACH01","space":"Research","expected_digest":hash,"allow_retained":true})).unwrap())).await,Err(ServerError::NotFound(_))), "missing original bytes must never fall back to the replacement PDF");
        let response = handle_get_pdf(
            State(state.clone()),
            crate::space_header::SpaceHeader(None),
            Query(PdfQuery {
                source_id: source_id.clone(),
                attachment_key: "ATTACH01".into(),
                space: Some("Research".into()),
                expected_digest: Some(replacement_hash.clone()),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_response();
        let returned = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            sha256_hex(&returned),
            replacement_hash,
            "new metadata must read exactly its addressed immutable bytes"
        );
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
                        expected_digest: None,
                        source_id: source_id.clone(),
                        attachment_key: "ATTACH01".into(),
                        space: Some("Research".into()),
                        ..Default::default()
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
    async fn zotero_pending_disconnect_is_discoverable_after_config_removal() {
        use axum::{
            body::Body,
            http::{Request, StatusCode},
        };
        use tower::ServiceExt;
        let _lock = data_dir_lock().await;
        let _root = super::super::tests::DataDirGuard::new();
        let (db, _db_dir) = super::super::tests::new_test_db().await;
        db.create_space("Research", None, false).await.unwrap();
        let source = source();
        config::save_config(&config::Config {
            sources: vec![source.clone()],
            ..Default::default()
        })
        .unwrap();
        let identity = registration_identity(&db, &source).await.unwrap();
        db.bind_zotero_registration(&identity).await.unwrap();
        let canonical = canonical_source_id(
            Some(&identity.server_id),
            "ITEM0001",
            Some(&identity.space_id),
        );
        db.upsert_documents(vec![RawDocument {
            source: "file".into(),
            source_id: canonical.clone(),
            source_agent: Some("zotero".into()),
            title: "Retained paper".into(),
            content: "Human retained evidence.".into(),
            space: Some("Research".into()),
            ..Default::default()
        }])
        .await
        .unwrap();
        db.upsert_sync_state(&source.id, "ITEM0001", 0, "retained-hash")
            .await
            .unwrap();
        db.observe_zotero_selection(
            &identity,
            &std::collections::BTreeSet::from(["ITEM0001".into()]),
            &std::collections::BTreeMap::from([("ITEM0001".into(), canonical.clone())]),
        )
        .await
        .unwrap();
        db.begin_zotero_disconnect(&identity).await.unwrap();
        let state = Arc::new(RwLock::new(ServerState {
            db: Some(db.clone()),
            ..Default::default()
        }));
        state.read().await.maintenance_coordinator.finish_recovery();
        let router = crate::router::build_router(state);
        async fn listed(router: &crate::router::AppRouter) -> (StatusCode, Vec<Source>) {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/sources")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            (
                status,
                if status.is_success() {
                    serde_json::from_slice(&bytes).unwrap()
                } else {
                    vec![]
                },
            )
        }
        let lane = ZOTERO_MUTATION_LANE.lock().await;
        let (status, rows) =
            tokio::time::timeout(std::time::Duration::from_secs(1), listed(&router))
                .await
                .expect("Sources list must remain readable during remote Zotero sync");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(rows[0].id, source.id);
        drop(lane);
        let (status, rows) = listed(&router).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            rows[0].last_sync_error_detail.as_deref(),
            Some("zotero_disconnect_pending")
        );
        // A reused ID cannot mix a new source with the retained intention.
        config::update_config(|c| {
            c.sources[0].source_type = SourceType::Directory;
            Ok(())
        })
        .unwrap();
        assert_eq!(listed(&router).await.0, StatusCode::CONFLICT);
        config::update_config(|c| {
            c.sources.clear();
            Ok(())
        })
        .unwrap();
        let before = std::fs::read(config::data_root().join("config.json")).unwrap();
        let (status, rows) = listed(&router).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, source.id);
        assert_eq!(rows[0].path, source.path);
        assert_eq!(rows[0].space.as_deref(), Some("Research"));
        assert_eq!(
            rows[0].last_sync_error_detail.as_deref(),
            Some("zotero_disconnect_pending")
        );
        assert_eq!(
            std::fs::read(config::data_root().join("config.json")).unwrap(),
            before
        );
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/zotero/sources/{}/disconnect-preview",
                        source.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let preview: ZoteroDisconnectPreview = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(preview.pending_disconnect);
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!(
                        "/api/sources/{}?expected_revision={}&expected_identity={}",
                        source.id, preview.revision, preview.identity_digest
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(listed(&router).await.1.is_empty());
        assert_eq!(
            db.get_sync_state(&source.id, "ITEM0001")
                .await
                .unwrap()
                .unwrap()
                .content_hash,
            "retained-hash"
        );
        assert!(db
            .get_chunks_scoped(&canonical, &ReadScope::Space("Research".into()))
            .await
            .unwrap()
            .unwrap()
            .iter()
            .any(|chunk| chunk.content.contains("Human retained evidence.")));
    }

    #[tokio::test]
    async fn zotero_repair_fence_blocks_both_disconnect_paths_and_sync_without_mutation() {
        use axum::{
            body::Body,
            http::{Request, StatusCode},
        };
        use tower::ServiceExt;
        let _lock = data_dir_lock().await;
        let _root = super::super::tests::DataDirGuard::new();
        let (db, _db_dir) = super::super::tests::new_test_db().await;
        db.create_space("Research", None, false).await.unwrap();
        let source = source();
        config::save_config(&config::Config {
            sources: vec![source.clone()],
            ..Default::default()
        })
        .unwrap();
        let identity = registration_identity(&db, &source).await.unwrap();
        db.bind_zotero_registration(&identity).await.unwrap();
        let canonical = canonical_source_id(
            Some(&identity.server_id),
            "ITEM0001",
            Some(&identity.space_id),
        );
        db.upsert_documents(vec![RawDocument {
            source: "file".into(),
            source_id: canonical.clone(),
            source_agent: Some("zotero".into()),
            title: "Retained paper".into(),
            content: "Original retained evidence.".into(),
            space: Some("Research".into()),
            ..Default::default()
        }])
        .await
        .unwrap();
        db.upsert_sync_state(&source.id, "ITEM0001", 0, "retained-hash")
            .await
            .unwrap();
        db.observe_zotero_selection(
            &identity,
            &std::collections::BTreeSet::from(["ITEM0001".into()]),
            &std::collections::BTreeMap::from([("ITEM0001".into(), canonical.clone())]),
        )
        .await
        .unwrap();
        let fixture = libsql::Builder::new_local(_db_dir.path().join("origin_memory.db"))
            .build()
            .await
            .unwrap();
        let conn = fixture.connect().unwrap();
        conn.execute("INSERT INTO pages(id,title,content,source_memory_ids,created_at,last_compiled,last_modified,space,workspace,kind,creation_kind,review_status,incarnation) VALUES('fenced-review','Human review','Human research conclusion.',?1,'2026-11-06','2026-11-06','2026-11-06','Research','Research','authored','authored','confirmed','zotero-fence-fixture')",[serde_json::to_string(&vec![canonical.clone()]).unwrap()]).await.unwrap();
        let preview = db.zotero_disconnect_preview(&identity).await.unwrap();
        assert_eq!(preview.affected_page_ids, vec!["fenced-review"]);
        let config_before = std::fs::read(config::data_root().join("config.json")).unwrap();
        let page_before =
            serde_json::to_value(db.get_page("fenced-review").await.unwrap().unwrap()).unwrap();
        let state = Arc::new(RwLock::new(ServerState {
            db: Some(db.clone()),
            ..Default::default()
        }));
        let router = crate::router::build_router(state.clone());
        let ordinary = format!("/api/sources/{}", source.id);
        let fenced = format!(
            "{ordinary}?expected_revision={}&expected_identity={}",
            preview.revision, preview.identity_digest
        );
        let sync = format!("{ordinary}/sync");
        for (method, uri) in [("DELETE", ordinary.as_str()), ("DELETE", fenced.as_str())] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(),StatusCode::CONFLICT,"sealed repair fence must reject {method} {uri} before mutation or remote Zotero fetch");
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            assert!(String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("repair_write_fence_conflict"));
        }
        assert_eq!(
            std::fs::read(config::data_root().join("config.json")).unwrap(),
            config_before
        );
        // An invalid fixture locator ensures even an unfenced sync cannot
        // accidentally contact a real local Zotero while proving early rejection.
        let mut malformed = source.clone();
        malformed.path = PathBuf::from("invalid-zotero-fixture-locator");
        config::save_config(&config::Config {
            sources: vec![malformed],
            ..Default::default()
        })
        .unwrap();
        let malformed_config = std::fs::read(config::data_root().join("config.json")).unwrap();
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&sync)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::CONFLICT,
            "sealed sync must reject before parsing or remote I/O"
        );
        assert_eq!(
            std::fs::read(config::data_root().join("config.json")).unwrap(),
            malformed_config
        );
        std::fs::write(config::data_root().join("config.json"), &config_before).unwrap();
        assert_eq!(
            db.zotero_disconnect_preview(&identity).await.unwrap(),
            preview
        );
        assert_eq!(
            serde_json::to_value(db.get_page("fenced-review").await.unwrap().unwrap()).unwrap(),
            page_before
        );
        assert_eq!(
            db.get_sync_state(&source.id, "ITEM0001")
                .await
                .unwrap()
                .unwrap()
                .content_hash,
            "retained-hash"
        );
        // The normal ready daemon path must still complete and retain data.
        state.read().await.maintenance_coordinator.finish_recovery();
        let response = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(&fenced)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(
            db.zotero_disconnect_preview(&identity)
                .await
                .unwrap()
                .disconnected
        );
        assert_eq!(
            db.get_page("fenced-review").await.unwrap().unwrap().content,
            "Human research conclusion."
        );
    }

    #[tokio::test]
    async fn zotero_legacy_offline_disconnect_reports_unknown_impact_and_retains_receipts() {
        let _lock = data_dir_lock().await;
        let _root = super::super::tests::DataDirGuard::new();
        let (db, _db_dir) = super::super::tests::new_test_db().await;
        db.create_space("Research", None, false).await.unwrap();
        let mut source = source();
        source.path = PathBuf::from(locator(&None, "COLLECT1"));
        config::save_config(&config::Config {
            sources: vec![source.clone()],
            ..Default::default()
        })
        .unwrap();
        db.upsert_sync_state(&source.id, "ITEM0001", 0, "old-content")
            .await
            .unwrap();
        assert!(registration_identity(&db, &source).await.is_err());
        let identity = disconnect_identity(&db, &source).await.unwrap();
        assert!(identity.server_id.is_empty());
        let preview = db.zotero_disconnect_preview(&identity).await.unwrap();
        assert!(!preview.impact_scope_known);
        assert!(preview.canonical_item_ids.is_empty());
        disconnect_source(
            db.clone(),
            &source.id,
            Some((preview.revision, &preview.identity_digest)),
        )
        .await
        .unwrap();
        assert!(config::load_config().sources.is_empty());
        assert!(db
            .get_sync_state(&source.id, "ITEM0001")
            .await
            .unwrap()
            .is_some());
        assert!(
            !db.zotero_disconnect_preview(&identity)
                .await
                .unwrap()
                .impact_scope_known
        );
        assert!(db
            .observe_zotero_selection(
                &identity,
                &std::collections::BTreeSet::new(),
                &std::collections::BTreeMap::new()
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn zotero_membership_rejects_incomplete_malformed_and_changed_registration_and_retains_pending_disconnect(
    ) {
        let _lock = data_dir_lock().await;
        let _root = super::super::tests::DataDirGuard::new();
        let (db, _db_dir) = super::super::tests::new_test_db().await;
        db.create_space("Research", None, false).await.unwrap();
        let source = source();
        config::save_config(&config::Config {
            sources: vec![source.clone()],
            ..Default::default()
        })
        .unwrap();
        let mut client = fake(vec![article("ITEM0001", "Paper", "Evidence")], &[]);
        sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .unwrap();
        let identity = registration_identity(&db, &source).await.unwrap();
        let before = db.zotero_disconnect_preview(&identity).await.unwrap();
        client.incomplete_items = true;
        client.items.clear();
        assert!(sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .is_err());
        assert_eq!(
            before,
            db.zotero_disconnect_preview(&identity).await.unwrap()
        );
        client.incomplete_items = false;
        client.items = vec![article("bad", "Malformed", "Evidence")];
        assert!(sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .is_err());
        assert_eq!(
            before,
            db.zotero_disconnect_preview(&identity).await.unwrap()
        );
        client.items = vec![
            article("ITEM0001", "Paper", "Evidence"),
            article("ITEM0001", "Duplicate", "Evidence"),
        ];
        assert!(sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .is_err());
        assert_eq!(
            before,
            db.zotero_disconnect_preview(&identity).await.unwrap()
        );
        // A listed item whose content fails remains present, never an absence.
        client.items.truncate(1);
        client.items[0]
            .data
            .extra
            .insert("collections".into(), serde_json::json!(["FOREIGN1"]));
        assert!(sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .is_err());
        assert_eq!(
            before,
            db.zotero_disconnect_preview(&identity).await.unwrap()
        );
        client.items[0].data.extra.remove("collections");
        client.fail_children_for.insert("ITEM0001".into());
        sync_source_with_client(db.clone(), &source, &mut client)
            .await
            .unwrap();
        assert_eq!(
            before,
            db.zotero_disconnect_preview(&identity).await.unwrap()
        );
        // A stale preview and a reused mutable config ID/type cannot authorize removal.
        db.observe_zotero_selection(
            &identity,
            &std::collections::BTreeSet::new(),
            &std::collections::BTreeMap::new(),
        )
        .await
        .unwrap();
        assert!(matches!(
            disconnect_source(
                db.clone(),
                &source.id,
                Some((before.revision, &before.identity_digest))
            )
            .await,
            Err(ServerError::Conflict(_))
        ));
        let mut reused = source.clone();
        reused.source_type = SourceType::Directory;
        config::save_config(&config::Config {
            sources: vec![reused],
            ..Default::default()
        })
        .unwrap();
        assert!(matches!(
            disconnect_source(db.clone(), &source.id, None).await,
            Err(ServerError::Conflict(_))
        ));
        assert_eq!(config::load_config().sources.len(), 1);
        config::save_config(&config::Config {
            sources: vec![source.clone()],
            ..Default::default()
        })
        .unwrap();
        db.begin_zotero_disconnect(&identity).await.unwrap();
        std::fs::write(config::data_root().join("config.json"), "not valid JSON").unwrap();
        assert!(disconnect_source(db.clone(), &source.id, None)
            .await
            .is_err());
        assert!(
            db.zotero_disconnect_preview(&identity)
                .await
                .unwrap()
                .pending_disconnect
        );
        assert!(db
            .get_sync_state(&source.id, "ITEM0001")
            .await
            .unwrap()
            .is_some());
        std::fs::write(
            config::data_root().join("config.json"),
            serde_json::to_vec(&config::Config {
                sources: vec![source.clone()],
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        disconnect_source(db.clone(), &source.id, None)
            .await
            .unwrap();
        disconnect_source(db.clone(), &source.id, None)
            .await
            .unwrap();
        assert!(
            db.zotero_disconnect_preview(&identity)
                .await
                .unwrap()
                .disconnected
        );
        assert!(db
            .get_sync_state(&source.id, "ITEM0001")
            .await
            .unwrap()
            .is_some());
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
        state.read().await.maintenance_coordinator.finish_recovery();
        let response = handle_get_document(
            State(state.clone()),
            crate::space_header::SpaceHeader(None),
            Query(ZoteroDocumentQuery {
                source_id: canonical.clone(),
                space: Some("Research".into()),
                ..Default::default()
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
                ..Default::default()
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
                expected_digest: None,
                source_id: canonical.clone(),
                attachment_key: "ATTACH01".into(),
                space: Some("Research".into()),
                ..Default::default()
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
                ..Default::default()
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
                    ..Default::default()
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
                    expected_digest: None,
                    source_id: canonical.clone(),
                    attachment_key: "ATTACH01".into(),
                    space: Some("Other".into()),
                    ..Default::default()
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

#[cfg(test)]
mod retained_pdf_generation_tests {
    use super::*;

    #[test]
    fn retained_generation_requires_exact_source_space_server_item_attachment_and_digest() {
        let old_hash = "a".repeat(64);
        let current = ZoteroDocumentSidecar {
            source_id: "exact-source".into(),
            space: Some("Research".into()),
            server_id: Some("server-a".into()),
            item_key: "ITEM0001".into(),
            title: "Current".into(),
            rendered_content: String::new(),
            bibliography: HashMap::new(),
            bibtex: String::new(),
            warnings: vec![],
            annotations: vec![],
            import_registration: None,
            attachments: vec![ZoteroAttachmentSidecar {
                key: "ATTACH01".into(),
                title: "paper.pdf".into(),
                content_type: Some("application/pdf".into()),
                cached_pdf_hash: Some("b".repeat(64)),
            }],
        };
        let mut old = current.clone();
        old.title = "Original".into();
        old.attachments[0].cached_pdf_hash = Some(old_hash.clone());
        let mut history = ZoteroSidecarHistory {
            current: Some(current),
            snapshots: vec![old.clone()],
        };
        assert!(matches!(
            selected_pdf_generation(&history, Some("ATTACH01"), Some(&old_hash), false),
            Err(ServerError::Conflict(_))
        ));
        let (selected, historical) =
            selected_pdf_generation(&history, Some("ATTACH01"), Some(&old_hash), true).unwrap();
        assert!(historical);
        assert_eq!(selected.title, "Original");
        history.current.as_mut().unwrap().attachments.clear();
        assert!(
            selected_pdf_generation(&history, Some("ATTACH01"), Some(&old_hash), true)
                .unwrap()
                .1,
            "a retired attachment still has its proven retained generation"
        );
        for dimension in ["source", "space", "server", "item", "attachment", "digest"] {
            let mut forged = old.clone();
            match dimension {
                "source" => forged.source_id = "another-source".into(),
                "space" => forged.space = Some("Other".into()),
                "server" => forged.server_id = Some("server-b".into()),
                "item" => forged.item_key = "ITEM0002".into(),
                "attachment" => forged.attachments[0].key = "ATTACH02".into(),
                _ => forged.attachments[0].cached_pdf_hash = Some("c".repeat(64)),
            }
            history.snapshots = vec![forged];
            assert!(
                matches!(
                    selected_pdf_generation(&history, Some("ATTACH01"), Some(&old_hash), true),
                    Err(ServerError::Conflict(_))
                ),
                "{dimension} cannot authorize a retained generation"
            );
        }
        assert!(matches!(
            selected_pdf_generation(&history, None, None, true),
            Err(ServerError::BadRequest(_))
        ));
    }
}
