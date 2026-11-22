// SPDX-License-Identifier: Apache-2.0

use super::*;

/// Send each registered member write through its real route, with an
/// ineligible page in one page-ID slot at a time, and each auth route
/// without a credential. Browser calls go through `/team/v1/app`, hosted
/// MCP calls through the daemon as the MCP client sends them, and version
/// uploads through the team data route. Every denial must be refused and
/// leave the hidden page as it was.
#[tokio::test]
async fn round5_catalog_generates_member_authorization_cases() {
    use crate::team_actor::{may_write_pages, member_routes, page_parameters, write_target};

    let (f, db) = fixture_with_daemon().await;
    team::set_url(&f.state.team_path, Some(ORIGIN)).unwrap();
    let visible = create_folder_test_page(&db, "Visible catalog fixture", "authored").await;
    let (hidden, _) = seed_saved_rules(&db).await;
    let hidden_before = db.get_page(&hidden).await.unwrap().unwrap();
    let draft = db
        .create_registered_page_draft(
            "page_00000000-0000-4000-8000-000000005009",
            "Draft catalog fixture",
            "Synthetic draft.",
            Some("Design"),
            "",
            Some(&f.writer.0.id),
        )
        .await
        .unwrap();
    let scope = ReadScope::Space("Design".into());
    let forwarded_calls = member_routes().collect::<std::collections::BTreeSet<_>>();
    let data = member_data_route_catalog().collect::<std::collections::BTreeSet<_>>();
    let auth = auth_route_catalog().collect::<std::collections::BTreeSet<_>>();
    // One body for every write: each route reads the fields it knows.
    let body = json!({
        "expected_version": hidden_before.version, "version": hidden_before.version,
        "content": "Round5 canary text.", "title": "Round5 canary title",
        "space": "Design", "folder": "", "basis_version": hidden_before.version,
    });
    let mut page_write_routes = 0;
    let mut denied_cases = 0;
    let mut not_refused = Vec::new();
    for &(method, template) in forwarded_calls.iter().chain(&data) {
        let method = Method::from_bytes(method.as_bytes()).unwrap();
        let slots: Vec<_> = template
            .split('/')
            .enumerate()
            .filter_map(|(i, part)| matches!(part, "{id}" | "{basis_id}" | "{page}").then_some(i))
            .collect();
        let published = if template.contains("/drafts/") {
            &draft.id
        } else {
            &visible.id
        };
        let concrete = |hidden_slot| {
            template
                .split('/')
                .enumerate()
                .map(|(i, part)| match part {
                    "{id}" | "{basis_id}" | "{page}" if hidden_slot == Some(i) => hidden.as_str(),
                    "{id}" | "{basis_id}" | "{page}" => published.as_str(),
                    "{space}" => "Design",
                    "{name}" => "fixture.md",
                    "{proposal}" => "fixture",
                    part => part,
                })
                .collect::<Vec<_>>()
                .join("/")
        };
        let allowed_path = concrete(None);
        assert!(
            !allowed_path.contains(['{', '}']),
            "unhandled slot in {template}"
        );
        if slots.is_empty() || method == Method::GET {
            continue;
        }
        page_write_routes += 1;
        if !template.contains("/{page}/") {
            // The fixture page itself is eligible, so a refusal below is the
            // hidden page's.
            let target = write_target(&method, template, Some(&f.writer.0.id)).unwrap();
            assert!(
                may_write_pages(
                    &db,
                    &scope,
                    target,
                    page_parameters(template, &allowed_path)
                )
                .await
                .unwrap(),
                "{method} {template} should allow the eligible fixture",
            );
        }
        for slot in slots.iter().copied() {
            let denied_path = concrete(Some(slot));
            let mut replies = Vec::new();
            if template.contains("/{page}/") {
                let mut file = wenlan_core::store::format::page::PageFile::new(
                    &hidden_before.title,
                    "Round5 canary text.",
                );
                file.id = Some(hidden.clone());
                let bytes = version::render(
                    &file,
                    &version::VersionMeta {
                        stay: None,
                        parent: None,
                        merged: Vec::new(),
                        body_digest: version::body_digest(&file.body),
                        author: version::Author {
                            member: f.writer.0.id.clone(),
                            kind: version::AuthorKind::Person,
                            agent: None,
                        },
                        ts: "2099-01-01T00:00:00Z".into(),
                        archived: false,
                        locked: false,
                        proposal: false,
                        decides: None,
                        device: None,
                    },
                );
                let name = version::version_file_name(&version::sha256_hex(bytes.as_bytes()));
                let path = denied_path.replace("fixture.md", &name);
                let (status, reply) = call(
                    &f.state,
                    method.clone(),
                    &path,
                    Some(&f.writer.1),
                    bytes.into(),
                )
                .await;
                replies.push(("team data", status, String::from_utf8_lossy(&reply).into()));
            } else {
                let key = (method.as_str(), template);
                if crate::team_routes::is_member_app_call(&method, template) {
                    let (status, reply) =
                        browser_write(&f, method.clone(), &denied_path, body.clone()).await;
                    replies.push(("browser", status, reply));
                }
                if crate::team_actor::MCP_CALLS.contains(&key) {
                    let (status, reply) =
                        forwarded(&f, method.clone(), &denied_path, body.clone()).await;
                    replies.push(("hosted MCP", status, reply));
                }
            }
            assert!(!replies.is_empty(), "{method} {template} has no route");
            for (via, status, reply) in replies {
                if !matches!(
                    status,
                    StatusCode::FORBIDDEN | StatusCode::NOT_FOUND | StatusCode::CONFLICT
                ) {
                    not_refused.push(format!("{via} {method} {denied_path}: {status} {reply}"));
                }
                denied_cases += 1;
            }
        }
    }
    assert_eq!(
        not_refused,
        Vec::<String>::new(),
        "denials that were not refused"
    );
    // Auth routes take no page, so each is called once without a credential:
    // it must answer with a member response and change nothing.
    let router = team_router(f.state.clone());
    let mut untyped_auth = Vec::new();
    for &(method, template) in &auth {
        let path = template.replace("{provider}", "google");
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(method.as_bytes()).unwrap())
                    .uri(&path)
                    .header(header::ORIGIN, ORIGIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        if !crate::member_view::is_typed(&response) || status.is_server_error() {
            untyped_auth.push(format!("{method} {path}: {status}"));
        }
    }
    assert_eq!(untyped_auth, Vec::<String>::new(), "auth routes");
    assert!(!db
        .member_can_access_page(
            &hidden,
            &scope,
            wenlan_core::db::MemberPageTarget::Published
        )
        .await
        .unwrap());
    let hidden_after = db.get_page(&hidden).await.unwrap().unwrap();
    assert_eq!(hidden_after.version, hidden_before.version);
    assert_eq!(hidden_after.title, hidden_before.title);
    assert_eq!(hidden_after.content, hidden_before.content);
    assert_eq!(hidden_after.status, hidden_before.status);
    assert!(!version::read_folder(&f.folder).pages.contains_key(&hidden));
    assert!(!auth.is_empty());
    assert!(page_write_routes >= 12 && denied_cases > page_write_routes);
    eprintln!(
        "catalog inputs: {} forwarded, {} team data, {} auth; {page_write_routes} page writes, {denied_cases} route denials",
        forwarded_calls.len(), data.len(), auth.len(),
    );
}

#[tokio::test]
async fn round5_member_projection_omits_markers_without_a_map() {
    let (mut f, db) = fixture_with_daemon().await;
    let (id, _) = seed_code_and_private_citation(&mut f, &db).await;
    db.set_page_citations_with_changelog(&id, None, "[]")
        .await
        .unwrap();
    let (status, reply) =
        forwarded(&f, Method::GET, &format!("/api/pages/{id}"), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(reply.contains("Synthetic public prose."));
    assert!(
        !reply.contains(MARKER),
        "an unbound marker reached the member"
    );
}

#[tokio::test]
async fn round5_member_refresh_retains_citations_on_unchanged_sentences() {
    let (mut f, db) = fixture_with_daemon().await;
    let (id, _) = seed_code_and_private_citation(&mut f, &db).await;
    let before = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(before.citations.len(), 1);
    let path = format!("/api/pages/{id}");
    let (status, reply) = forwarded(&f, Method::GET, &path, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let reply: Value = serde_json::from_str(&reply).unwrap();
    let projected = reply["page"]["content"].as_str().unwrap();
    assert!(!projected.contains(MARKER));
    let (status, reply) = forwarded(
        &f,
        Method::PUT,
        &path,
        json!({"expected_version": before.version, "content": projected}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let unchanged = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(unchanged.version, before.version);
    assert_eq!(unchanged.content, before.content);
    assert_eq!(json!(unchanged.citations), json!(before.citations));
    let (status, reply) = forwarded(
        &f,
        Method::PUT,
        &path,
        json!({"expected_version": before.version, "content": format!("{projected} Add a public sentence.")}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let after = db.get_page(&id).await.unwrap().unwrap();
    assert!(after.content.contains("Add a public sentence."));
    assert!(after.content.contains(MARKER));
    assert_eq!(after.citations.len(), 1);
    assert_eq!(after.citations[0].locator, before.citations[0].locator);
}

async fn browser_write(
    f: &Fixture,
    method: Method,
    path: &str,
    data: Value,
) -> (StatusCode, String) {
    let cookie = f.state.sessions.create_fresh(&f.writer.0.id).unwrap();
    let request = Request::builder()
        .method(method)
        .uri(format!("/team/v1/app{path}"))
        .header(header::COOKIE, format!("__Host-wenlan-session={cookie}"))
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(data.to_string()))
        .unwrap();
    let response = team_router(f.state.clone()).oneshot(request).await.unwrap();
    (response.status(), response_text(response).await)
}

#[tokio::test]
async fn round5_member_heading_edit_skips_hidden_backlinks() {
    let (f, db) = fixture_with_daemon().await;
    team::set_url(&f.state.team_path, Some(ORIGIN)).unwrap();
    let target = create_folder_test_page(&db, "Shared index", "authored")
        .await
        .id;
    db.update_page_content(&target, "## Before\n\nPublic notes.", &[], "manual_edit")
        .await
        .unwrap();
    let (hidden, _) = seed_saved_rules(&db).await;
    let visible = create_folder_test_page(&db, "Shared reader", "authored")
        .await
        .id;
    for id in [&hidden, &visible] {
        db.update_page_content(id, "See [[Shared index#Before]].", &[], "manual_edit")
            .await
            .unwrap();
    }
    let before = db.get_page(&hidden).await.unwrap().unwrap();
    let version = db.get_page(&target).await.unwrap().unwrap().version;
    let (status, reply) = browser_write(
        &f,
        Method::POST,
        &format!("/api/memory/{target}/update-page"),
        json!({"content":"## After\n\nPublic notes.", "expected_version":version}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let after = db.get_page(&hidden).await.unwrap().unwrap();
    assert_eq!(after.content, before.content);
    assert_eq!(after.version, before.version);
    assert!(db
        .get_page(&visible)
        .await
        .unwrap()
        .unwrap()
        .content
        .contains("#After"));
}

#[tokio::test]
async fn round5_member_create_does_not_confirm_a_hidden_title() {
    let (f, db) = fixture_with_daemon().await;
    let (hidden, version) = seed_saved_rules(&db).await;
    for _ in 0..2 {
        let (status, reply) = forwarded(
            &f,
            Method::POST,
            "/api/pages",
            json!({"title":"Rules I saved","content":"Public team conventions.","space":"Design"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert!(!reply.contains(&hidden));
        assert_eq!(
            db.get_page(&hidden).await.unwrap().unwrap().version,
            version
        );
    }
}

#[tokio::test]
async fn round5_actorless_forwarding_lists_only_space_folders() {
    let (mut f, db) = fixture_with_daemon().await;
    let root = f._dir.path().join("wiki");
    std::fs::create_dir_all(root.join("Design").join("Shared")).unwrap();
    std::fs::create_dir_all(root.join("Private ancestor")).unwrap();
    wenlan_core::space_folders::turn_on(&root).unwrap();
    let draft = db
        .create_page_draft(
            "Public note",
            "Public text.",
            Some("Design"),
            Some("Private ancestor"),
        )
        .await
        .unwrap();
    let published = db
        .publish_page_draft(&draft.id, draft.version)
        .await
        .unwrap();
    let wenlan_core::pages::PageDraftPublishOutcome::Published(page) = published else {
        panic!("publish fixture");
    };
    wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(root.clone(), &db)
        .write_page_gated(&db, &page)
        .await
        .unwrap();
    assert!(std::fs::read_dir(root.join("Private ancestor"))
        .unwrap()
        .next()
        .is_some());
    set_daemon(&mut f, &db, &root);
    let (status, reply) = forwarded(&f, Method::GET, "/api/knowledge/folders", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!reply.contains("Private ancestor"), "{reply}");
    assert!(reply.contains("Design/Shared"), "{reply}");
}

#[tokio::test]
async fn round5_member_cannot_publish_a_legacy_draft_outside_its_space_folder() {
    let (mut f, db) = fixture_with_daemon().await;
    team::set_url(&f.state.team_path, Some(ORIGIN)).unwrap();
    let root = f._dir.path().join("wiki");
    let outside = root.join("Private ancestor");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::create_dir_all(root.join("Design")).unwrap();
    wenlan_core::space_folders::turn_on(&root).unwrap();
    set_daemon(&mut f, &db, &root);
    let draft = db
        .create_registered_page_draft(
            "page_00000000-0000-4000-8000-000000005001",
            "Draft note",
            "Public text.",
            Some("Design"),
            "Private ancestor",
            Some(&f.writer.0.id),
        )
        .await
        .unwrap();
    let (status, _) = browser_write(
        &f,
        Method::POST,
        &format!("/api/pages/drafts/{}/publish", draft.id),
        json!({"expected_version":draft.version}),
    )
    .await;
    assert!(
        status.is_client_error(),
        "unexpected publish status {status}"
    );
    assert_eq!(
        db.get_page(&draft.id).await.unwrap().unwrap().status,
        "draft"
    );
    assert!(std::fs::read_dir(outside).unwrap().next().is_none());
}

#[tokio::test]
async fn round5_snapshot_download_requires_the_current_space_stay() {
    for mode in [HostedMode::Legacy, HostedMode::Migrated] {
        snapshot_download_requires_the_current_space_stay(mode).await;
    }
}

async fn snapshot_download_requires_the_current_space_stay(mode: HostedMode) {
    let (f, db) = fixture_with_hosted_mode(mode).await;
    let id = create_folder_test_page(&db, "Team itinerary", "authored")
        .await
        .id;
    sync_hosted_test_folder(&f, &db).await;
    let previous = listed_files(&f)
        .await
        .into_iter()
        .find(|path| path.contains(&id))
        .unwrap();
    assert_eq!(download(&f, &previous).await.0, StatusCode::OK);
    db.create_space("Personal", None, false).await.unwrap();
    for space in ["Personal", "Design"] {
        let page = db.get_page(&id).await.unwrap().unwrap();
        db.set_page_space(&id, Some(space), page.version, &ReadScope::Global)
            .await
            .unwrap();
        db.update_page_content(
            &id,
            &format!("Latest {space} itinerary."),
            &[],
            "manual_edit",
        )
        .await
        .unwrap();
    }
    sync_hosted_test_folder(&f, &db).await;
    assert_eq!(
        download(&f, &previous).await.0,
        StatusCode::NOT_FOUND,
        "{mode:?}"
    );
    let inventory = legacy_file_list(&f).await;
    assert!(!inventory.files.contains(&previous), "{mode:?}");
    let current = inventory.files.iter().find(|path| path.contains(&id));
    match mode {
        HostedMode::Legacy => {
            let (status, body) = download(&f, current.unwrap()).await;
            assert_eq!(status, StatusCode::OK);
            assert!(String::from_utf8(body)
                .unwrap()
                .contains("Latest Design itinerary."));
        }
        HostedMode::Migrated => {
            assert!(current.is_none(), "a different Space stay was delivered");
            assert!(inventory.deleted_pages.contains(&id), "{inventory:?}");
        }
    }
}

#[tokio::test]
async fn round5_snapshot_basis_omits_a_pre_space_version() {
    for mode in [HostedMode::Legacy, HostedMode::Migrated] {
        snapshot_basis_omits_a_pre_space_version(mode).await;
    }
}

async fn snapshot_basis_omits_a_pre_space_version(mode: HostedMode) {
    let (f, db) = fixture_with_hosted_mode(mode).await;
    let target = seed_moved_page(&db).await;
    let citer = create_folder_test_page(&db, "Citing team note", "authored")
        .await
        .id;
    db.mark_page_basis(&citer, &target, 1, "fixture")
        .await
        .unwrap();
    sync_hosted_test_folder(&f, &db).await;
    let inventory = legacy_file_list(&f).await;
    let path = inventory.files.iter().find(|path| path.contains(&citer));
    match mode {
        HostedMode::Legacy => {
            let (status, bytes) = download(&f, path.unwrap()).await;
            assert_eq!(status, StatusCode::OK);
            assert!(!String::from_utf8(bytes)
                .unwrap()
                .contains(&format!("wenlan:{target}@1")));
        }
        HostedMode::Migrated => {
            assert!(path.is_none(), "a basis-bearing legacy file was served");
            assert!(inventory.deleted_pages.contains(&citer), "{inventory:?}");
        }
    }
}

#[tokio::test]
async fn round5_unsafe_legacy_deletion_still_reaches_offline_members() {
    for mode in [HostedMode::Legacy, HostedMode::Migrated] {
        unsafe_legacy_deletion_still_reaches_offline_members(mode).await;
    }
}

async fn unsafe_legacy_deletion_still_reaches_offline_members(mode: HostedMode) {
    let (f, db) = fixture_with_hosted_mode(mode).await;
    let id = create_folder_test_page(&db, "Old team note", "authored")
        .await
        .id;
    sync_hosted_test_folder(&f, &db).await;
    let active = listed_files(&f)
        .await
        .into_iter()
        .find(|path| path.contains(&id))
        .unwrap();
    let (status, bytes) = download(&f, &active).await;
    assert_eq!(status, StatusCode::OK);
    let mut archive = version::parse(active.rsplit('/').next().unwrap(), &bytes).unwrap();
    archive.meta.archived = true;
    archive.meta.parent = Some(archive.sha);
    archive.meta.author.agent = Some("Round5 private tool".into());
    let legacy = version::render(&archive.page, &archive.meta);
    let (sha, _) = version::write_version(&f.folder, "server", &id, &legacy).unwrap();
    let unsafe_path = format!(
        "members/server/pages/{id}/{}",
        version::version_file_name(&sha)
    );
    db.delete_page(&id).await.unwrap();
    std::fs::remove_file(f.folder.join(&active)).unwrap();
    sync_hosted_test_folder(&f, &db).await;
    assert_eq!(download(&f, &unsafe_path).await.0, StatusCode::NOT_FOUND);
    let manifest = legacy_file_list(&f).await;
    assert!(
        manifest.deleted_pages.contains(&id),
        "{mode:?}: {manifest:?}"
    );
    if mode == HostedMode::Migrated {
        assert_eq!(download(&f, &active).await.0, StatusCode::NOT_FOUND);
    }
}
