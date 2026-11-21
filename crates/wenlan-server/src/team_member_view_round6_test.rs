// SPDX-License-Identifier: Apache-2.0

use super::*;

/// A Design page whose body carries sentence citations the member never sees.
async fn seed_cited_page(db: &Arc<MemoryDB>, title: &str, body: &str, markers: u32) -> String {
    let id = create_folder_test_page(db, title, "authored").await.id;
    db.update_page_content(&id, body, &[], "agent_refresh")
        .await
        .unwrap();
    let page = db.get_page(&id).await.unwrap().unwrap();
    let citations: Vec<Value> = (1..=markers)
        .map(|n| {
            json!({
                "occurrence": n, "marker": n, "source_kind": "memory",
                "locator": format!("round6-private-{n}"), "score": 1.0,
                "status": "verified", "scope": "sentence",
            })
        })
        .collect();
    let changelog = json!([{
        "version": page.version, "at": 1, "edited_by": "agent_refresh", "agent": TOOL,
    }]);
    db.set_page_citations_with_changelog(
        &id,
        Some(&Value::from(citations).to_string()),
        &changelog.to_string(),
    )
    .await
    .unwrap();
    assert_eq!(
        db.get_page(&id).await.unwrap().unwrap().citations.len(),
        markers as usize
    );
    id
}

/// Reads the page as the member, edits the projection, and saves it.
async fn member_save(
    f: &Fixture,
    id: &str,
    edit: impl FnOnce(&str) -> String,
) -> (StatusCode, String) {
    let path = format!("/api/pages/{id}");
    let (status, reply) = forwarded(f, Method::GET, &path, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let reply: Value = serde_json::from_str(&reply).unwrap();
    let projected = reply["page"]["content"].as_str().unwrap().to_owned();
    let version = reply["page"]["version"].as_i64().unwrap();
    forwarded(
        f,
        Method::PUT,
        &path,
        json!({"expected_version": version, "content": edit(&projected)}),
    )
    .await
}

#[tokio::test]
async fn round6_member_edit_keeps_citations_on_repeated_sentences_in_unchanged_paragraphs() {
    let (mut f, db) = fixture_with_daemon().await;
    let root = f._dir.path().join("wiki");
    set_daemon(&mut f, &db, &root);
    let body = "Alpha context. Same claim.[1]\n\nIntro paragraph.\n\nBeta context. Same claim.[2]";
    let id = seed_cited_page(&db, "Round6 repeated claims", body, 2).await;
    let (status, reply) = member_save(&f, &id, |projected| {
        projected.replace("Intro paragraph.", "Intro paragraph, edited.")
    })
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let after = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(
        after.content,
        "Alpha context. Same claim.[1]\n\nIntro paragraph, edited.\n\nBeta context. Same claim.[2]"
    );
    assert_eq!(
        after
            .citations
            .iter()
            .map(|c| c.locator.as_str())
            .collect::<Vec<_>>(),
        ["round6-private-1", "round6-private-2"],
    );
}

#[tokio::test]
async fn round6_member_save_that_repeats_or_moves_cited_text_is_refused() {
    let (mut f, db) = fixture_with_daemon().await;
    let root = f._dir.path().join("wiki");
    set_daemon(&mut f, &db, &root);
    let body = "First claim.[1]\n\nSecond claim.[2]\n\nClosing words.";
    let cases: [(&str, fn(&str) -> String); 3] = [
        ("repeated", |p| {
            p.replace("First claim.", "First claim.\n\nFirst claim.")
        }),
        ("moved", |_| {
            "Second claim.\n\nFirst claim.\n\nClosing words.".into()
        }),
        ("repeated sentence", |p| {
            p.replace("Second claim.", "Second claim. Second claim.")
        }),
    ];
    for (n, (case, edit)) in cases.into_iter().enumerate() {
        let id = seed_cited_page(&db, &format!("Round6 cited {n}"), body, 2).await;
        let before = db.get_page(&id).await.unwrap().unwrap();
        let (status, reply) = member_save(&f, &id, edit).await;
        assert_eq!(status, StatusCode::CONFLICT, "{case}: {reply}");
        assert!(
            reply.contains("repeats or moves cited text"),
            "{case}: {reply}"
        );
        assert!(!reply.contains("round6-private"), "{case}: {reply}");
        let after = db.get_page(&id).await.unwrap().unwrap();
        assert_eq!(after.version, before.version, "{case}");
        assert_eq!(after.content, before.content, "{case}");
        assert_eq!(json!(after.citations), json!(before.citations), "{case}");
    }
}

/// Uploads a whole version of `page` as the team writer.
async fn writer_uploads(f: &Fixture, page: &str, title: &str, body: &str) {
    let mut file = wenlan_core::store::format::page::PageFile::new(title, body);
    file.id = Some(page.into());
    let bytes = render_test_version(&file, &f.writer.0.id);
    let name = version::version_file_name(&version::sha256_hex(bytes.as_bytes()));
    let (status, _) = call(
        &f.state,
        Method::PUT,
        &format!("/team/v1/spaces/Design/versions/{page}/{name}"),
        Some(&f.writer.1),
        bytes.into(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn round6_synced_member_create_sees_only_member_titles_and_skips_the_link_sweep() {
    let (f, db) = fixture_with_daemon().await;
    let (hidden, _) = seed_saved_rules(&db).await;
    let hidden_body = format!("{SAVED_RULE} See [[Round6 linked page]].");
    db.update_page_content(&hidden, &hidden_body, &[], "manual_edit")
        .await
        .unwrap();
    let orphans = |labels: Vec<(String, i64)>| {
        labels
            .iter()
            .any(|(label, _)| label.eq_ignore_ascii_case("Round6 linked page"))
    };
    assert!(orphans(db.list_orphan_link_labels(1).await.unwrap()));
    sync_hosted_test_folder(&f, &db).await;
    let same_title = "page_00000000-0000-4000-8000-000000006001";
    let linked = "page_00000000-0000-4000-8000-000000006002";
    writer_uploads(
        &f,
        same_title,
        "Rules I saved",
        "A member's team conventions.",
    )
    .await;
    writer_uploads(&f, linked, "Round6 linked page", "A member's linked page.").await;
    sync_hosted_test_folder(&f, &db).await;
    let created = db.get_page(same_title).await.unwrap().unwrap();
    assert_eq!(created.title, "Rules I saved", "a hidden title was taken");
    assert!(db.get_page(linked).await.unwrap().is_some());
    assert!(
        orphans(db.list_orphan_link_labels(1).await.unwrap()),
        "a member import resolved a hidden page's link"
    );
    assert_eq!(
        db.get_page(&hidden).await.unwrap().unwrap().content,
        hidden_body
    );
}

/// Archives and restores a Design page, syncing after each step, so the
/// folder holds a valid archive from this stay. Returns the archive's path.
async fn archive_and_restore(f: &Fixture, db: &Arc<MemoryDB>, id: &str) -> String {
    sync_hosted_test_folder(f, db).await;
    db.archive_page(id).await.unwrap();
    sync_hosted_test_folder(f, db).await;
    let page = db.get_page(id).await.unwrap().unwrap();
    db.restore_page(id, None, page.version, &ReadScope::Global)
        .await
        .unwrap();
    sync_hosted_test_folder(f, db).await;
    let archives = archive_paths(f, id);
    assert_eq!(archives.len(), 1, "{archives:?}");
    archives.into_iter().next().unwrap()
}

fn archive_paths(f: &Fixture, id: &str) -> Vec<String> {
    version::read_folder(&f.folder)
        .pages
        .get(id)
        .into_iter()
        .flatten()
        .filter(|version| version.meta.archived)
        .map(|version| {
            let name = version::version_file_name(&version.sha);
            std::fs::read_dir(f.folder.join("members"))
                .unwrap()
                .flatten()
                .map(|member| member.file_name().into_string().unwrap())
                .map(|member| format!("members/{member}/pages/{id}/{name}"))
                .find(|path| f.folder.join(path).is_file())
                .unwrap()
        })
        .collect()
}

async fn deleted_pages(f: &Fixture) -> Vec<String> {
    let (status, body) = call(
        &f.state,
        Method::GET,
        "/team/v1/spaces/Design/files",
        Some(&f.reader.1),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice::<TeamFileList>(&body)
        .unwrap()
        .deleted_pages
}

#[tokio::test]
async fn round6_deleting_a_reentered_page_withholds_its_first_stay_archive() {
    let (f, db) = fixture_with_daemon().await;
    let id = create_folder_test_page(&db, "Round6 reentered", "authored")
        .await
        .id;
    let first_stay_archive = archive_and_restore(&f, &db, &id).await;
    db.create_space("Personal", None, false).await.unwrap();
    for space in ["Personal", "Design"] {
        let page = db.get_page(&id).await.unwrap().unwrap();
        db.set_page_space(&id, Some(space), page.version, &ReadScope::Global)
            .await
            .unwrap();
    }
    db.delete_page(&id).await.unwrap();
    assert!(
        !listed_files(&f).await.contains(&first_stay_archive),
        "a first-stay archive was reauthorized by the deletion"
    );
    assert_eq!(
        download(&f, &first_stay_archive).await.0,
        StatusCode::NOT_FOUND
    );
    assert!(deleted_pages(&f).await.contains(&id));
}

#[tokio::test]
async fn round6_only_the_verified_final_archive_suppresses_a_deletion() {
    let (f, db) = fixture_with_daemon().await;
    let id = create_folder_test_page(&db, "Round6 archived twice", "authored")
        .await
        .id;
    let older_archive = archive_and_restore(&f, &db, &id).await;
    db.delete_page(&id).await.unwrap();
    sync_hosted_test_folder(&f, &db).await;
    let final_archive = archive_paths(&f, &id)
        .into_iter()
        .find(|path| path != &older_archive)
        .expect("the deletion wrote a final archive");
    let files = listed_files(&f).await;
    assert!(files.contains(&final_archive), "{files:?}");
    assert!(
        !files.contains(&older_archive),
        "an older archive is served"
    );
    assert!(!deleted_pages(&f).await.contains(&id));

    // Without that file the older archive still must not stand in for it:
    // either a newly verified final archive is served, or the deletion is.
    std::fs::remove_file(f.folder.join(&final_archive)).unwrap();
    sync_hosted_test_folder(&f, &db).await;
    let files = listed_files(&f).await;
    assert!(
        !files.contains(&older_archive),
        "an older archive is served"
    );
    assert!(!files.contains(&final_archive));
    assert_eq!(download(&f, &older_archive).await.0, StatusCode::NOT_FOUND);
    let served = files.iter().filter(|path| path.contains(&id)).count();
    assert!(served <= 1, "{files:?}");
    assert_eq!(
        served == 0,
        deleted_pages(&f).await.contains(&id),
        "{files:?}"
    );
}

/// A member refresh carrying a summary; the host moves the page to a
/// private Space and writes its own summary right after the body write.
async fn refresh_raced_by_a_move(f: &Fixture, db: &Arc<MemoryDB>, edit: bool) -> StatusCode {
    let title = format!("Round6 summary {edit}");
    let id = create_folder_test_page(db, &title, "authored").await.id;
    let _ = db.create_space("Personal", None, false).await;
    let path = format!("/api/pages/{id}");
    let (status, reply) = forwarded(f, Method::GET, &path, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let reply: Value = serde_json::from_str(&reply).unwrap();
    let mut content = reply["page"]["content"].as_str().unwrap().to_owned();
    if edit {
        content.push_str(" A member sentence.");
    }
    let version = reply["page"]["version"].as_i64().unwrap();
    let (at_seam, resume) = crate::page_routes::refresh_seam::pause(&id);
    let host = async {
        at_seam.await.unwrap();
        let page = db.get_page(&id).await.unwrap().unwrap();
        db.set_page_space(&id, Some("Personal"), page.version, &ReadScope::Global)
            .await
            .unwrap();
        db.update_page_summary(&id, Some("Host private summary"))
            .await
            .unwrap();
        resume.send(()).unwrap();
    };
    let member = forwarded(
        f,
        Method::PUT,
        &path,
        json!({"expected_version": version, "content": content, "summary": "Member summary"}),
    );
    let ((status, reply), ()) = tokio::join!(member, host);
    let after = db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(after.space.as_deref(), Some("Personal"));
    assert_eq!(
        after.summary.as_deref(),
        Some("Host private summary"),
        "edit={edit}: a member summary landed after the page left the Space ({status} {reply})"
    );
    status
}

#[tokio::test]
async fn round6_member_refresh_writes_summary_with_the_body_under_one_check() {
    let (f, db) = fixture_with_daemon().await;
    // The body and summary landed together while the page was shared.
    assert_eq!(refresh_raced_by_a_move(&f, &db, true).await, StatusCode::OK);
    // An unchanged body: the summary alone is checked where it is written.
    assert_ne!(
        refresh_raced_by_a_move(&f, &db, false).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn round6_member_cannot_write_a_page_whose_file_is_outside_the_space_folder() {
    let (mut f, db) = fixture_with_daemon().await;
    let root = f._dir.path().join("wiki");
    std::fs::create_dir_all(root.join("Design")).unwrap();
    std::fs::create_dir_all(root.join("Private ancestor")).unwrap();
    wenlan_core::space_folders::turn_on(&root).unwrap();
    let draft = db
        .create_page_draft_with_id_in_registered_space_and_folder(
            &wenlan_core::pages::new_page_id(),
            "Round6 legacy note",
            "Legacy text.",
            Some("Design"),
            "Private ancestor",
        )
        .await
        .unwrap();
    let wenlan_core::pages::PageDraftPublishOutcome::Published(page) = db
        .publish_page_draft(&draft.id, draft.version)
        .await
        .unwrap()
    else {
        panic!("publish fixture");
    };
    wenlan_core::export::knowledge::KnowledgeProjectionWrite::new(root.clone(), &db)
        .write_page_gated(&db, &page)
        .await
        .unwrap();
    let outside = |root: &std::path::Path| {
        std::fs::read_dir(root.join("Private ancestor"))
            .unwrap()
            .flatten()
            .map(|entry| std::fs::read_to_string(entry.path()).unwrap())
            .collect::<Vec<_>>()
    };
    let file_before = outside(&root);
    assert_eq!(file_before.len(), 1);
    set_daemon(&mut f, &db, &root);
    let path = format!("/api/pages/{}", page.id);
    // The member can read the page; only its writes are refused.
    let (status, reply) = forwarded(&f, Method::GET, &path, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let (status, reply) = forwarded(
        &f,
        Method::PUT,
        &path,
        json!({"expected_version": page.version, "content": "Legacy text. A member line."}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{reply}");
    assert!(reply.contains("outside the Space"), "{reply}");
    assert!(!reply.contains("Private ancestor"), "{reply}");
    let (status, reply) = forwarded(
        &f,
        Method::POST,
        &format!("{path}/rename"),
        json!({"expected_version": page.version, "title": "Round6 renamed"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{reply}");
    let after = db.get_page(&page.id).await.unwrap().unwrap();
    assert_eq!(after.version, page.version);
    assert_eq!(after.content, page.content);
    assert_eq!(after.title, page.title);
    assert_eq!(outside(&root), file_before);
}
