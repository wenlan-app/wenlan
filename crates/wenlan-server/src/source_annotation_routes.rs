// SPDX-License-Identifier: Apache-2.0
//! User-authored annotations for exact-kind saved sources.

use crate::error::ServerError;
use crate::route_registry::{get, TrackedRouter};
use crate::state::SharedState;
use axum::{
    extract::{RawQuery, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::Json,
};
use std::collections::HashMap;
use wenlan_core::{
    db::MemoryDB,
    read_scope::{resolve_read_scope, ReadScope},
    truth_contract::TruthGrant,
};
use wenlan_types::source_annotations::{
    SourceAnnotation, SourceAnnotationPage, UpdateSourceAnnotationRequest,
};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router.route(
        "/api/source-annotations",
        get(handle_get_source_annotation).post(handle_update_source_annotation),
    )
}

fn selector_header(headers: &HeaderMap) -> Result<Option<String>, ServerError> {
    let mut selectors = Vec::new();
    for name in ["x-wenlan-space", "x-origin-space"] {
        for value in headers.get_all(name).iter() {
            let value = value
                .to_str()
                .map_err(|_| ServerError::BadRequest("invalid Space header".into()))?;
            if value.trim().is_empty() {
                return Err(ServerError::BadRequest(
                    "Space selector must not be blank".into(),
                ));
            }
            selectors.push(value.trim().to_string());
        }
    }
    if selectors.windows(2).any(|pair| pair[0] != pair[1]) {
        return Err(ServerError::BadRequest("conflicting Space headers".into()));
    }
    Ok(selectors.into_iter().next())
}

fn strict_query(raw: Option<String>) -> Result<HashMap<String, String>, ServerError> {
    let mut values = HashMap::new();
    for (key, value) in url::form_urlencoded::parse(raw.as_deref().unwrap_or_default().as_bytes()) {
        let key = key.into_owned();
        let value = value.into_owned();
        if let Some(previous) = values.get(&key) {
            if previous != &value {
                return Err(ServerError::BadRequest(format!(
                    "conflicting duplicate query parameter: {key}"
                )));
            }
        } else {
            values.insert(key, value);
        }
    }
    Ok(values)
}

fn normalize_selector(value: Option<&str>, label: &str) -> Result<Option<String>, ServerError> {
    let Some(value) = value else { return Ok(None) };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ServerError::BadRequest(format!(
            "{label} selector must not be blank"
        )));
    }
    Ok(Some(trimmed.to_string()))
}

fn exact_selector(
    query: Option<&str>,
    body: Option<&str>,
    header: Option<String>,
) -> Result<Option<String>, ServerError> {
    let query = normalize_selector(query, "query Space")?;
    let body = normalize_selector(body, "body Space")?;
    let candidates = [query, body, header];
    let selected = candidates.iter().flatten().next().cloned();
    if candidates
        .iter()
        .flatten()
        .any(|candidate| Some(candidate) != selected.as_ref())
    {
        return Err(ServerError::BadRequest(
            "conflicting Space selectors".into(),
        ));
    }
    Ok(selected)
}

async fn resolve_scope_and_key(
    db: &MemoryDB,
    selector: Option<&str>,
) -> Result<(ReadScope, String), ServerError> {
    let scope = resolve_read_scope(db, selector)
        .await
        .map_err(|error| match error {
            wenlan_core::read_scope::ReadScopeResolveError::Unknown(name) => {
                ServerError::ValidationError(format!("unknown Space: {name}"))
            }
            wenlan_core::read_scope::ReadScopeResolveError::AmbiguousUncategorized => {
                ServerError::ValidationError(
                    "uncategorized is ambiguous with a registered Space".into(),
                )
            }
            wenlan_core::read_scope::ReadScopeResolveError::Store(error) => error.into(),
        })?;
    let key = match &scope {
        ReadScope::Global => "global".to_string(),
        ReadScope::Uncategorized => "uncategorized".to_string(),
        ReadScope::Space(name) => db
            .get_space(name)
            .await?
            .map(|space| space.id)
            .ok_or_else(|| ServerError::ValidationError(format!("unknown Space: {name}")))?,
    };
    Ok((scope, key))
}

fn source_identity(source: &str, source_id: &str) -> Result<(String, String), ServerError> {
    if source.trim().is_empty() || source_id.trim().is_empty() {
        return Err(ServerError::BadRequest(
            "source and source_id must not be blank".into(),
        ));
    }
    if source != "webpage" {
        return Err(ServerError::BadRequest(
            "unsupported source kind; expected webpage".into(),
        ));
    }
    if source_id != source_id.trim() {
        return Err(ServerError::BadRequest(
            "source_id must preserve the exact source identity without surrounding whitespace"
                .into(),
        ));
    }
    Ok((source.to_string(), source_id.to_string()))
}

fn source_scope_headers(selector: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(selector) = selector {
        let encoded: String = selector
            .bytes()
            .flat_map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    vec![byte as char]
                }
                _ => format!("%{byte:02X}").chars().collect(),
            })
            .collect();
        headers.insert(
            "x-wenlan-source-scope",
            HeaderValue::from_str(&encoded).expect("percent encoded scope is ASCII"),
        );
    }
    headers
}

async fn visible_source(
    db: &MemoryDB,
    source_id: &str,
    scope: &ReadScope,
) -> Result<(), ServerError> {
    if db
        .get_webpage_chunks_scoped(source_id, scope)
        .await?
        .is_none()
    {
        return Err(ServerError::NotFound("source not found".into()));
    }
    Ok(())
}

async fn hydrate_pages(
    db: &MemoryDB,
    scope: &ReadScope,
    grant: &TruthGrant,
    annotation: &mut SourceAnnotation,
) -> Result<(), ServerError> {
    let mut visible_ids = Vec::new();
    let mut pages = Vec::new();
    for id in &annotation.page_ids {
        let Some(page) = db.get_page_scoped(id, scope).await? else {
            continue;
        };
        if page.status != "active" || page.kind == "entity" {
            continue;
        }
        let Some(page) = wenlan_core::truth_adapter::filter_page(db, grant, Some(page)).await?
        else {
            continue;
        };
        visible_ids.push(id.clone());
        pages.push(SourceAnnotationPage {
            id: id.clone(),
            title: page.title,
        });
    }
    annotation.page_ids = visible_ids;
    annotation.pages = pages;
    Ok(())
}

/// GET /api/source-annotations?source=webpage&source_id=...
pub async fn handle_get_source_annotation(
    State(state): State<SharedState>,
    headers: HeaderMap,
    view: crate::truth_guard::TruthView,
    RawQuery(raw_query): RawQuery,
) -> Result<(HeaderMap, Json<SourceAnnotation>), ServerError> {
    let query = strict_query(raw_query)?;
    let (source, source_id) = source_identity(
        query.get("source").map(String::as_str).unwrap_or_default(),
        query
            .get("source_id")
            .map(String::as_str)
            .unwrap_or_default(),
    )?;
    let selector = exact_selector(
        query.get("space").map(String::as_str),
        None,
        selector_header(&headers)?,
    )?;
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    let (scope, context_key) = resolve_scope_and_key(&db, selector.as_deref()).await?;
    visible_source(&db, &source_id, &scope).await?;
    let mut annotation = db
        .get_source_annotation(&source, &source_id, &context_key)
        .await?
        .unwrap_or(SourceAnnotation {
            source,
            source_id,
            notes: String::new(),
            page_ids: Vec::new(),
            version: 0,
            updated_at: 0,
            pages: Vec::new(),
        });
    hydrate_pages(&db, &scope, &view.grant, &mut annotation).await?;
    Ok((source_scope_headers(selector.as_deref()), Json(annotation)))
}

/// POST /api/source-annotations — patch notes and/or Page association IDs.
pub async fn handle_update_source_annotation(
    State(state): State<SharedState>,
    headers: HeaderMap,
    view: crate::truth_guard::TruthView,
    RawQuery(raw_query): RawQuery,
    Json(request): Json<UpdateSourceAnnotationRequest>,
) -> Result<(StatusCode, Json<SourceAnnotation>), ServerError> {
    let query = strict_query(raw_query)?;
    let (source, source_id) = source_identity(&request.source, &request.source_id)?;
    let selector = exact_selector(
        query.get("space").map(String::as_str),
        request.space.as_deref(),
        selector_header(&headers)?,
    )?;
    let db = state
        .read()
        .await
        .db
        .clone()
        .ok_or(ServerError::DbNotInitialized)?;
    // Serialize scope resolution, destination validation, and persistence with
    // Page moves/Space renames that use the database's established lock.
    let _space_write = db.lock_space_writes().await;
    let (scope, context_key) = resolve_scope_and_key(&db, selector.as_deref()).await?;
    visible_source(&db, &source_id, &scope).await?;
    if let Some(notes) = request.notes.as_deref() {
        if notes.len() > wenlan_core::db::source_annotations::MAX_SOURCE_ANNOTATION_NOTES_BYTES {
            return Err(ServerError::ValidationError(
                "notes exceed 65536 UTF-8 bytes".into(),
            ));
        }
    }
    if let Some(page_ids) = request.page_ids.as_ref() {
        if page_ids.len() > wenlan_core::db::source_annotations::MAX_SOURCE_ANNOTATION_PAGES {
            return Err(ServerError::ValidationError(
                "page_ids exceed 100 entries".into(),
            ));
        }
        let mut unique = std::collections::HashSet::new();
        for id in page_ids {
            if id.trim().is_empty() || !unique.insert(id) {
                return Err(ServerError::ValidationError(
                    "page_ids must be nonblank and unique".into(),
                ));
            }
            let Some(page) = db.get_page_scoped(id, &scope).await? else {
                return Err(ServerError::ValidationError(
                    "page_ids must reference active Pages visible in this scope".into(),
                ));
            };
            if page.status != "active" || page.kind == "entity" {
                return Err(ServerError::ValidationError(
                    "page_ids must reference active Pages visible in this scope".into(),
                ));
            }
            if wenlan_core::truth_adapter::filter_page(&db, &view.grant, Some(page))
                .await?
                .is_none()
            {
                return Err(ServerError::ValidationError(
                    "page_ids must reference active Pages visible in this scope".into(),
                ));
            }
        }
    }
    // Clients replace only the associations they can see. Preserve stored
    // links outside that view without disclosing their IDs in the response.
    // The subsequent version check rejects a concurrent annotation edit.
    let mut replacement_page_ids = request.page_ids.clone();
    if let Some(replacement) = replacement_page_ids.as_mut() {
        if let Some(existing) = db
            .get_source_annotation(&source, &source_id, &context_key)
            .await?
        {
            let mut visible = existing.clone();
            hydrate_pages(&db, &scope, &view.grant, &mut visible).await?;
            for id in existing.page_ids {
                if !visible.page_ids.contains(&id) && !replacement.contains(&id) {
                    replacement.push(id);
                }
            }
        }
    }
    let outcome = db
        .write_source_annotation(
            &source,
            &source_id,
            &context_key,
            request.expected_version,
            request.notes.as_deref(),
            replacement_page_ids.as_deref(),
        )
        .await?;
    let mut annotation = match outcome {
        wenlan_core::db::source_annotations::SourceAnnotationWrite::Saved(annotation) => annotation,
        wenlan_core::db::source_annotations::SourceAnnotationWrite::VersionConflict {
            current_version,
        } => {
            return Err(ServerError::Conflict(format!(
                "source annotation version conflict; current version is {current_version}"
            )))
        }
    };
    hydrate_pages(&db, &scope, &view.grant, &mut annotation).await?;
    Ok((StatusCode::OK, Json(annotation)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_query_rejects_conflicting_duplicate_selectors() {
        assert!(strict_query(Some("space=work&space=private".into())).is_err());
        assert!(strict_query(Some("source=webpage&source=document".into())).is_err());
        assert!(strict_query(Some("source_id=a&source_id=b".into())).is_err());
        assert_eq!(
            strict_query(Some("space=work&space=work".into()))
                .unwrap()
                .get("space")
                .map(String::as_str),
            Some("work")
        );
    }

    #[tokio::test]
    async fn get_and_post_routes_reject_conflicting_duplicate_space_query_values() {
        use axum::{
            body::Body,
            http::{Request, StatusCode},
        };
        use tower::ServiceExt;

        let state = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::state::ServerState::default(),
        ));
        let app = crate::router::build_router(state);
        let get_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/source-annotations?source=webpage&source_id=https%3A%2F%2Fexample.test%2Fa&space=work&space=private")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_response.status(), StatusCode::BAD_REQUEST);

        let post_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/source-annotations?space=work&space=private")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"source":"webpage","source_id":"https://example.test/a","expected_version":0,"notes":"note"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(post_response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn selectors_reject_blank_and_conflicting_values() {
        assert!(exact_selector(Some("  "), None, None).is_err());
        assert!(exact_selector(Some("work"), Some("career"), None).is_err());
        assert!(exact_selector(Some("work"), None, Some("career".into())).is_err());
        assert_eq!(
            exact_selector(Some(" work "), Some("work"), Some("work".into())).unwrap(),
            Some("work".into())
        );
    }

    #[tokio::test]
    async fn page_association_accepts_active_source_projected_page_ids() {
        use axum::{
            body::{to_bytes, Body},
            http::{Request, StatusCode},
        };
        use std::sync::Arc;
        use tower::ServiceExt;

        let temp = tempfile::tempdir().unwrap();
        let db = Arc::new(
            MemoryDB::new(temp.path(), Arc::new(wenlan_core::events::NoopEmitter))
                .await
                .unwrap(),
        );
        let source_id = "https://example.test/source-projected-page";
        db.upsert_documents(vec![wenlan_core::sources::RawDocument {
            source: "webpage".to_string(),
            source_id: source_id.to_string(),
            title: "Saved source".to_string(),
            content: "Source content retained for a source-projected Page.".to_string(),
            url: Some(source_id.to_string()),
            ..Default::default()
        }])
        .await
        .unwrap();
        let memories = db
            .get_memories_by_source_id("webpage", source_id)
            .await
            .unwrap();
        let page_id = wenlan_core::document_enrichment::source_page_id(
            source_id,
            "/tmp/source-projected-page.md",
        );
        assert!(page_id.starts_with("src_"));
        let request = wenlan_types::requests::CreateConceptRequest {
            title: "Saved source Page".to_string(),
            content: "Active source-projected Page content.".to_string(),
            summary: Some("Source Page".to_string()),
            entity_id: None,
            space: None.into(),
            source_memory_ids: memories.into_iter().map(|memory| memory.id).collect(),
            creation_kind: Some("source".to_string()),
            workspace: None,
        };
        wenlan_core::post_write::page_write(
            &db,
            wenlan_core::post_write::PageWrite::Create {
                page_id: Some(&page_id),
                req: request,
                agent: "doc-enrich",
                knowledge_path: None,
                page_min_cluster_size: 1,
                page_match_threshold: 0.0,
                citations_json: None,
                basis_marks: Vec::new(),
            },
        )
        .await
        .unwrap();
        db.set_page_review_status(&page_id, "confirmed")
            .await
            .unwrap();

        let state = std::sync::Arc::new(tokio::sync::RwLock::new(crate::state::ServerState {
            db: Some(db.clone()),
            ..Default::default()
        }));
        let app = crate::router::build_router(state);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/source-annotations")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "source": "webpage",
                            "source_id": source_id,
                            "expected_version": 0,
                            "page_ids": [page_id]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 32 * 1024).await.unwrap();
        let saved: SourceAnnotation = serde_json::from_slice(&body).unwrap();
        assert_eq!(saved.page_ids, vec![page_id.clone()]);
        assert_eq!(
            saved.pages,
            vec![SourceAnnotationPage {
                id: page_id.clone(),
                title: "Saved source Page".to_string(),
            }]
        );

        // Stored links may become invisible after archival or a scope change.
        // A visible-only replacement must neither reveal nor erase them.
        let hidden_id = "unavailable-page".to_string();
        db.write_source_annotation(
            "webpage",
            source_id,
            "global",
            1,
            None,
            Some(std::slice::from_ref(&hidden_id)),
        )
        .await
        .unwrap();
        for (version, visible_ids) in [(2, vec![page_id]), (3, Vec::new())] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/source-annotations")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            serde_json::json!({
                                "source": "webpage", "source_id": source_id,
                                "expected_version": version, "page_ids": visible_ids
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), 32 * 1024).await.unwrap();
            let visible: SourceAnnotation = serde_json::from_slice(&body).unwrap();
            assert_eq!(visible.page_ids, visible_ids);
            assert!(!String::from_utf8_lossy(&body).contains(&hidden_id));
            let stored = db
                .get_source_annotation("webpage", source_id, "global")
                .await
                .unwrap()
                .unwrap();
            assert!(stored.page_ids.contains(&hidden_id));
            assert_eq!(stored.page_ids.len(), visible_ids.len() + 1);
        }
    }

    #[test]
    fn scoped_source_response_acknowledges_encoded_space_but_global_does_not() {
        let scoped = source_scope_headers(Some("研發 工具"));
        assert_eq!(
            scoped
                .get("x-wenlan-source-scope")
                .unwrap()
                .to_str()
                .unwrap(),
            "%E7%A0%94%E7%99%BC%20%E5%B7%A5%E5%85%B7"
        );
        assert!(source_scope_headers(None)
            .get("x-wenlan-source-scope")
            .is_none());
    }

    #[test]
    fn source_kind_is_exact_and_source_id_is_preserved_verbatim() {
        let source_id = "https://example.test/a/long/path?x=one&y=two#fragment";
        let (source, persisted_id) = source_identity("webpage", source_id).unwrap();
        assert_eq!(source, "webpage");
        assert_eq!(persisted_id, source_id);
        assert!(source_identity("document", source_id).is_err());
        assert!(source_identity("webpage", "   ").is_err());
        assert!(source_identity("webpage", " https://example.test/a ").is_err());
    }
}
