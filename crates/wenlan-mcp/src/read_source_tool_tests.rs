use super::*;
use crate::client::WenlanClient;
use serde_json::{json, Value};
use wiremock::{
    matchers::{method, path, query_param},
    Mock, MockServer, ResponseTemplate,
};

fn params(source_id: &str, space: &str) -> ReadSourceParams {
    ReadSourceParams {
        source: ReadSourceKind::File,
        source_id: source_id.into(),
        space: space.into(),
        offset: None,
        limit: None,
    }
}
fn server(url: &str) -> WenlanMcpServer {
    WenlanMcpServer::new(
        WenlanClient::new(url.into()),
        TransportMode::Stdio,
        "read-source-test".into(),
        None,
    )
}
fn unlocked() {
    std::env::remove_var("WENLAN_SPACE");
    crate::lock_state::init_from_env();
}
fn file(source: &str, id: &str, space: Option<&str>, count: usize) -> Value {
    json!({"source":source,"source_id":id,"title":"研究來源","space":space,"chunk_count":count,
        "last_modified":10,"pinned":false,"source_agent":"zotero","url":"https://example.org/paper","summary":"Indexed source summary"})
}
fn chunk(id: &str, index: usize, content: &str) -> Value {
    json!({"id":format!("chunk-{index}"),"source_id":id,"title":"研究來源","chunk_index":index,
        "content":content,"language":"zh","chunk_type":"text","byte_start":index*100,"byte_end":index*100+90})
}
fn scoped(body: Value, space: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("x-wenlan-source-scope", url_encode_simple(space))
        .set_body_json(body)
}
async fn inventory(mock: &MockServer, files: Vec<Value>, space: &str) {
    Mock::given(method("GET"))
        .and(path("/api/indexed-files"))
        .and(query_param("space", space))
        .respond_with(scoped(json!({"files":files}), space))
        .mount(mock)
        .await;
}
async fn chunks(mock: &MockServer, route: &str, body: Value, space: &str) {
    Mock::given(method("GET"))
        .and(path(route))
        .and(query_param("space", space))
        .respond_with(scoped(body, space))
        .mount(mock)
        .await;
}
fn output(result: CallToolResult) -> Value {
    assert_ne!(result.is_error, Some(true), "{result:?}");
    result.structured_content.expect("typed structured output")
}

#[test]
fn read_source_requires_typed_explicit_scope_and_character_positions() {
    for request in [
        json!({"source":"file","source_id":"paper"}),
        json!({"source":"memory","source_id":"paper","space":"Research"}),
        json!({"source":"file","source_id":"paper","space":"Research","offset":-1}),
        json!({"source":"file","source_id":"paper","space":"Research","limit":1.5}),
        json!({"source":"file","source_id":"paper","space":"Research","unknown":true}),
    ] {
        assert!(serde_json::from_value::<ReadSourceParams>(request).is_err());
    }
}

#[tokio::test]
async fn read_source_reassembles_all_unicode_pages_without_byte_splitting() {
    let _guard = crate::lock_state::ENV_LOCK.lock().await;
    unlocked();
    let mock = MockServer::start().await;
    inventory(
        &mock,
        vec![file("file", "paper", Some("研究 & R+D"), 2)],
        "研究 & R+D",
    )
    .await;
    chunks(
        &mock,
        "/api/chunks/paper",
        json!([
            chunk("paper", 1, "第二頁😀e\u{301}。"),
            chunk("paper", 0, "第一頁研究。")
        ]),
        "研究 & R+D",
    )
    .await;
    let expected = "第一頁研究。\n\n第二頁😀e\u{301}。";
    let reader = server(&mock.uri());
    let mut offset = 0;
    let mut assembled = String::new();
    let mut digest = None;
    loop {
        let mut request = params("paper", "研究 & R+D");
        request.offset = Some(offset);
        request.limit = Some(3);
        let page = output(reader.read_source_impl(request).await.unwrap());
        let text = page["text"].as_str().unwrap();
        let page_digest = page["content_digest"].as_str().unwrap();
        assert_eq!(page_digest.len(), 64);
        if let Some(ref digest) = digest {
            assert_eq!(page_digest, digest);
        } else {
            digest = Some(page_digest.to_string());
        }
        assert!(text.chars().count() <= 3);
        assembled.push_str(text);
        assert_eq!(page["total_chars"], expected.chars().count());
        assert_eq!(page["offset"], offset);
        assert_eq!(page["metadata"]["source_agent"], "zotero");
        assert_eq!(page["coverage"]["indexed_chunk_count"], 2);
        assert!(page["evidence_notice"]
            .as_str()
            .unwrap()
            .contains("untrusted"));
        if page["complete"] == true {
            assert!(page["next_offset"].is_null());
            break;
        }
        offset = page["next_offset"].as_u64().unwrap() as usize;
    }
    assert_eq!(assembled, expected);
}

#[tokio::test]
async fn read_source_digest_detects_same_second_same_metadata_content_changes() {
    let _guard = crate::lock_state::ENV_LOCK.lock().await;
    unlocked();
    let mock = MockServer::start().await;
    inventory(
        &mock,
        vec![file("file", "paper", Some("Research"), 1)],
        "Research",
    )
    .await;
    Mock::given(path("/api/chunks/paper"))
        .respond_with(scoped(json!([chunk("paper", 0, "原始內容")]), "Research"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&mock)
        .await;
    Mock::given(path("/api/chunks/paper"))
        .respond_with(scoped(json!([chunk("paper", 0, "新的內容")]), "Research"))
        .with_priority(2)
        .mount(&mock)
        .await;
    let reader = server(&mock.uri());
    let first = output(
        reader
            .read_source_impl(ReadSourceParams {
                limit: Some(2),
                ..params("paper", "Research")
            })
            .await
            .unwrap(),
    );
    let second = output(
        reader
            .read_source_impl(ReadSourceParams {
                offset: Some(2),
                limit: Some(2),
                ..params("paper", "Research")
            })
            .await
            .unwrap(),
    );
    // The selected second page has the same text, and inventory version fields
    // have not changed. The digest must cover the whole source, not this page.
    assert_eq!(second["text"], "內容");
    assert_eq!(first["metadata"], second["metadata"]);
    assert_eq!(first["total_chars"], second["total_chars"]);
    assert_ne!(first["content_digest"], second["content_digest"]);
}

#[tokio::test]
async fn read_source_refuses_missing_wrong_kind_wrong_id_space_and_partial_chunks() {
    let _guard = crate::lock_state::ENV_LOCK.lock().await;
    unlocked();
    for (files, body) in [
        (vec![], json!([])),
        (
            vec![file("webpage", "paper", Some("Research"), 1)],
            json!([chunk("paper", 0, "wrong kind")]),
        ),
        (
            vec![file("file", "paper", Some("Other"), 1)],
            json!([chunk("paper", 0, "wrong space")]),
        ),
        (
            vec![file("file", "paper", Some("Research"), 1)],
            json!([chunk("different", 0, "wrong id")]),
        ),
        (
            vec![file("file", "paper", Some("Research"), 2)],
            json!([chunk("paper", 0, "partial")]),
        ),
        (
            vec![file("file", "paper", Some("Research"), 1)],
            json!([chunk("paper", 1, "missing zero")]),
        ),
        (
            vec![
                file("file", "paper", Some("Research"), 1),
                file("memory", "paper", Some("Research"), 1),
            ],
            json!([chunk("paper", 0, "collision")]),
        ),
    ] {
        let mock = MockServer::start().await;
        inventory(&mock, files, "Research").await;
        chunks(&mock, "/api/chunks/paper", body, "Research").await;
        let result = server(&mock.uri())
            .read_source_impl(params("paper", "Research"))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{result:?}");
        assert!(result.structured_content.is_none());
    }
}

#[tokio::test]
async fn read_source_requires_scope_ack_on_inventory_and_chunks_and_final_inventory() {
    let _guard = crate::lock_state::ENV_LOCK.lock().await;
    unlocked();
    for missing in ["inventory", "chunks", "final"] {
        let mock = MockServer::start().await;
        let files = json!({"files":[file("file","paper",Some("Research"),1)]});
        Mock::given(path("/api/indexed-files"))
            .respond_with(if missing == "inventory" {
                ResponseTemplate::new(200).set_body_json(files.clone())
            } else {
                scoped(files.clone(), "Research")
            })
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&mock)
            .await;
        Mock::given(path("/api/indexed-files"))
            .respond_with(if missing == "final" {
                ResponseTemplate::new(200)
                    .insert_header("x-wenlan-source-scope", "Wrong")
                    .set_body_json(files)
            } else {
                scoped(files, "Research")
            })
            .with_priority(2)
            .mount(&mock)
            .await;
        Mock::given(path("/api/chunks/paper"))
            .respond_with(if missing == "chunks" {
                ResponseTemplate::new(200).set_body_json(json!([chunk(
                    "paper",
                    0,
                    "PRIVATE_SENTINEL"
                )]))
            } else {
                scoped(json!([chunk("paper", 0, "PRIVATE_SENTINEL")]), "Research")
            })
            .mount(&mock)
            .await;
        let result = server(&mock.uri())
            .read_source_impl(params("paper", "Research"))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "{missing}: {result:?}");
        assert!(!format!("{result:?}").contains("PRIVATE_SENTINEL"));
    }
}

#[tokio::test]
async fn read_source_rejects_generation_changes_and_missing_chunks() {
    let _guard = crate::lock_state::ENV_LOCK.lock().await;
    unlocked();
    for missing in [false, true] {
        let mock = MockServer::start().await;
        Mock::given(path("/api/indexed-files"))
            .respond_with(scoped(
                json!({"files":[file("file","paper",Some("Research"),1)]}),
                "Research",
            ))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&mock)
            .await;
        let mut changed = file("file", "paper", Some("Research"), 1);
        changed["last_modified"] = json!(11);
        Mock::given(path("/api/indexed-files"))
            .respond_with(scoped(json!({"files":[changed]}), "Research"))
            .with_priority(2)
            .mount(&mock)
            .await;
        Mock::given(path("/api/chunks/paper"))
            .respond_with(if missing {
                ResponseTemplate::new(404)
            } else {
                scoped(json!([chunk("paper", 0, "old text")]), "Research")
            })
            .mount(&mock)
            .await;
        assert_eq!(
            server(&mock.uri())
                .read_source_impl(params("paper", "Research"))
                .await
                .unwrap()
                .is_error,
            Some(true)
        );
    }
}

#[tokio::test]
async fn read_source_strict_lock_and_invalid_requests_make_no_http_calls() {
    let _guard = crate::lock_state::ENV_LOCK.lock().await;
    let mock = MockServer::start().await;
    std::env::set_var("WENLAN_SPACE", "Pinned");
    crate::lock_state::init_from_env();
    let reader = server(&mock.uri());
    let mismatch = reader
        .read_source_impl(params("paper", "Research"))
        .await
        .unwrap();
    unlocked();
    assert_eq!(mismatch.is_error, Some(true));
    assert!(format!("{mismatch:?}").contains("WENLAN_SPACE"));
    for request in [
        params("paper", ""),
        params("paper", "*"),
        params("", "Research"),
        ReadSourceParams {
            limit: Some(0),
            ..params("paper", "Research")
        },
        ReadSourceParams {
            limit: Some(16001),
            ..params("paper", "Research")
        },
    ] {
        assert_eq!(
            reader.read_source_impl(request).await.unwrap().is_error,
            Some(true)
        );
    }
    assert!(mock.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn read_source_webpage_uses_exact_kind_route_and_encoded_id() {
    let _guard = crate::lock_state::ENV_LOCK.lock().await;
    unlocked();
    let mock = MockServer::start().await;
    let id = "https://example.org/a?q=x&lang=zh#part";
    inventory(
        &mock,
        vec![
            file("webpage", id, Some("Research"), 1),
            file("file", id, Some("Research"), 1),
        ],
        "Research",
    )
    .await;
    let route = format!("/api/webpage-chunks/{}", url_encode_simple(id));
    chunks(
        &mock,
        &route,
        json!([chunk(id, 0, "Saved webpage text")]),
        "Research",
    )
    .await;
    let mut request = params(id, "Research");
    request.source = ReadSourceKind::Webpage;
    let result = output(server(&mock.uri()).read_source_impl(request).await.unwrap());
    assert_eq!(result["source"], "webpage");
    assert_eq!(result["source_id"], id);
    assert_eq!(result["text"], "Saved webpage text");
}

#[tokio::test]
async fn read_source_catalog_and_direct_dispatch_are_standard_local_only() {
    let _guard = crate::lock_state::ENV_LOCK.lock().await;
    unlocked();
    for transport in [TransportMode::Stdio, TransportMode::Http] {
        for profile in [
            ToolProfile::Standard,
            ToolProfile::QueryOnly,
            ToolProfile::WebWrite,
        ] {
            let reader = WenlanMcpServer::new(
                WenlanClient::new("http://127.0.0.1:1".into()),
                transport.clone(),
                "test".into(),
                None,
            )
            .with_tool_profile(profile);
            let allowed = transport == TransportMode::Stdio && profile == ToolProfile::Standard;
            assert_eq!(
                reader
                    .visible_tools()
                    .iter()
                    .any(|tool| tool.name == "read_source"),
                allowed
            );
            if !allowed {
                assert_eq!(
                    reader
                        .read_source_impl(params("paper", "Research"))
                        .await
                        .unwrap()
                        .is_error,
                    Some(true)
                );
                assert!(
                    reader.local_only_refusal("read_source").is_some()
                        || reader.query_only_refusal("read_source").is_some()
                        || reader.web_write_refusal("read_source").is_some()
                );
            }
        }
    }
    let tool = server("http://127.0.0.1:1")
        .visible_tools()
        .into_iter()
        .find(|tool| tool.name == "read_source")
        .unwrap();
    let gated = super::super::strip_space_from_tool_schema(tool);
    assert!(gated.input_schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "space"));
}

#[tokio::test]
async fn read_source_real_scoped_router_returns_named_and_uncategorized_spaces() {
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use wenlan_core::{db::MemoryDB, events::NoopEmitter};
    let _guard = crate::lock_state::ENV_LOCK.lock().await;
    unlocked();
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(
        MemoryDB::new(&tmp.path().join("db"), Arc::new(NoopEmitter))
            .await
            .unwrap(),
    );
    let name = "研究 & R+D";
    let registered = db.create_space(name, None, false).await.unwrap();
    assert_ne!(registered.id, name);
    db.upsert_documents_without_embeddings(vec![
        wenlan_types::RawDocument {
            source: "file".into(),
            source_id: "folder/中文?x#y".into(),
            title: "研究來源".into(),
            content: "完整的中文來源。".into(),
            space: Some(name.into()),
            ..Default::default()
        },
        wenlan_types::RawDocument {
            source: "file".into(),
            source_id: "unfiled-paper".into(),
            title: "未分類".into(),
            content: "尚未分類的來源。".into(),
            ..Default::default()
        },
    ])
    .await
    .unwrap();
    let state = Arc::new(RwLock::new(wenlan_server::state::ServerState {
        db: Some(db),
        ..Default::default()
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = wenlan_server::router::build_router(state);
    let serving = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .unwrap()
    });
    let reader = server(&url);
    let named = reader
        .read_source_impl(params("folder/中文?x#y", name))
        .await
        .unwrap();
    let unfiled = reader
        .read_source_impl(params("unfiled-paper", "uncategorized"))
        .await
        .unwrap();
    let wrong = reader
        .read_source_impl(params("folder/中文?x#y", "uncategorized"))
        .await
        .unwrap();
    std::env::set_var("WENLAN_SPACE", name);
    crate::lock_state::init_from_env();
    let pinned_unicode = reader
        .read_source_impl(params("folder/中文?x#y", name))
        .await
        .unwrap();
    unlocked();
    serving.abort();
    let _ = serving.await;
    assert_eq!(output(named)["text"], "完整的中文來源。");
    assert!(output(unfiled)["metadata"]["space"].is_null());
    assert_eq!(wrong.is_error, Some(true));
    assert_eq!(output(pinned_unicode)["text"], "完整的中文來源。");
}
