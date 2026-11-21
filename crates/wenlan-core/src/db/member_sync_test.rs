// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::db::tests::test_db;
use crate::store::format::page::PageFile;
use version::{Author, AuthorKind, Decides, VersionMeta};
use wenlan_types::team::{TeamAccess, TeamSpaceAccess};

const SPACE: &str = "Planning";
const ID: &str = "page-meeting";

fn snapshot(
    id: &str,
    title: &str,
    body: &str,
    author: &str,
    stay: Option<&str>,
    parent: Option<&str>,
) -> PageVersion {
    let mut page = PageFile::new(title, body);
    page.id = Some(id.into());
    hydrate(
        page,
        VersionMeta {
            parent: parent.map(str::to_string),
            merged: Vec::new(),
            body_digest: version::body_digest(body),
            author: Author {
                member: author.into(),
                kind: AuthorKind::Person,
                agent: None,
            },
            ts: "2026-09-01T00:00:00Z".into(),
            archived: false,
            locked: false,
            proposal: false,
            decides: None,
            device: None,
            stay: stay.map(str::to_string),
        },
    )
}

fn hydrate(page: PageFile, meta: VersionMeta) -> PageVersion {
    let body = version::render(&page, &meta);
    let name = version::version_file_name(&version::sha256_hex(body.as_bytes()));
    version::parse(&name, body.as_bytes()).unwrap()
}

fn writer<'a>(db: &'a MemoryDB, settings: &'a Path) -> MemberSyncWriter<'a> {
    MemberSyncWriter {
        db,
        space: SPACE,
        member_id: "host",
        settings_path: settings,
        knowledge_path: None,
    }
}

fn add_editor(settings: &Path) -> String {
    crate::team::add_member(
        &crate::rulesync_team::team_path(settings),
        "Planning collaborator",
        &[TeamSpaceAccess {
            space: SPACE.into(),
            access: TeamAccess::Write,
            id: None,
        }],
    )
    .unwrap()
    .0
    .id
}

async fn seed(db: &MemoryDB, id: &str, space: &str, title: &str, body: &str) -> Page {
    let conn = db.conn.lock().await;
    conn.execute(
        "INSERT INTO pages(id,title,content,space,workspace,created_at,last_compiled,
                           last_modified,creation_kind,incarnation)
         VALUES (?1,?2,?3,?4,?4,'2026-09-01','2026-09-01','2026-09-01','authored',?1)",
        params![id, title, body, space],
    )
    .await
    .unwrap();
    read_page(&conn, id).await.unwrap()
}

async fn anchor(db: &MemoryDB, page: &Page, body: &str) -> PageVersion {
    let stay = db
        .shared_page_stay_token(page, SPACE)
        .await
        .unwrap()
        .unwrap();
    let root = snapshot(&page.id, &page.title, body, "host", Some(&stay), None);
    db.set_app_metadata(
        &format!("shared_space.held:{SPACE}"),
        &serde_json::json!({(page.id.clone()): {"version":page.version,"sha": root.sha}})
            .to_string(),
    )
    .await
    .unwrap();
    root
}

#[tokio::test]
async fn checked_member_edit_preserves_host_citations_summary_and_private_links() {
    let (db, dir) = test_db().await;
    let settings = dir.path().join("settings.json");
    let editor = add_editor(&settings);
    db.create_space(SPACE, None, false).await.unwrap();
    db.create_space("Personal", None, false).await.unwrap();
    let private = seed(
        &db,
        "page-personal",
        "Personal",
        "Agenda",
        "My [[Planning note]]",
    )
    .await;
    let original = "The meeting is in May [1].\n\nReview the slides.";
    seed(&db, ID, SPACE, "Planning note", original).await;
    let citations = r#"[{"occurrence":1,"marker":1,"source_kind":"external_url","locator":"https://example.com/calendar","score":0.9,"status":"verified","scope":"sentence"}]"#;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            "UPDATE pages SET citations=?2,summary='Team planning summary' WHERE id=?1",
            params![ID, citations],
        )
        .await
        .unwrap();
    }
    let page = db.get_page(ID).await.unwrap().unwrap();
    let root = anchor(&db, &page, "The meeting is in May.\n\nReview the slides.").await;
    let edit = snapshot(
        ID,
        "Agenda",
        "The meeting is in May.\n\nReview the charts.",
        &editor,
        root.meta.stay.as_deref(),
        Some(&root.sha),
    );
    let history = [root.clone(), edit.clone()];
    let result = writer(&db, &settings)
        .write(MemberSyncInput {
            change: &edit,
            page: Some(&page),
            history: &history,
            operation: MemberSyncOperation::Edit {
                preceding: &[],
                conflict: None,
            },
        })
        .await
        .unwrap();
    assert!(matches!(
        result,
        MemberSyncOutcome::Applied { wrote: true, .. }
    ));
    let saved = db.get_page(ID).await.unwrap().unwrap();
    assert_eq!(saved.title, "Agenda");
    assert_eq!(
        saved.content,
        "The meeting is in May [1].\n\nReview the charts."
    );
    assert_eq!(saved.summary.as_deref(), Some("Team planning summary"));
    let private_after = db.get_page(&private.id).await.unwrap().unwrap();
    assert_eq!(
        (private_after.content, private_after.version),
        (private.content, private.version)
    );
    let conn = db.conn.lock().await;
    let stored: String = conn
        .query("SELECT citations FROM pages WHERE id=?1", [ID])
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(stored, citations);
}

#[tokio::test]
async fn checked_member_edit_leaves_notes_that_rely_on_the_page_waiting() {
    let (db, dir) = test_db().await;
    let settings = dir.path().join("settings.json");
    let editor = add_editor(&settings);
    db.create_space(SPACE, None, false).await.unwrap();
    let body = "The meeting is in May.";
    let page = seed(&db, ID, SPACE, "Planning note", body).await;
    seed(
        &db,
        "page-plan",
        SPACE,
        "Plan",
        "Built on the planning note.",
    )
    .await;
    db.mark_page_basis("page-plan", ID, page.version, "human")
        .await
        .unwrap();
    let root = anchor(&db, &page, body).await;
    let edit = snapshot(
        ID,
        "Planning note",
        "The meeting moved to June.",
        &editor,
        root.meta.stay.as_deref(),
        Some(&root.sha),
    );
    let history = [root.clone(), edit.clone()];
    let result = writer(&db, &settings)
        .write(MemberSyncInput {
            change: &edit,
            page: Some(&page),
            history: &history,
            operation: MemberSyncOperation::Edit {
                preceding: &[],
                conflict: None,
            },
        })
        .await
        .unwrap();
    assert!(matches!(
        result,
        MemberSyncOutcome::Applied { wrote: true, .. }
    ));
    let basis = db
        .list_page_basis("page-plan")
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.basis_page_id == ID)
        .unwrap();
    assert_eq!(
        basis.state,
        crate::db::page_basis::BASIS_STATE_PENDING,
        "a member's edit must leave the note that relies on it waiting"
    );
}

#[tokio::test]
async fn checked_member_history_rolls_back_when_final_body_cannot_preserve_citations() {
    let (db, dir) = test_db().await;
    let settings = dir.path().join("settings.json");
    let editor = add_editor(&settings);
    db.create_space(SPACE, None, false).await.unwrap();
    let original = "The meeting is in May [1].\n\nReview the slides.";
    seed(&db, ID, SPACE, "Planning note", original).await;
    {
        let conn = db.conn.lock().await;
        conn.execute(
            r#"UPDATE pages SET citations='[{"occurrence":1,"marker":1,"source_kind":"external_url","locator":"https://example.com/calendar","score":1.0,"status":"verified"}]' WHERE id=?1"#,
            [ID],
        ).await.unwrap();
    }
    let page = db.get_page(ID).await.unwrap().unwrap();
    let root = anchor(&db, &page, "The meeting is in May.\n\nReview the slides.").await;
    let intermediate = snapshot(
        ID,
        &page.title,
        "The meeting is in May.\n\nReview the charts.",
        &editor,
        root.meta.stay.as_deref(),
        Some(&root.sha),
    );
    let edit = snapshot(
        ID,
        &page.title,
        "Review the charts.",
        &editor,
        root.meta.stay.as_deref(),
        Some(&intermediate.sha),
    );
    let history = [root, intermediate.clone(), edit.clone()];
    let checked = writer(&db, &settings);
    let result = checked
        .write(MemberSyncInput {
            change: &edit,
            page: Some(&page),
            history: &history,
            operation: MemberSyncOperation::Edit {
                preceding: &[&intermediate],
                conflict: None,
            },
        })
        .await
        .unwrap();
    assert!(matches!(result, MemberSyncOutcome::Refused));
    let saved = db.get_page(ID).await.unwrap().unwrap();
    assert_eq!(saved.content, original);
    assert_eq!(saved.version, page.version);
    assert!(db.list_page_history(ID, 10).await.unwrap().is_empty());
    let receipts = db.member_sync_refusals(SPACE, &editor).await.unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].reason, Reason::CitationsWouldChange);
    assert_eq!(receipts[0].change, edit.sha);
    assert!(db
        .member_sync_refusals(SPACE, "other-member")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn checked_member_conflict_copy_keeps_private_orphan_links_untouched() {
    let (db, dir) = test_db().await;
    let settings = dir.path().join("settings.json");
    let editor = add_editor(&settings);
    db.create_space(SPACE, None, false).await.unwrap();
    db.create_space("Personal", None, false).await.unwrap();
    let private = seed(
        &db,
        "page-personal",
        "Personal",
        "My notes",
        "A link to [[Planning note (Host)]].",
    )
    .await;
    let label = "Planning note (Host)";
    db.replace_page_links(
        &private.id,
        &[crate::synthesis::wikilinks::Wikilink {
            label: label.into(),
            target_page_id: None,
        }],
    )
    .await
    .unwrap();
    let before = db.list_orphan_link_labels(1).await.unwrap();
    assert!(before.iter().any(|(name, _)| name == label));
    let page = seed(&db, ID, SPACE, "Planning note", "Review the slides.").await;
    let root = anchor(&db, &page, &page.content).await;
    let stay = root.meta.stay.as_deref();
    let losing = snapshot(
        ID,
        &page.title,
        "Review the charts.",
        "host",
        stay,
        Some(&root.sha),
    );
    let edit = snapshot(
        ID,
        &page.title,
        "Review the agenda.",
        &editor,
        stay,
        Some(&root.sha),
    );
    let history = [root, losing.clone(), edit.clone()];
    let result = writer(&db, &settings)
        .write(MemberSyncInput {
            change: &edit,
            page: Some(&page),
            history: &history,
            operation: MemberSyncOperation::Edit {
                preceding: &[],
                conflict: Some(MemberSyncConflict::Copy {
                    id: "page-copy",
                    version: &losing,
                    name: "Host",
                }),
            },
        })
        .await
        .unwrap();
    assert!(matches!(
        result,
        MemberSyncOutcome::Applied { wrote: true, .. }
    ));
    assert_eq!(
        db.get_page("page-copy").await.unwrap().unwrap().title,
        label
    );
    assert_eq!(db.list_orphan_link_labels(1).await.unwrap(), before);
}

#[tokio::test]
async fn checked_member_review_rejects_an_old_card_and_closes_its_current_card() {
    let (db, dir) = test_db().await;
    let settings = dir.path().join("settings.json");
    let editor = add_editor(&settings);
    db.create_space(SPACE, None, false).await.unwrap();
    seed(&db, ID, SPACE, "Planning note", "Review the slides.").await;
    {
        let conn = db.conn.lock().await;
        conn.execute("UPDATE pages SET review_required=1 WHERE id=?1", [ID])
            .await
            .unwrap();
    }
    let page = db.get_page(ID).await.unwrap().unwrap();
    let root = anchor(&db, &page, &page.content).await;
    let stay = root.meta.stay.as_deref();
    let proposal = snapshot(
        ID,
        &page.title,
        "Review the charts.",
        &editor,
        stay,
        Some(&root.sha),
    );
    let proposal = hydrate(
        proposal.page,
        VersionMeta {
            proposal: true,
            ..proposal.meta
        },
    );
    let history = [root.clone()];
    let checked = writer(&db, &settings);
    let staged = checked
        .write(MemberSyncInput {
            change: &proposal,
            page: Some(&page),
            history: &history,
            operation: MemberSyncOperation::Stage,
        })
        .await
        .unwrap();
    let MemberSyncOutcome::Applied {
        card: Some(card), ..
    } = staged
    else {
        panic!("current proposal should create a card");
    };
    db.update_page_content(
        ID,
        "Review the slides and meeting notes.",
        &[],
        "manual_edit",
    )
    .await
    .unwrap();
    let page = db.get_page(ID).await.unwrap().unwrap();
    let restaged = checked
        .write(MemberSyncInput {
            change: &proposal,
            page: Some(&page),
            history: &history,
            operation: MemberSyncOperation::Stage,
        })
        .await
        .unwrap();
    assert!(matches!(restaged, MemberSyncOutcome::Applied { card: Some(id), .. } if id == card));
    let cards = db.open_shared_page_cards(SPACE).await.unwrap();
    assert_eq!(cards[0].page_version, Some(page.version));
    let decision = snapshot(ID, "", "", &editor, stay, None);
    let decision = hydrate(
        decision.page,
        VersionMeta {
            decides: Some(Decides {
                proposal: proposal.sha.clone(),
                accepted: false,
            }),
            ..decision.meta
        },
    );
    let refused = checked
        .write(MemberSyncInput {
            change: &decision,
            page: Some(&page),
            history: &history,
            operation: MemberSyncOperation::Decide {
                proposal: &proposal,
                card: Some((&card, Some("earlier-stay"))),
            },
        })
        .await
        .unwrap();
    assert!(matches!(refused, MemberSyncOutcome::Refused));
    assert!(matches!(
        db.page_card_state(&card).await.unwrap(),
        crate::db::revision_cards::PageCardState::Open
    ));
    let current_decision = hydrate(
        decision.page,
        VersionMeta {
            ts: "2026-09-01T00:01:00Z".into(),
            ..decision.meta
        },
    );
    let closed = checked
        .write(MemberSyncInput {
            change: &current_decision,
            page: Some(&page),
            history: &history,
            operation: MemberSyncOperation::Decide {
                proposal: &proposal,
                card: Some((&card, stay)),
            },
        })
        .await
        .unwrap();
    assert!(matches!(
        closed,
        MemberSyncOutcome::Applied { wrote: true, .. }
    ));
    assert!(matches!(
        db.page_card_state(&card).await.unwrap(),
        crate::db::revision_cards::PageCardState::Closed
    ));
}

#[tokio::test]
async fn checked_member_lock_cannot_touch_a_page_that_moved_after_planning() {
    let (db, dir) = test_db().await;
    let settings = dir.path().join("settings.json");
    let editor = add_editor(&settings);
    db.create_space(SPACE, None, false).await.unwrap();
    db.create_space("Personal", None, false).await.unwrap();
    let page = seed(&db, ID, SPACE, "Planning note", "Review the slides.").await;
    let root = anchor(&db, &page, &page.content).await;
    let lock = snapshot(
        ID,
        &page.title,
        &page.content,
        &editor,
        root.meta.stay.as_deref(),
        Some(&root.sha),
    );
    let lock = hydrate(
        lock.page,
        VersionMeta {
            locked: true,
            ..lock.meta
        },
    );
    db.set_page_space(ID, Some("Personal"), page.version, &ReadScope::Global)
        .await
        .unwrap();
    let history = [root, lock.clone()];
    let result = writer(&db, &settings)
        .write(MemberSyncInput {
            change: &lock,
            page: Some(&page),
            history: &history,
            operation: MemberSyncOperation::Edit {
                preceding: &[],
                conflict: None,
            },
        })
        .await
        .unwrap();
    assert!(matches!(result, MemberSyncOutcome::Refused));
    let saved = db.get_page(ID).await.unwrap().unwrap();
    assert_eq!(saved.workspace.as_deref(), Some("Personal"));
    assert!(!saved.review_required);
    assert_eq!(
        db.member_sync_refusals(SPACE, &editor).await.unwrap()[0].reason,
        Reason::PageUnavailable
    );
}
