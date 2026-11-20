// SPDX-License-Identifier: AGPL-3.0-only

use super::{secret, KIND_HEADER, MEMBER_HEADER, SECRET_HEADER};
use crate::router::{build_router, AppRouter};
use crate::state::ServerState;
use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;
use wenlan_core::{db::MemoryDB, events::NoopEmitter, sources::RawDocument};

const SPACE: &str = "Design";
const AGENT: &str = "privacy-test";

async fn request(
    app: &AppRouter,
    method: Method,
    path: &str,
    body: Value,
    forwarded: bool,
) -> (StatusCode, Value) {
    request_as(app, method, path, body, forwarded.then_some("reader")).await
}

async fn request_as(
    app: &AppRouter,
    method: Method,
    path: &str,
    body: Value,
    member: Option<&str>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("x-agent-name", AGENT)
        .header(crate::space_header::HEADER_NAME, SPACE);
    if let Some(member) = member {
        req = req
            .header(SECRET_HEADER, secret())
            .header(MEMBER_HEADER, member)
            .header(KIND_HEADER, "person");
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn post_as_member(app: &AppRouter, path: &str, body: Value) -> Value {
    let (status, body) = request(app, Method::POST, path, body, true).await;
    assert_eq!(status, StatusCode::OK, "{path}");
    body
}

async fn create_member_draft(app: &AppRouter, title: &str, content: &str) -> Value {
    post_as_member(
        app,
        "/api/pages/drafts",
        json!({"draft_id": wenlan_core::pages::new_page_id(), "title": title, "content": content}),
    )
    .await
}

async fn fixture() -> (AppRouter, Arc<MemoryDB>, tempfile::TempDir) {
    let scratch = tempfile::tempdir().unwrap();
    let db = Arc::new(
        MemoryDB::new(scratch.path(), Arc::new(NoopEmitter))
            .await
            .unwrap(),
    );
    db.create_space(SPACE, None, false).await.unwrap();
    db.apply_brief_update(
        &serde_json::from_value(json!({
            "space": SPACE,
            "caller_id": "host",
            "operation_id": "public-brief",
            "mutations": [{"kind": "add", "text": "Review the design", "state": "active"}]
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    let state = ServerState {
        db: Some(db.clone()),
        ..ServerState::default()
    }
    .with_brief_status_root(scratch.path().join("status"));
    let app = build_router(Arc::new(RwLock::new(state)));
    (app, db, scratch)
}

#[tokio::test]
async fn hosted_reads_skip_private_retrieval_and_library_onboarding() {
    let (app, db, _scratch) = fixture().await;
    // The empty library makes it safe to check that recall never enters the
    // private search bookkeeping, and Brief never computes local material counts.
    let recall = post_as_member(&app, "/api/memory/search", json!({"query": "design"})).await;
    let brief = post_as_member(&app, "/api/brief", json!({"topic": "design"})).await;
    assert_eq!(recall["results"], json!([]));
    assert_eq!(brief["brief"]["active"][0]["text"], "Review the design");
    let searches = db.list_agent_activity(10, Some(AGENT), None).await.unwrap();
    assert_eq!(
        (searches.len(), brief.get("getting_started").is_some()),
        (0, false),
        "a hosted read entered private search bookkeeping or exposed library onboarding"
    );
}

#[tokio::test]
async fn hosted_readers_keep_shared_pages_without_private_memory_data() {
    const PRIVATE_ID: &str = "fixture-private-evidence";
    const PRIVATE_TEXT: &str = "design citrinefixtureproof";
    const SHARED_TITLE: &str = "Design meeting";
    const SHARED_TEXT: &str = "The design meeting starts Tuesday. [1]";
    let (app, db, _scratch) = fixture().await;
    db.upsert_documents(vec![RawDocument {
        source: "memory".into(),
        source_id: PRIVATE_ID.into(),
        title: "Fixture memory".into(),
        content: PRIVATE_TEXT.into(),
        space: Some(SPACE.into()),
        confirmed: Some(true),
        ..Default::default()
    }])
    .await
    .unwrap();
    let page = wenlan_core::post_write::create_page_with_floor(
        &db,
        wenlan_types::requests::CreateConceptRequest {
            title: SHARED_TITLE.into(),
            content: SHARED_TEXT.into(),
            summary: None,
            entity_id: None,
            space: Some(SPACE.to_string()).into(),
            source_memory_ids: vec![PRIVATE_ID.into()],
            creation_kind: Some("distilled".into()),
            workspace: Some(SPACE.into()),
        },
        AGENT,
        None,
        1,
    )
    .await
    .unwrap();
    let proof = json!([{
        "occurrence": 1, "marker": 1, "source_kind": "memory",
        "locator": PRIVATE_ID, "score": 1.0, "status": "verified",
    }]);
    let history = json!([{
        "version": 1, "at": 1, "edited_by": "agent_refresh",
        "incoming_source_ids": [PRIVATE_ID], "sources_added": 1,
    }]);
    db.set_page_citations_with_changelog(&page.id, Some(&proof.to_string()), &history.to_string())
        .await
        .unwrap();

    let query = json!({"query": "design"});
    let (status, local) = request(
        &app,
        Method::POST,
        "/api/memory/search",
        query.clone(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(local["results"].to_string().contains(PRIVATE_TEXT));
    let source_path = format!("/api/pages/{}/sources", page.id);
    let (status, local) = request(&app, Method::GET, &source_path, Value::Null, false).await;
    assert_eq!(status, StatusCode::OK);
    assert!(local.to_string().contains(PRIVATE_TEXT));

    for rerank in [false, true] {
        let recall = post_as_member(
            &app,
            "/api/memory/search",
            json!({"query": "design", "rerank": rerank}),
        )
        .await;
        assert_eq!(recall["results"], json!([]));
        assert_eq!(recall["supplemental_pages"][0]["content"], SHARED_TEXT);
    }
    let brief = post_as_member(&app, "/api/brief", json!({"topic": "design"})).await;
    assert_eq!(
        brief["related_context"]["results"][0]["content"],
        SHARED_TEXT
    );
    assert_eq!(
        brief["related_context"]["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(brief.get("getting_started").is_none());

    let (_, sources) = request(&app, Method::GET, &source_path, Value::Null, true).await;
    assert_eq!(sources, json!([]));
    let consumers = [
        (Method::GET, format!("/api/pages/{}", page.id), SHARED_TEXT),
        (
            Method::GET,
            format!("/api/pages/{}/revisions", page.id),
            "agent_refresh",
        ),
        (Method::GET, "/api/pages".to_string(), SHARED_TITLE),
        (Method::POST, "/api/pages/search".to_string(), SHARED_TITLE),
        (
            Method::GET,
            "/api/pages/recent?since_ms=1".to_string(),
            SHARED_TITLE,
        ),
        (
            Method::GET,
            "/api/pages/recent-changes".to_string(),
            SHARED_TITLE,
        ),
        (Method::GET, "/api/memory/graph".to_string(), SHARED_TITLE),
    ];
    for (method, path, expected) in consumers {
        let (status, body) = request(&app, method, &path, query.clone(), true).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        let wire = body.to_string();
        assert!(wire.contains(expected), "{path} lost the shared page");
        assert!(
            !wire.contains(PRIVATE_ID) && !wire.contains(PRIVATE_TEXT),
            "{path}"
        );
    }
    let (status, counts) = request(&app, Method::GET, "/api/spaces", Value::Null, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(counts[0]["name"], SPACE);
    assert_eq!(counts[0]["memory_count"], 0);
    assert_eq!(counts[0]["entity_count"], 0);
    let (status, rules) = request(&app, Method::GET, "/api/skills/rules", Value::Null, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rules["space_name"], SPACE);

    let (status, _) = request(
        &app,
        Method::POST,
        "/api/pages",
        json!({"title": "Team update", "content": SHARED_TEXT, "source_memory_ids": [PRIVATE_ID]}),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let written = post_as_member(
        &app,
        "/api/pages",
        json!({"title": "Team update", "content": "Publish the new meeting agenda."}),
    )
    .await;
    assert!(written["id"].as_str().is_some());
}

#[tokio::test]
async fn hosted_draft_publish_requires_a_real_publication_to_replay() {
    let (app, db, _scratch) = fixture().await;
    let page = post_as_member(
        &app,
        "/api/pages",
        json!({"title": "Team note", "content": "Initial text."}),
    )
    .await;
    let id = page["id"].as_str().unwrap();
    db.update_page_content(id, "Edited text.", &[], "manual_edit")
        .await
        .unwrap();
    let version = db.get_page(id).await.unwrap().unwrap().version;
    assert!(version > 1);
    let (status, _) = request(
        &app,
        Method::POST,
        &format!("/api/pages/drafts/{id}/publish"),
        json!({"expected_version": version - 1}),
        true,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "ordinary edit is not a publication"
    );
}

#[tokio::test]
async fn hosted_draft_publish_conflict_only_identifies_shared_pages() {
    let (app, db, _scratch) = fixture().await;
    for (title, hidden) in [("How I work", true), ("Team conventions", false)] {
        let page = post_as_member(
            &app,
            "/api/pages",
            json!({"title": title, "content": "Fictional notes."}),
        )
        .await;
        let id = page["id"].as_str().unwrap();
        if hidden {
            let space = db.get_space(SPACE).await.unwrap().unwrap();
            let state = serde_json::from_value(json!({
                "space_id": space.id, "space_name": SPACE, "status": "accepted", "rules_page_id": id
            }))
            .unwrap();
            db.set_space_rules_state(&state).await.unwrap();
        }
        let (status, _) = request(
            &app,
            Method::GET,
            &format!("/api/pages/{id}"),
            Value::Null,
            true,
        )
        .await;
        assert_eq!(
            status,
            if hidden {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::OK
            }
        );
        let draft = create_member_draft(&app, &title.to_uppercase(), "Draft wording.").await;
        let draft_id = draft["page"]["id"].as_str().unwrap();
        let (status, conflict) = request(
            &app,
            Method::POST,
            &format!("/api/pages/drafts/{draft_id}/publish"),
            json!({"expected_version": draft["page"]["version"]}),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(conflict["code"], "page_title_conflict");
        if hidden {
            assert!(
                conflict.get("existing_page_id").is_none()
                    && conflict.get("existing_page_title").is_none(),
                "a hidden page's identity appeared in a title conflict: {conflict}"
            );
        } else {
            assert_eq!(conflict["existing_page_id"], id);
            assert_eq!(conflict["existing_page_title"], title);
        }
    }
}

#[tokio::test]
async fn hosted_draft_publish_replay_omits_private_provenance() {
    const FIXTURE_LOCATOR: &str = "fixture-unpublished-locator";
    let (app, db, _scratch) = fixture().await;
    let created = create_member_draft(&app, "Team draft", "New text.").await;
    let draft: wenlan_core::pages::Page = serde_json::from_value(created["page"].clone()).unwrap();
    let path = format!("/api/pages/drafts/{}/publish", draft.id);
    let publish = json!({"expected_version": draft.version});
    let published = post_as_member(&app, &path, publish.clone()).await;
    assert_eq!(published["page"]["status"], "active");
    let citations = json!([{
        "occurrence": 1, "marker": 1, "source_kind": "memory",
        "locator": FIXTURE_LOCATOR, "score": 1.0, "status": "verified",
    }]);
    let changelog = json!([{
        "version": draft.version + 1, "at": 1, "edited_by": "agent_refresh",
        "incoming_source_ids": [FIXTURE_LOCATOR], "sources_added": 1,
    }]);
    db.set_page_citations_with_changelog(
        &draft.id,
        Some(&citations.to_string()),
        &changelog.to_string(),
    )
    .await
    .unwrap();
    assert!(db
        .get_page(&draft.id)
        .await
        .unwrap()
        .unwrap()
        .changelog
        .is_some_and(|history| history.contains(FIXTURE_LOCATOR)));
    let replayed = post_as_member(&app, &path, publish).await;
    assert_eq!(replayed["page"]["version"], published["page"]["version"]);
    assert!(
        !replayed.to_string().contains(FIXTURE_LOCATOR),
        "publication response must omit private provenance"
    );
}

#[tokio::test]
async fn hosted_draft_publish_requires_the_draft_owner() {
    let (app, db, _scratch) = fixture().await;
    let local = db
        .create_page_draft("Local draft", "Private text.", Some(SPACE), None)
        .await
        .unwrap();
    let created = create_member_draft(&app, "Member draft", "Work in progress.").await;
    let owned_id = created["page"]["id"].as_str().unwrap();
    let mut failures = Vec::new();
    for id in [
        local.id.as_str(),
        owned_id,
        "page_00000000-0000-4000-8000-000000000000",
    ] {
        let (status, body) = request_as(
            &app,
            Method::POST,
            &format!("/api/pages/drafts/{id}/publish"),
            json!({"expected_version": 1}),
            Some("another-writer"),
        )
        .await;
        if status != StatusCode::NOT_FOUND || body["code"] != "page_draft_not_found" {
            failures.push((id, status, body["code"].clone()));
        }
    }
    assert!(
        failures.is_empty(),
        "a writer reached a draft they could not read: {failures:?}"
    );
    assert_eq!(
        db.get_page(&local.id).await.unwrap().unwrap().status,
        "draft"
    );
    let published = post_as_member(
        &app,
        &format!("/api/pages/drafts/{owned_id}/publish"),
        json!({"expected_version": 1}),
    )
    .await;
    assert_eq!(published["page"]["status"], "active");
}

#[tokio::test]
async fn hosted_readers_hide_unpublished_drafts() {
    let (app, db, _scratch) = fixture().await;
    let draft = db
        .create_page_draft("Team draft", "Work in progress.", Some(SPACE), None)
        .await
        .unwrap();
    let path = format!("/api/pages/{}", draft.id);
    let (host_status, host) = request(&app, Method::GET, &path, Value::Null, false).await;
    assert_eq!(host_status, StatusCode::OK);
    assert_eq!(host["page"]["status"], "draft");
    let (list_status, list) = request(
        &app,
        Method::GET,
        "/api/pages?status=draft",
        Value::Null,
        true,
    )
    .await;
    let (read_status, _) = request(&app, Method::GET, &path, Value::Null, true).await;
    assert_eq!(list_status, StatusCode::OK);
    assert_eq!(
        (list["pages"].as_array().unwrap().is_empty(), read_status),
        (true, StatusCode::NOT_FOUND),
        "drafts must be absent from hosted listing and by-id reads"
    );
}

#[tokio::test]
async fn hosted_basis_responses_drop_targets_outside_the_grant() {
    use wenlan_core::read_scope::ReadScope;
    let (app, db, _scratch) = fixture().await;
    let mut pages = Vec::new();
    for title in ["Citing note", "Shared basis", "Basis moved out"] {
        let page = post_as_member(
            &app,
            "/api/pages",
            json!({"title": title, "content": format!("{title}.")}),
        )
        .await;
        pages.push(
            db.get_page(page["id"].as_str().unwrap())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    let [citer, shared, moved] = pages.as_slice() else {
        unreachable!()
    };
    for basis in [shared, moved] {
        db.mark_page_basis(&citer.id, &basis.id, basis.version, AGENT)
            .await
            .unwrap();
    }
    db.create_space("Private", None, false).await.unwrap();
    db.set_page_space(
        &moved.id,
        Some("Private"),
        moved.version,
        &ReadScope::Global,
    )
    .await
    .unwrap();
    let path = format!("/api/pages/{}", citer.id);
    let (status, local) = request(&app, Method::GET, &path, Value::Null, false).await;
    assert_eq!(status, StatusCode::OK);
    assert!(local["basis"].to_string().contains(&moved.id));
    let (status, hosted) = request(&app, Method::GET, &path, Value::Null, true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !hosted.to_string().contains(&moved.id),
        "a hosted basis reference disclosed an out-of-grant page"
    );
    assert!(hosted["basis"].to_string().contains(&shared.id));
}

#[tokio::test]
async fn hosted_search_omits_documents_prepared_from_private_chunks() {
    use wenlan_core::document_enrichment::{
        run_document_enrichment_with_profile, OkfSourceProfile,
    };
    use wenlan_core::prompts::PromptRegistry;
    use wenlan_core::shared_space::version::sha256_hex;

    const IMPORT_MARKER: &str = "fixturezirconimport";
    let (app, db, scratch) = fixture().await;
    let path = scratch.path().join("import.md");
    let content = format!(
        "---\ntype: concept\ntitle: Specimen notes\ndescription: Import fixture.\n---\n\n+         # Specimen notes\n\nThe fictional sample is labeled {IMPORT_MARKER}.\n\n{}",
        "The sample was cataloged in the laboratory after the inspection.\n\n".repeat(40)
    );
    std::fs::write(&path, &content).unwrap();
    let profile = OkfSourceProfile {
        bundle_root: scratch.path().into(),
        space: Some(SPACE.into()),
    };
    db.enqueue_document(
        "fixture-okf",
        path.to_str().unwrap(),
        Some(&sha256_hex(content.as_bytes())),
    )
    .await
    .unwrap();
    let entry = db.claim_next_pending().await.unwrap().unwrap();
    let outcome = run_document_enrichment_with_profile(
        &db,
        &entry,
        None,
        None,
        &PromptRegistry::default(),
        Some(&profile),
    )
    .await;
    let page = db.get_page(&outcome.page_id).await.unwrap().unwrap();
    assert_eq!(page.creation_kind, "source");
    assert_eq!(page.space.as_deref(), Some(SPACE));
    assert!(!outcome.chunk_ids.is_empty() && page.content.contains(IMPORT_MARKER));

    let path = format!("/api/pages/{}", page.id);
    let (status, local) = request(&app, Method::GET, &path, Value::Null, false).await;
    assert_eq!(status, StatusCode::OK);
    assert!(local["page"]["content"]
        .as_str()
        .unwrap()
        .contains(IMPORT_MARKER));
    let search = json!({"query": "Specimen notes"});
    let hosted = post_as_member(&app, "/api/pages/search", search.clone()).await;
    assert!(
        !hosted.to_string().contains(IMPORT_MARKER),
        "hosted page search returned text copied from private chunks"
    );
    let (status, _) = request(&app, Method::GET, &path, Value::Null, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    for (route, body) in [
        ("/api/memory/search", search),
        ("/api/brief", json!({"topic": "Specimen notes"})),
    ] {
        let hosted = post_as_member(&app, route, body).await.to_string();
        assert!(!hosted.contains(IMPORT_MARKER) && !hosted.contains(&page.id));
    }
}

#[tokio::test]
async fn hosted_revision_history_omits_host_prompts_and_previous_bodies() {
    const HOST_PROMPT: &str = "fixture historical request";
    const OLD_BODY: &str = "fixture historical prose";
    let (app, db, _scratch) = fixture().await;
    let created = post_as_member(
        &app,
        "/api/pages",
        json!({"title": "Published note", "content": OLD_BODY}),
    )
    .await;
    let id = created["id"].as_str().unwrap();
    db.update_page_content(id, "Published replacement prose.", &[], "manual_edit")
        .await
        .unwrap();
    let version = db.get_page(id).await.unwrap().unwrap().version;
    let history = json!([
        {"version": version - 1, "at": 1, "edited_by": "create"},
        {"version": version, "at": 2, "edited_by": "person_asked",
         "person_asked": HOST_PROMPT},
    ]);
    db.set_page_citations_with_changelog(id, None, &history.to_string())
        .await
        .unwrap();
    let path = format!("/api/pages/{id}/revisions?include=text");
    let (status, local) = request(&app, Method::GET, &path, Value::Null, false).await;
    assert_eq!(status, StatusCode::OK);
    let local = local.to_string();
    assert!(local.contains(HOST_PROMPT) && local.contains(OLD_BODY));
    for path in [format!("/api/pages/{id}/revisions"), path] {
        let (status, hosted) = request(&app, Method::GET, &path, Value::Null, true).await;
        assert_eq!(status, StatusCode::OK);
        let wire = hosted.to_string();
        assert!(wire.contains("person_asked"));
        assert!(
            !wire.contains(HOST_PROMPT) && !wire.contains(OLD_BODY),
            "hosted history must omit verbatim host input and previous bodies"
        );
    }
}

#[tokio::test]
async fn hosted_refresh_has_the_same_outcome_with_and_without_hidden_sources() {
    const SOURCE: &str = "fixture-refresh-evidence";
    const BEFORE: &str = "Design review starts Tuesday.";
    const AFTER: &str = "Design review starts Tuesday. Bring the checklist.";
    const PROPOSED: &str = "Design review starts Tuesday. Bring the final checklist.";
    let (app, db, _scratch) = fixture().await;
    db.upsert_documents(vec![RawDocument {
        source: "memory".into(),
        source_id: SOURCE.into(),
        title: "Fictional reminder".into(),
        content: BEFORE.into(),
        space: Some(SPACE.into()),
        confirmed: Some(true),
        ..Default::default()
    }])
    .await
    .unwrap();
    let mut outcomes = Vec::new();
    for (title, sources) in [
        ("Unlinked design note", vec![]),
        ("Linked design note", vec![SOURCE.to_string()]),
    ] {
        let page = wenlan_core::post_write::create_page_with_floor(
            &db,
            wenlan_types::requests::CreateConceptRequest {
                title: title.into(),
                content: BEFORE.into(),
                summary: None,
                entity_id: None,
                space: Some(SPACE.to_string()).into(),
                source_memory_ids: sources.clone(),
                creation_kind: Some("authored".into()),
                workspace: Some(SPACE.into()),
            },
            AGENT,
            None,
            1,
        )
        .await
        .unwrap();
        let id = page.id;
        let before = db.get_page(&id).await.unwrap().unwrap();
        let path = format!("/api/pages/{id}");
        let (status, _) = request(&app, Method::PUT, &path, json!({"content": AFTER}), true).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "a human page requires a version"
        );
        let (status, response) = request(
            &app,
            Method::PUT,
            &path,
            json!({"content": AFTER, "expected_version": before.version}),
            true,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "hidden sources must not refuse refresh"
        );
        assert!(!response.to_string().contains(SOURCE));
        outcomes.push(response);

        let after = db.get_page(&id).await.unwrap().unwrap();
        assert_eq!(after.version, before.version + 1);
        assert!(after.content.contains("Bring the checklist."));
        assert!(
            after.source_memory_ids == sources,
            "refresh dropped source links"
        );
        let evidence = db.get_page_evidence(&id).await.unwrap();
        assert_eq!(evidence.len(), sources.len());
        let (stale, _) = request(
            &app,
            Method::PUT,
            &path,
            json!({"content": AFTER, "expected_version": before.version}),
            true,
        )
        .await;
        assert_eq!(stale, StatusCode::CONFLICT);

        assert!(db.set_page_review_required(&id, true).await.unwrap());
        let (status, proposal) = request(
            &app,
            Method::PUT,
            &path,
            json!({
                "content": PROPOSED,
                "expected_version": after.version,
                "person_asked": "Use the final checklist."
            }),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(proposal["gated"], true);
        assert!(!proposal.to_string().contains(SOURCE));
        assert_eq!(
            db.get_page(&id).await.unwrap().unwrap().content,
            after.content
        );
        let card = proposal["revision_card_id"].as_str().unwrap();
        wenlan_core::post_write::accept_pending_revision_with_knowledge_path(
            &db, card, "human", None,
        )
        .await
        .unwrap();
        let accepted = db.get_page(&id).await.unwrap().unwrap();
        assert_eq!(accepted.content, PROPOSED);
        assert!(
            accepted.source_memory_ids == sources,
            "review dropped source links"
        );
        assert_eq!(
            db.get_page_evidence(&id).await.unwrap().len(),
            sources.len()
        );
    }
    assert!(
        outcomes[0] == outcomes[1],
        "one refresh response disclosed source presence"
    );
}

#[tokio::test]
async fn hosted_forwarding_does_not_admit_private_readers() {
    let (app, _, _scratch) = fixture().await;
    for (method, path) in [
        (Method::POST, "/api/search"),
        (Method::POST, "/api/context"),
        (Method::GET, "/api/memory/stats"),
        (Method::POST, "/api/memory/entities/list"),
        (Method::GET, "/api/memory/synthetic/detail"),
        (Method::GET, "/api/memory/by-ids"),
        (Method::GET, "/api/steep/worklist"),
    ] {
        let (status, _) = request(&app, method, path, Value::Null, true).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
}
