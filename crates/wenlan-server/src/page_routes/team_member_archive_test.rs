// SPDX-License-Identifier: AGPL-3.0-only
//! An archive, a restore or a basis mark the team gate forwards for a member
//! is recorded as that member in person, inside the pinned Space only; the
//! owner's own archive and restore are recorded as the person, and an AI
//! tool's as that tool, with its refusal rules unchanged.

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

fn amy() -> TeamActor {
    let mut headers = HeaderMap::new();
    headers.insert(
        team_actor::SECRET_HEADER,
        HeaderValue::from_str(team_actor::secret()).unwrap(),
    );
    headers.insert(team_actor::KIND_HEADER, HeaderValue::from_static("person"));
    headers.insert(team_actor::MEMBER_HEADER, HeaderValue::from_static("m-amy"));
    headers.insert(
        team_actor::MEMBER_NAME_HEADER,
        HeaderValue::from_static("Amy"),
    );
    TeamActor(team_actor::from_headers(&headers))
}

/// The app's person marker, as the owner's app sends it.
fn person_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-wenlan-caller", HeaderValue::from_static("person"));
    headers
}

fn ai_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-agent-name", HeaderValue::from_static("claude-code"));
    headers
}

async fn page_in(db: &wenlan_core::db::MemoryDB, id: &str, space: &str) -> i64 {
    let draft = db
        .create_page_draft_with_id_in_registered_space_and_folder(id, id, "Body", Some(space), "")
        .await
        .unwrap();
    let wenlan_core::pages::PageDraftPublishOutcome::Published(page) =
        db.publish_page_draft(id, draft.version).await.unwrap()
    else {
        panic!("publish")
    };
    page.version
}

async fn archive(
    state: &Arc<RwLock<ServerState>>,
    headers: HeaderMap,
    space: Option<&str>,
    who: TeamActor,
    id: &str,
) -> Result<Json<PageRemovalResponse>, ServerError> {
    handle_archive_page(
        State(state.clone()),
        headers,
        SpaceHeader(space.map(str::to_string)),
        who,
        Path(id.into()),
    )
    .await
    .map(PageResponse::into_test_json)
}

async fn restore(
    state: &Arc<RwLock<ServerState>>,
    headers: HeaderMap,
    space: Option<&str>,
    who: TeamActor,
    id: &str,
    version: i64,
) -> Result<Json<wenlan_types::responses::RestorePageResponse>, ServerError> {
    handle_restore_page(
        State(state.clone()),
        headers,
        SpaceHeader(space.map(str::to_string)),
        who,
        TruthView::automatic(),
        Path(id.into()),
        Json(wenlan_types::requests::RestorePageRequest {
            expected_version: version,
            title: None,
        }),
    )
    .await
    .map(PageResponse::into_test_json)
}

async fn lifecycle(db: &wenlan_core::db::MemoryDB, id: &str) -> Vec<(String, Option<String>)> {
    db.page_lifecycle_rows(id)
        .await
        .unwrap()
        .into_iter()
        .map(|(_, marker, _, actor)| (marker, actor))
        .collect()
}

const PAGE: &str = "page_00000000-0000-4000-8000-00000000b001";
const OTHER: &str = "page_00000000-0000-4000-8000-00000000b002";

#[tokio::test]
async fn a_members_archive_and_restore_are_recorded_as_that_member() {
    let (_tmp, db, state) = server().await;
    page_in(&db, PAGE, "Alpha").await;
    let _ = archive(&state, HeaderMap::new(), Some("Alpha"), amy(), PAGE)
        .await
        .unwrap();
    let archived = db.get_page(PAGE).await.unwrap().unwrap();
    assert_eq!(archived.status, "archived");
    let _ = restore(
        &state,
        HeaderMap::new(),
        Some("Alpha"),
        amy(),
        PAGE,
        archived.version,
    )
    .await
    .unwrap();
    assert_eq!(
        lifecycle(&db, PAGE).await,
        [
            (
                "restore".to_string(),
                Some("shared_person:m-amy".to_string())
            ),
            (
                "archive".to_string(),
                Some("shared_person:m-amy".to_string())
            ),
        ]
    );
    // The page's history names Amy on both rows.
    let Json(revisions) = handle_get_page_revisions(
        State(state.clone()),
        axum::http::HeaderMap::new(),
        SpaceHeader(Some("Alpha".into())),
        TruthView::automatic(),
        Path(PAGE.into()),
        axum::extract::Query(HashMap::new()),
    )
    .await
    .unwrap()
    .into_test_json();
    let history = revisions.history;
    let kinds: Vec<_> = history
        .iter()
        .filter(|item| {
            matches!(
                item.kind,
                wenlan_types::responses::PageHistoryKind::Archived
                    | wenlan_types::responses::PageHistoryKind::Restored
            )
        })
        .map(|item| item.edited_by.clone())
        .collect();
    assert_eq!(kinds.len(), 2, "{history:?}");
    assert!(kinds
        .iter()
        .all(|who| who.as_deref() == Some("shared_person:m-amy")));
}

#[tokio::test]
async fn a_member_cannot_archive_or_restore_outside_the_pinned_space() {
    let (_tmp, db, state) = server().await;
    page_in(&db, PAGE, "Beta").await;
    for pinned in [Some("Alpha"), None] {
        assert!(
            archive(&state, HeaderMap::new(), pinned, amy(), PAGE)
                .await
                .is_err(),
            "{pinned:?}"
        );
    }
    assert_eq!(db.get_page(PAGE).await.unwrap().unwrap().status, "active");
    // Archived by the owner, it still cannot be put back from another Space.
    let _ = archive(&state, person_headers(), None, TeamActor::default(), PAGE)
        .await
        .unwrap();
    let version = db.get_page(PAGE).await.unwrap().unwrap().version;
    for pinned in [Some("Alpha"), None] {
        assert!(
            restore(&state, HeaderMap::new(), pinned, amy(), PAGE, version)
                .await
                .is_err(),
            "{pinned:?}"
        );
    }
    assert_eq!(db.get_page(PAGE).await.unwrap().unwrap().status, "archived");
}

#[tokio::test]
async fn the_owners_archive_and_restore_are_recorded_as_the_person() {
    let (_tmp, db, state) = server().await;
    page_in(&db, PAGE, "Alpha").await;
    let _ = archive(&state, person_headers(), None, TeamActor::default(), PAGE)
        .await
        .unwrap();
    let version = db.get_page(PAGE).await.unwrap().unwrap().version;
    let _ = restore(
        &state,
        person_headers(),
        None,
        TeamActor::default(),
        PAGE,
        version,
    )
    .await
    .unwrap();
    assert_eq!(
        lifecycle(&db, PAGE).await,
        [
            ("restore".to_string(), Some("manual_edit".to_string())),
            ("archive".to_string(), Some("manual_edit".to_string())),
        ]
    );
}

/// An AI tool still may not archive a page a person wrote; nothing is
/// recorded for a refused archive.
#[tokio::test]
async fn an_ai_tools_refusal_is_unchanged() {
    let (_tmp, db, state) = server().await;
    page_in(&db, PAGE, "Alpha").await;
    let refused = archive(&state, ai_headers(), None, TeamActor::default(), PAGE).await;
    assert!(refused.is_err(), "an AI tool archived a person's page");
    assert_eq!(db.get_page(PAGE).await.unwrap().unwrap().status, "active");
    assert!(lifecycle(&db, PAGE).await.is_empty());
}

#[tokio::test]
async fn a_members_basis_mark_is_recorded_as_that_member_and_kept_in_one_space() {
    let (_tmp, db, state) = server().await;
    page_in(&db, PAGE, "Alpha").await;
    let basis_version = page_in(&db, OTHER, "Alpha").await;
    let mark = |space: &'static str, basis: &'static str, who: TeamActor| {
        let state = state.clone();
        async move {
            crate::page_basis_routes::handle_mark_basis(
                State(state),
                SpaceHeader(Some(space.to_string())),
                who,
                HeaderMap::new(),
                Path((PAGE.to_string(), basis.to_string())),
                Json(wenlan_types::page_basis::BasisVersionRequest {
                    version: basis_version,
                }),
            )
            .await
        }
    };
    let _ = mark("Alpha", OTHER, amy()).await.unwrap();
    let basis = db.list_page_basis(PAGE).await.unwrap();
    assert_eq!(basis.len(), 1);
    assert_eq!(basis[0].added_by, "shared_person:m-amy");
    // Pinned to another Space, neither page is there to mark.
    let (_tmp2, db2, state2) = server().await;
    page_in(&db2, PAGE, "Alpha").await;
    let v = page_in(&db2, OTHER, "Beta").await;
    let refused = crate::page_basis_routes::handle_mark_basis(
        State(state2),
        SpaceHeader(Some("Alpha".into())),
        amy(),
        HeaderMap::new(),
        Path((PAGE.to_string(), OTHER.to_string())),
        Json(wenlan_types::page_basis::BasisVersionRequest { version: v }),
    )
    .await;
    assert!(refused.is_err());
    assert!(db2.list_page_basis(PAGE).await.unwrap().is_empty());
}

/// The activity log names the member who archived, not the owner.
#[tokio::test]
async fn a_members_archive_is_logged_under_their_name() {
    let (_tmp, db, state) = server().await;
    page_in(&db, PAGE, "Alpha").await;
    let _ = archive(&state, HeaderMap::new(), Some("Alpha"), amy(), PAGE)
        .await
        .unwrap();
    let names: Vec<String> = db
        .list_agent_activity(20, None, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.action.contains("archive"))
        .map(|row| row.agent_name)
        .collect();
    // The log keeps agent names in its normalized form.
    assert_eq!(names, ["shared-person:m-amy"]);
}

/// Repeating a restore that already landed writes nothing, so it names
/// nobody: an earlier restore recorded without a name stays unnamed.
#[tokio::test]
async fn a_replayed_restore_names_nobody() {
    let (_tmp, db, state) = server().await;
    page_in(&db, PAGE, "Alpha").await;
    let _ = archive(&state, person_headers(), None, TeamActor::default(), PAGE)
        .await
        .unwrap();
    let archived = db.get_page(PAGE).await.unwrap().unwrap().version;
    // Restored straight in the store, as before names were kept: no name.
    let restored = db
        .restore_page(
            PAGE,
            None,
            archived,
            &wenlan_core::read_scope::ReadScope::Global,
        )
        .await
        .unwrap();
    let _ = restore(
        &state,
        HeaderMap::new(),
        Some("Alpha"),
        amy(),
        PAGE,
        restored.version,
    )
    .await
    .unwrap();
    assert_eq!(
        lifecycle(&db, PAGE).await,
        [
            ("restore".to_string(), None),
            ("archive".to_string(), Some("manual_edit".to_string())),
        ]
    );
}
