// SPDX-License-Identifier: AGPL-3.0-only
//! A hand edit or publish the team gate forwards for a member is recorded
//! as that member in person; the same call without the gate's identity (or
//! with a guessed one) is recorded exactly as before.

use axum::http::{HeaderMap, HeaderValue};

use super::*;
use crate::space_header::SpaceHeader;
use crate::team_actor::{self, TeamActor};
use crate::truth_guard::TruthView;

async fn server() -> (
    tempfile::TempDir,
    Arc<wenlan_core::db::MemoryDB>,
    Arc<RwLock<ServerState>>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(
        wenlan_core::db::MemoryDB::new(tmp.path(), Arc::new(wenlan_core::events::NoopEmitter))
            .await
            .unwrap(),
    );
    db.create_space("Alpha", None, false).await.unwrap();
    db.create_space("Beta", None, false).await.unwrap();
    let state = Arc::new(RwLock::new(ServerState {
        db: Some(db.clone()),
        ..Default::default()
    }));
    (tmp, db, state)
}

/// The actor a forwarded call carries, read the way the daemon reads it.
fn actor(secret: &str, member: &str) -> TeamActor {
    let mut headers = HeaderMap::new();
    headers.insert(
        team_actor::SECRET_HEADER,
        HeaderValue::from_str(secret).unwrap(),
    );
    headers.insert(team_actor::KIND_HEADER, HeaderValue::from_static("person"));
    headers.insert(
        team_actor::MEMBER_HEADER,
        HeaderValue::from_str(member).unwrap(),
    );
    headers.insert(
        team_actor::MEMBER_NAME_HEADER,
        HeaderValue::from_static("Amy"),
    );
    TeamActor(team_actor::from_headers(&headers))
}

async fn page_in(db: &wenlan_core::db::MemoryDB, id: &str, space: &str) -> i64 {
    let draft = db
        .create_page_draft_with_id_in_registered_space_and_folder(
            id,
            "Plan",
            "Body",
            Some(space),
            "",
        )
        .await
        .unwrap();
    let wenlan_core::pages::PageDraftPublishOutcome::Published(page) =
        db.publish_page_draft(id, draft.version).await.unwrap()
    else {
        panic!("publish")
    };
    page.version
}

async fn edit(
    state: &Arc<RwLock<ServerState>>,
    space: Option<&str>,
    who: TeamActor,
    id: &str,
    content: &str,
    version: i64,
) -> Result<Json<wenlan_types::responses::ManualPageWriteResponse>, ServerError> {
    let wire = serde_json::from_value(serde_json::json!({
        "content": content,
        "expected_version": version,
        "operation_id": format!("op-{content}"),
    }))
    .unwrap();
    handle_update_page(
        State(state.clone()),
        SpaceHeader(space.map(str::to_string)),
        who,
        Path(id.into()),
        Json(wire),
    )
    .await
}

async fn last_writer(db: &wenlan_core::db::MemoryDB, id: &str) -> String {
    db.list_page_history(id, 1).await.unwrap()[0]
        .edited_by
        .clone()
}

const PAGE: &str = "page_00000000-0000-4000-8000-00000000a001";

#[tokio::test]
async fn a_members_edit_is_recorded_as_that_member_in_person() {
    let (_tmp, db, state) = server().await;
    let version = page_in(&db, PAGE, "Alpha").await;
    let amy = actor(team_actor::secret(), "m-amy");
    let _ = edit(&state, Some("Alpha"), amy.clone(), PAGE, "One", version)
        .await
        .unwrap();
    assert_eq!(last_writer(&db, PAGE).await, "shared_person:m-amy");
    // Compare-and-swap is unchanged: a stale version is refused.
    let refused = edit(&state, Some("Alpha"), amy, PAGE, "Two", version).await;
    assert!(refused.is_err(), "a stale expected_version must be refused");
    assert_eq!(db.get_page(PAGE).await.unwrap().unwrap().content, "One");
}

#[tokio::test]
async fn a_member_cannot_edit_outside_the_pinned_space() {
    let (_tmp, db, state) = server().await;
    let version = page_in(&db, PAGE, "Beta").await;
    let amy = actor(team_actor::secret(), "m-amy");
    for pinned in [Some("Alpha"), None] {
        let refused = edit(&state, pinned, amy.clone(), PAGE, "Taken", version).await;
        assert!(refused.is_err(), "{pinned:?}");
    }
    assert_eq!(db.get_page(PAGE).await.unwrap().unwrap().content, "Body");
}

/// A guessed secret is the same as no identity: the call is the person at
/// this computer, as before this change.
#[tokio::test]
async fn without_the_gates_secret_an_edit_is_recorded_as_before() {
    let (_tmp, db, state) = server().await;
    let mut version = page_in(&db, PAGE, "Alpha").await;
    for (n, who) in [
        TeamActor::default(),
        actor("guessed", "m-amy"),
        actor("", "m-amy"),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(who.0.is_none());
        // No pinned Space needed, as before.
        let _ = edit(&state, None, who, PAGE, &format!("Edit {n}"), version)
            .await
            .unwrap();
        assert_eq!(last_writer(&db, PAGE).await, "manual_edit");
        version = db.get_page(PAGE).await.unwrap().unwrap().version;
    }
}

async fn publish(
    state: &Arc<RwLock<ServerState>>,
    space: Option<&str>,
    who: TeamActor,
    id: &str,
    version: i64,
) -> Result<(), ServerError> {
    handle_publish_page_draft(
        State(state.clone()),
        SpaceHeader(space.map(str::to_string)),
        who,
        TruthView::automatic(),
        Path(id.into()),
        Json(PageDraftVersionRequest {
            expected_version: version,
        }),
    )
    .await
    .map(|_| ())
}

#[tokio::test]
async fn publishing_names_the_member_and_stays_in_the_pinned_space() {
    let (_tmp, db, state) = server().await;
    let amy = actor(team_actor::secret(), "m-amy");
    let draft = db
        .create_page_draft_with_id_in_registered_space_and_folder(
            PAGE,
            "Plan",
            "Body",
            Some("Beta"),
            "",
        )
        .await
        .unwrap();
    assert!(
        publish(&state, Some("Alpha"), amy.clone(), PAGE, draft.version)
            .await
            .is_err()
    );
    assert_eq!(db.get_page(PAGE).await.unwrap().unwrap().status, "draft");
    publish(&state, Some("Beta"), amy, PAGE, draft.version)
        .await
        .unwrap();
    assert_eq!(last_writer(&db, PAGE).await, "shared_person:m-amy");

    let other = "page_00000000-0000-4000-8000-00000000a002";
    let draft = db
        .create_page_draft_with_id_in_registered_space_and_folder(
            other,
            "Plan 2",
            "Body",
            Some("Beta"),
            "",
        )
        .await
        .unwrap();
    publish(
        &state,
        None,
        actor("guessed", "m-amy"),
        other,
        draft.version,
    )
    .await
    .unwrap();
    assert_eq!(last_writer(&db, other).await, "publish");
}

async fn create_draft(
    state: &Arc<RwLock<ServerState>>,
    space: Option<&str>,
    who: TeamActor,
    id: &str,
) -> Result<Json<PageDraftResponse>, ServerError> {
    let req = serde_json::from_value(serde_json::json!({
        "draft_id": id,
        "title": "Trail plan",
        "content": "Shared first lines",
    }))
    .unwrap();
    handle_create_page_draft(
        State(state.clone()),
        SpaceHeader(space.map(str::to_string)),
        who,
        TruthView::automatic(),
        Json(req),
    )
    .await
}

/// A repeated create answers with the draft as it is now. Once the owner
/// moves it to a Space the member is not in, the replay is not found and
/// carries none of what the owner wrote there.
#[tokio::test]
async fn a_replayed_draft_create_does_not_show_a_draft_moved_away() {
    let (_tmp, db, state) = server().await;
    let amy = actor(team_actor::secret(), "m-amy");
    let id = "page_00000000-0000-4000-8000-00000000a003";
    let first = create_draft(&state, Some("Alpha"), amy.clone(), id)
        .await
        .unwrap();
    // The same request again is still answered while the draft stays put.
    let _ = create_draft(&state, Some("Alpha"), amy.clone(), id)
        .await
        .unwrap();
    let moved = db
        .update_page_draft_in_registered_space(
            id,
            first.0.page.version,
            "Trail plan",
            "Shared first lines",
            Some("Beta"),
        )
        .await
        .unwrap();
    assert!(matches!(
        moved,
        wenlan_core::pages::PageDraftUpdateOutcome::Updated(_)
    ));
    let replay = create_draft(&state, Some("Alpha"), amy, id).await;
    let Err(refused) = replay else {
        panic!("a draft moved to Beta must not be echoed")
    };
    let response = axum::response::IntoResponse::into_response(refused);
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    let page = db.get_page(id).await.unwrap().unwrap();
    assert_eq!(page.space.as_deref(), Some("Beta"));
}

#[test]
fn only_a_member_write_carries_the_pinned_space_guard() {
    use wenlan_core::read_scope::ReadScope;
    assert_eq!(
        member_space_guard(true, Some("Alpha")),
        Some(ReadScope::Space("Alpha".into()))
    );
    assert_eq!(member_space_guard(false, Some("Alpha")), None);
    assert_eq!(member_space_guard(false, None), None);
}

/// The member's save is checked against the pinned Space again inside the
/// write: a page the owner moved after the route's check (so it now sits in
/// Beta) gets nothing, the same as a page never in the Space.
#[tokio::test]
async fn a_member_save_guarded_to_a_space_the_page_left_writes_nothing() {
    let (_tmp, db, _state) = server().await;
    let version = page_in(&db, PAGE, "Beta").await;
    let result = wenlan_core::post_write::page_write(
        &db,
        wenlan_core::post_write::PageWrite::UpdatePreservingSources {
            page_id: PAGE,
            req: wenlan_types::requests::UpdatePageRequest {
                content: "Attacker replacement".into(),
                source_memory_ids: vec![],
                expected_version: Some(version),
                caller_id: None,
                operation_id: Some("op-race".into()),
            },
            edited_by: "shared_person:m-amy",
            knowledge_path: None,
            basis: wenlan_core::db::BasisWrite {
                research_write: None,
                citation_snapshots: Vec::new(),
                minor: false,
                actor: "human".to_string(),
                marks: Vec::new(),
                confirm: None,
                confirm_snapshot: Vec::new(),
                card_marks: Vec::new(),
                code_basis: Vec::new(),
                space_guard: member_space_guard(true, Some("Alpha")),
                person_asked: None,
                restored_from: None,
                follows: false,
            },
        },
    )
    .await;
    assert!(result.is_err(), "{:?}", result.map(|r| r.outcome));
    let page = db.get_page(PAGE).await.unwrap().unwrap();
    assert_eq!(page.content, "Body");
    assert_eq!(page.version, version);
}

/// An AI tool's change, written the way a connected tool's refresh is.
async fn ai_change(db: &wenlan_core::db::MemoryDB, id: &str, content: &str, version: i64) -> i64 {
    wenlan_core::post_write::page_write(
        db,
        wenlan_core::post_write::PageWrite::Update {
            page_id: id,
            req: wenlan_types::requests::UpdatePageRequest {
                content: content.into(),
                source_memory_ids: vec![],
                expected_version: Some(version),
                caller_id: None,
                operation_id: Some(format!("op-ai-{content}")),
            },
            edited_by: "agent_refresh",
            require_stale: false,
            expected_source_revision: None,
            knowledge_path: None,
            citations: None,
        },
    )
    .await
    .unwrap();
    db.get_page(id).await.unwrap().unwrap().version
}

/// Putting back an AI tool's change is the member's own hand edit with the
/// text from before it: a new version by that member, in the history and in
/// the Space's recent changes, refused when stale or once the page left.
#[tokio::test]
async fn a_members_put_back_is_a_new_version_by_that_member() {
    let (_tmp, db, state) = server().await;
    let first = page_in(&db, PAGE, "Alpha").await;
    let after_ai = ai_change(&db, PAGE, "Body\n\nAn AI tool's sentence.", first).await;
    let amy = actor(team_actor::secret(), "m-amy");

    // A put-back computed against the version before the AI's is stale.
    let stale = edit(&state, Some("Alpha"), amy.clone(), PAGE, "Body", first).await;
    assert!(stale.is_err(), "a stale put-back must be refused");

    let _ = edit(&state, Some("Alpha"), amy.clone(), PAGE, "Body", after_ai)
        .await
        .unwrap();
    let page = db.get_page(PAGE).await.unwrap().unwrap();
    assert_eq!(page.content, "Body");
    assert_eq!(page.version, after_ai + 1, "a new version, not a rewind");
    let history = db.list_page_history(PAGE, 3).await.unwrap();
    assert_eq!(history[0].edited_by, "shared_person:m-amy");
    assert_eq!(
        history[1].edited_by, "agent_refresh",
        "the AI's version stays"
    );
    let changes = db
        .list_recent_changes_scoped(
            5,
            &wenlan_core::read_scope::ReadScope::Space("Alpha".into()),
        )
        .await
        .unwrap();
    let change = changes
        .iter()
        .find(|c| c.page_id == PAGE)
        .expect("in recent changes");
    assert_eq!(change.edited_by.as_deref(), Some("shared_person:m-amy"));

    // The owner moves the page to Beta: a put-back pinned to Alpha gets nothing.
    let moved = db
        .set_page_space(
            PAGE,
            Some("Beta"),
            page.version,
            &wenlan_core::read_scope::ReadScope::Global,
        )
        .await;
    assert!(moved.is_ok(), "{:?}", moved.err());
    let version = db.get_page(PAGE).await.unwrap().unwrap().version;
    let refused = edit(&state, Some("Alpha"), amy, PAGE, "Taken back", version).await;
    assert!(
        refused.is_err(),
        "a page that left the Space is not put back"
    );
    assert_eq!(db.get_page(PAGE).await.unwrap().unwrap().content, "Body");
}
