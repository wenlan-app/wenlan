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
