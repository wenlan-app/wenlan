// SPDX-License-Identifier: Apache-2.0

use super::*;
use serde_json::{json, Value};

async fn held_path(f: &Fixture, id: &str) -> String {
    let path = legacy_file_list(f)
        .await
        .files
        .into_iter()
        .find(|path| path.split('/').nth(3) == Some(id))
        .expect("a legacy member can get this version before going offline");
    assert_eq!(download(f, &path).await.0, StatusCode::OK);
    path
}

async fn download(f: &Fixture, path: &str) -> (StatusCode, Vec<u8>) {
    call(
        &f.state,
        Method::GET,
        &format!("/team/v1/spaces/Design/file?path={path}"),
        Some(&f.reader.1),
        vec![],
    )
    .await
}

async fn assert_removed_from_legacy(f: &Fixture, id: &str, path: &str) {
    let list = legacy_file_list(f).await;
    assert!(
        list.deleted_pages.iter().any(|page| page == id),
        "missing deletion for {id}: {list:?}"
    );
    assert!(!list
        .files
        .iter()
        .any(|path| path.split('/').nth(3) == Some(id)));
    assert_eq!(download(f, path).await.0, StatusCode::NOT_FOUND);
}

async fn library_page(f: &Fixture, id: &str) -> StatusCode {
    call(
        &f.state,
        Method::GET,
        &format!("/team/v1/library/pages/{id}"),
        Some(&f.reader.1),
        vec![],
    )
    .await
    .0
}

const ORIGIN: &str = "https://team.example.com";

fn browser_session(f: &Fixture, member: &Member) -> String {
    team::set_url(&f.state.team_path, Some(ORIGIN)).unwrap();
    f.state.sessions.create_fresh(&member.id).unwrap()
}

async fn review(f: &Fixture, cookie: &str, action: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::post(format!("/team/v1/library/{action}"))
        .header(header::COOKIE, format!("__Host-wenlan-session={cookie}"))
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = team_router(f.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let body = response_text(response).await;
    (
        status,
        serde_json::from_str(&body).unwrap_or(Value::String(body)),
    )
}

#[tokio::test]
async fn team_legacy_deletion_pending_before_upgrade() {
    let (mut f, db) = fixture_with_hosted_mode(HostedMode::Legacy).await;
    let id = create_folder_test_page(&db, "Deleted while offline", "authored")
        .await
        .id;
    sync_hosted_test_folder(&f, &db).await;
    let path = held_path(&f, &id).await;
    db.delete_page(&id).await.unwrap();
    sync_hosted_test_folder(&f, &db).await;
    f.set_hosted_mode(HostedMode::Migrated);

    assert_removed_from_legacy(&f, &id, &path).await;
    assert_eq!(library_page(&f, &id).await, StatusCode::NOT_FOUND);
    f.set_hosted_mode(HostedMode::Migrated);
    assert_removed_from_legacy(&f, &id, &path).await;
}

#[tokio::test]
async fn team_legacy_deletion_when_the_head_has_a_basis() {
    legacy_deletion_with_shared_reference(false).await;
}

#[tokio::test]
async fn team_legacy_deletion_when_the_head_has_a_citation() {
    legacy_deletion_with_shared_reference(true).await;
}

async fn legacy_deletion_with_shared_reference(citation: bool) {
    let (mut f, db) = fixture_with_hosted_mode(HostedMode::Legacy).await;
    let basis = create_folder_test_page(&db, "A shared basis", "authored").await;
    let id = create_folder_test_page(&db, "Depends on a shared page", "authored")
        .await
        .id;
    let basis_version = db.get_page(&basis.id).await.unwrap().unwrap().version;
    if citation {
        db.update_page_content(
            &id,
            "This relies on the shared page. [1]",
            &[],
            "manual_edit",
        )
        .await
        .unwrap();
        let citations = json!([{
            "occurrence": 1, "marker": 1, "source_kind": "memory", "locator": basis.id,
            "score": 1.0, "status": "verified", "cited_version": basis_version,
        }]);
        db.set_page_citations_with_changelog(&id, Some(&citations.to_string()), "[]")
            .await
            .unwrap();
    } else {
        db.mark_page_basis(&id, &basis.id, basis_version, "fixture")
            .await
            .unwrap();
    }
    sync_hosted_test_folder(&f, &db).await;
    let path = held_path(&f, &id).await;
    let version = version::parse(
        path.rsplit('/').next().unwrap(),
        &download(&f, &path).await.1,
    )
    .unwrap();
    assert!(if citation {
        !version.page.citations.is_empty()
    } else {
        !version.page.basis.is_empty()
    });
    f.set_hosted_mode(HostedMode::Migrated);

    assert_removed_from_legacy(&f, &id, &path).await;
    assert_eq!(library_page(&f, &id).await, StatusCode::OK);
    assert!(legacy_file_list(&f)
        .await
        .deleted_pages
        .iter()
        .all(|page| page != &basis.id));
}

#[tokio::test]
async fn team_legacy_deletion_when_the_heads_do_not_match() {
    let (mut f, db) = fixture_with_hosted_mode(HostedMode::Legacy).await;
    let id = create_folder_test_page(&db, "Unmatched edition", "authored")
        .await
        .id;
    sync_hosted_test_folder(&f, &db).await;
    let path = held_path(&f, &id).await;
    db.update_page_content(
        &id,
        "An edit not yet synced to the folder.",
        &[],
        "manual_edit",
    )
    .await
    .unwrap();
    f.set_hosted_mode(HostedMode::Migrated);

    assert_removed_from_legacy(&f, &id, &path).await;
    assert_eq!(library_page(&f, &id).await, StatusCode::OK);
    sync_hosted_test_folder(&f, &db).await;
    f.set_hosted_mode(HostedMode::Migrated);
    assert_removed_from_legacy(&f, &id, &path).await;
}

#[tokio::test]
async fn team_legacy_deletion_after_a_reviewed_edit() {
    let (f, db) = fixture_with_hosted_mode(HostedMode::Migrated).await;
    let original = create_folder_test_page(&db, "Editable page", "authored").await;
    sync_hosted_test_folder(&f, &db).await;
    let path = held_path(&f, &original.id).await;
    let revision = db.get_page(&original.id).await.unwrap().unwrap().version;
    let session = browser_session(&f, &f.writer.0);
    let (status, preview) = review(
        &f,
        &session,
        "preview",
        json!({
            "kind": "page", "title": "Editable page", "body": "A reviewed new edition.",
            "space_id": "design", "item_id": original.id, "expected_revision": revision,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let (status, receipt) = review(
        &f,
        &session,
        "publish",
        json!({"preview_id": preview["id"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");

    assert_removed_from_legacy(&f, &original.id, &path).await;
    assert_eq!(library_page(&f, &original.id).await, StatusCode::OK);
}

async fn create_dependent_page(db: &wenlan_core::db::MemoryDB, source: &str) -> String {
    let id = create_folder_test_page(db, "A note from a personal memory", "authored")
        .await
        .id;
    db.upsert_documents(vec![wenlan_core::sources::RawDocument {
        source: "memory".into(),
        source_id: source.into(),
        title: "Host memory".into(),
        content: "Fictional content of the host's memory.".into(),
        confirmed: Some(true),
        ..Default::default()
    }])
    .await
    .unwrap();
    db.update_page_content(
        &id,
        "A synthesis from the forgotten memory.",
        &[source],
        "manual_edit",
    )
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn team_legacy_deletion_when_the_source_was_permanently_deleted() {
    permanent_source_removal(HostedMode::Legacy).await;
}

#[tokio::test]
async fn team_legacy_deletion_when_a_refresh_clears_the_source_staleness() {
    permanent_source_removal(HostedMode::Migrated).await;
}

async fn permanent_source_removal(mode: HostedMode) {
    let (mut f, db) = fixture_with_hosted_mode(mode).await;
    let source = "fixture-deleted-memory";
    let id = create_dependent_page(&db, source).await;
    sync_hosted_test_folder(&f, &db).await;
    let path = held_path(&f, &id).await;
    db.delete_by_source_id("memory", source).await.unwrap();

    match mode {
        HostedMode::Legacy => f.set_hosted_mode(HostedMode::Migrated),
        HostedMode::Migrated => {
            // A private refresh can clear staleness without changing the page revision.
            let revision = db.get_page_source_revision(&id).await.unwrap();
            assert!(db
                .clear_page_staleness_at_revision(&id, revision)
                .await
                .unwrap());
        }
    }

    assert_removed_from_legacy(&f, &id, &path).await;
    assert_eq!(library_page(&f, &id).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn team_legacy_deletion_after_forget_and_withdraw() {
    let (mut f, db) = fixture_with_hosted_mode(HostedMode::Migrated).await;
    let withdrawn = create_folder_test_page(&db, "Withdrawn team note", "authored")
        .await
        .id;
    let source = "fixture-legacy-source";
    let id = create_dependent_page(&db, source).await;
    sync_hosted_test_folder(&f, &db).await;
    let path = held_path(&f, &id).await;
    let withdrawn_path = held_path(&f, &withdrawn).await;
    let (owner, _) = team::owner_key(&f.state.team_path, "Owner").unwrap();
    let session = browser_session(&f, &owner);
    let (status, body) = review(&f, &session, &format!("withdraw/{withdrawn}"), Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_removed_from_legacy(&f, &withdrawn, &withdrawn_path).await;
    assert_eq!(library_page(&f, &withdrawn).await, StatusCode::NOT_FOUND);
    assert_eq!(library_page(&f, &id).await, StatusCode::OK);
    assert_eq!(
        db.forget_memory(source, "person", "fixture").await.unwrap(),
        wenlan_core::db::ForgetOutcome::Forgotten
    );

    assert_removed_from_legacy(&f, &id, &path).await;
    assert_eq!(library_page(&f, &id).await, StatusCode::NOT_FOUND);
    db.restore_forgotten_memory(source, "person").await.unwrap();
    sync_hosted_test_folder(&f, &db).await;
    f.set_hosted_mode(HostedMode::Migrated);
    assert_removed_from_legacy(&f, &id, &path).await;
    assert_eq!(library_page(&f, &id).await, StatusCode::NOT_FOUND);
    assert_removed_from_legacy(&f, &withdrawn, &withdrawn_path).await;
    assert_eq!(library_page(&f, &withdrawn).await, StatusCode::NOT_FOUND);
}
