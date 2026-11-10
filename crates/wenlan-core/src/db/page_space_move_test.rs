// SPDX-License-Identifier: Apache-2.0

use super::{tests::test_db, MemoryDB, SetPageSpaceError, UNFILED_SPACE_ID};
use crate::read_scope::ReadScope;
use wenlan_types::responses::SetPageSpaceErrorCode;

const ID: &str = "page-move";
const BODY: &str = "Body that the move must not touch [1].";

/// The refusal code and version, or a panic naming what came back instead.
fn refusal<T: std::fmt::Debug>(
    result: Result<T, SetPageSpaceError>,
) -> (SetPageSpaceErrorCode, Option<i64>) {
    match result {
        Err(SetPageSpaceError::Refused {
            code,
            current_version,
        }) => (code, current_version),
        other => panic!("expected a typed Space-move refusal, got {other:?}"),
    }
}

/// An unfiled page citing one memory that moved into `work`, one still in no
/// Space, and one in `other`. Its cites edges, a community assignment, and a
/// basis on a `work` page ride along so the move's side effects are visible.
async fn fixture() -> (MemoryDB, tempfile::TempDir) {
    let (mut db, dir) = test_db().await;
    db.embedder = std::sync::OnceLock::new();
    {
        let conn = db.conn.lock().await;
        conn.execute_batch(&format!(
            "INSERT INTO spaces (id,name,created_at,updated_at)
             VALUES ('space-move-work','work',1,1),('space-move-other','other',1,1);
             INSERT INTO memories (id,source,source_id,title,content,chunk_index,last_modified,
                                   space,version,content_hash,source_agent,chunk_type)
             VALUES ('row-work','memory','mem-work','Work','Work memory text',0,10,'work',1,
                     'hash-work','test','text'),
                    ('row-loose','memory','mem-loose','Loose','Loose memory text',0,10,
                     '{UNFILED_SPACE_ID}',1,'hash-loose','test','text'),
                    ('row-other','memory','mem-other','Other','Other memory text',0,10,'other',1,
                     'hash-other','test','text');
             INSERT INTO pages
                 (id,title,summary,content,space,workspace,source_memory_ids,version,status,
                  kind,creation_kind,created_at,last_compiled,last_modified,incarnation)
             VALUES ('{ID}','Moving note','Summary','{BODY}','{UNFILED_SPACE_ID}',
                     '{UNFILED_SPACE_ID}','[\"mem-work\",\"mem-loose\",\"mem-other\"]',3,'active',
                     'concept','distilled','2026-01-01','2026-01-02','2026-01-03','inc-move'),
                    ('page-basis','Work basis','Summary','Basis body','work','work','[]',1,
                     'active','concept','distilled','2026-01-01','2026-01-02','2026-01-03',
                     'inc-basis');
             INSERT INTO edges (edge_id,src_id,src_kind,dst_id,dst_kind,edge_type,lineage,
                                grounded,space,created_at,semantic_type)
             VALUES ('edge-work','{ID}','page','mem-work','memory','cites','legacy',0,
                     '{UNFILED_SPACE_ID}',1,NULL),
                    ('edge-loose','{ID}','page','mem-loose','memory','cites','evidence',0,
                     '{UNFILED_SPACE_ID}',1,NULL);
             INSERT INTO page_community_assignments
                 (page_id,space,community_id,state,score,page_version,routing_input_generation,
                  routing_space_generation,community_published_generation,route_version,
                  updated_at)
             VALUES ('{ID}','{UNFILED_SPACE_ID}',NULL,'dropped',0.0,3,0,0,0,'v1',1);
             INSERT INTO page_basis
                 (page_id,basis_page_id,basis_title,confirmed_version,confirmed_digest,
                  confirmed_by,confirmed_at,state,changed_version,minor_only,added_by,added_at,
                  basis_incarnation)
             VALUES ('{ID}','page-basis','Old basis name',1,'','test',1,'pending',NULL,0,
                     'test',1,'inc-basis');"
        ))
        .await
        .unwrap();
        MemoryDB::append_page_history(&conn, ID, "distill", 1)
            .await
            .unwrap();
    }
    (db, dir)
}

async fn text(db: &MemoryDB, sql: &str) -> String {
    db.conn
        .lock()
        .await
        .query(sql, ())
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap()
}

async fn integer(db: &MemoryDB, sql: &str) -> i64 {
    db.conn
        .lock()
        .await
        .query(sql, ())
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap()
}

async fn snapshot(db: &MemoryDB) -> serde_json::Value {
    serde_json::to_value(db.get_page_browse(ID).await.unwrap().unwrap()).unwrap()
}

#[tokio::test]
async fn moving_an_unfiled_page_into_a_space_bumps_version_and_history_only() {
    let (db, _dir) = fixture().await;
    let before = db.get_page(ID).await.unwrap().unwrap();
    let history = db.list_page_history(ID, 20).await.unwrap();

    let moved = db
        .set_page_space(ID, Some(" work "), 3, &ReadScope::Global)
        .await
        .unwrap();

    assert_eq!(
        (
            moved.page_id.as_str(),
            moved.space.as_deref(),
            moved.version
        ),
        (ID, Some("work"), 4)
    );
    let after = db.get_page(ID).await.unwrap().unwrap();
    assert_eq!(after.space.as_deref(), Some("work"));
    assert_eq!(after.workspace.as_deref(), Some("work"));
    assert_eq!(after.version, 4);
    assert_eq!(after.content, before.content);
    assert_eq!(after.source_memory_ids, before.source_memory_ids);
    assert_eq!(after.title, before.title);
    assert_eq!(after.last_edited_by.as_deref(), Some("space_move"));
    assert_eq!(
        after.last_delta_summary.as_deref(),
        Some("Moved from no Space to Space 'work'")
    );
    let moved_history = db.list_page_history(ID, 20).await.unwrap();
    assert_eq!(moved_history.len(), history.len() + 1);
    let entry = moved_history.iter().find(|e| e.version == 4).unwrap();
    assert_eq!(entry.edited_by, "space_move");
    assert_eq!(entry.content, BODY);
    assert_eq!(
        crate::authorship::version_author("space_move", "distilled"),
        crate::authorship::VersionAuthor::System
    );

    // The Space-scoped grouping row is gone; the next pass places the page.
    assert_eq!(
        integer(
            &db,
            "SELECT COUNT(*) FROM page_community_assignments WHERE page_id='page-move'"
        )
        .await,
        0
    );
    // The cites edge to a note in the new Space follows the page; the one to
    // a note left in no Space keeps its stamp rather than trip the fence.
    assert_eq!(
        text(&db, "SELECT space FROM edges WHERE edge_id='edge-work'").await,
        "work"
    );
    assert_eq!(
        text(&db, "SELECT space FROM edges WHERE edge_id='edge-loose'").await,
        UNFILED_SPACE_ID
    );
    // The basis was re-evaluated in the move's transaction: citer and basis
    // now share a Space, so the stored title follows the basis' own name.
    assert_eq!(
        text(
            &db,
            "SELECT basis_title FROM page_basis WHERE page_id='page-move'"
        )
        .await,
        "Work basis"
    );
}

#[tokio::test]
async fn unreadable_sources_are_reported_by_position_only() {
    let (db, _dir) = fixture().await;
    let moved = db
        .set_page_space(ID, Some("work"), 3, &ReadScope::Global)
        .await
        .unwrap();
    // mem-loose stays in no Space and mem-other in `other`: positions 2 and 3.
    assert_eq!(moved.unreadable_source_positions, vec![2, 3]);
    let wire = serde_json::to_string(&moved).unwrap();
    assert!(
        !wire.contains("other") && !wire.contains("mem-"),
        "the reply names positions, never another Space or a source: {wire}"
    );

    // Moving back to no Space is the undo: version bumps again and now the
    // two Space-held sources are the unreadable ones.
    let back = db
        .set_page_space(ID, None, 4, &ReadScope::Global)
        .await
        .unwrap();
    assert_eq!((back.space, back.version), (None, 5));
    assert_eq!(back.unreadable_source_positions, vec![1, 3]);
    let page = db.get_page(ID).await.unwrap().unwrap();
    assert_eq!(page.space, None);
    assert_eq!(
        text(&db, "SELECT workspace FROM pages WHERE id='page-move'").await,
        UNFILED_SPACE_ID
    );
}

#[tokio::test]
async fn same_space_is_a_no_op() {
    let (db, _dir) = fixture().await;
    let before = snapshot(&db).await;
    let history = db.list_page_history(ID, 20).await.unwrap();
    for target in [None, Some(""), Some("  ")] {
        let same = db
            .set_page_space(ID, target, 3, &ReadScope::Global)
            .await
            .unwrap();
        assert_eq!((same.space, same.version), (None, 3));
        assert_eq!(same.unreadable_source_positions, vec![1, 3]);
    }
    assert_eq!(snapshot(&db).await, before);
    assert_eq!(db.list_page_history(ID, 20).await.unwrap(), history);
    assert_eq!(
        integer(
            &db,
            "SELECT COUNT(*) FROM page_community_assignments WHERE page_id='page-move'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn unknown_space_stale_version_and_locked_caller_are_refused_without_writing() {
    let (db, _dir) = fixture().await;
    let before = snapshot(&db).await;
    for name in ["nowhere", UNFILED_SPACE_ID] {
        assert_eq!(
            refusal(
                db.set_page_space(ID, Some(name), 3, &ReadScope::Global)
                    .await
            ),
            (SetPageSpaceErrorCode::PageSpaceUnknown, Some(3))
        );
    }
    for stale in [2, 4] {
        assert_eq!(
            refusal(
                db.set_page_space(ID, Some("work"), stale, &ReadScope::Global)
                    .await
            ),
            (SetPageSpaceErrorCode::PageSpaceVersionConflict, Some(3))
        );
    }
    for scope in [ReadScope::Space("work".into()), ReadScope::Uncategorized] {
        assert_eq!(
            refusal(db.set_page_space(ID, Some("work"), 3, &scope).await),
            (SetPageSpaceErrorCode::PageSpaceNeedsGlobalScope, None)
        );
    }
    assert!(matches!(
        db.set_page_space("no-such-page", Some("work"), 1, &ReadScope::Global)
            .await,
        Err(SetPageSpaceError::Failed(crate::WenlanError::NotFound(_)))
    ));
    assert_eq!(snapshot(&db).await, before);
}

#[tokio::test]
async fn entity_overview_source_draft_and_archived_pages_are_not_movable() {
    let (db, _dir) = fixture().await;
    let cases = [
        ("kind='entity',creation_kind='entity'", "entity"),
        ("title='Overview'", "overview"),
        ("creation_kind='source'", "source"),
        ("creation_kind='imported'", "imported"),
        ("status='draft'", "draft"),
        ("status='archived'", "archived"),
    ];
    for (change, label) in cases {
        let conn = db.conn.lock().await;
        conn.execute_batch(&format!(
            "UPDATE pages SET title='Moving note',kind='concept',creation_kind='distilled',
                              status='active' WHERE id='{ID}';
             UPDATE pages SET {change} WHERE id='{ID}';"
        ))
        .await
        .unwrap();
        drop(conn);
        let version = integer(&db, "SELECT version FROM pages WHERE id='page-move'").await;
        assert_eq!(
            refusal(
                db.set_page_space(ID, Some("work"), version, &ReadScope::Global)
                    .await
            ),
            (SetPageSpaceErrorCode::PageSpaceNotMovable, Some(version)),
            "{label} page must not move"
        );
        assert_eq!(
            text(&db, "SELECT space FROM pages WHERE id='page-move'").await,
            UNFILED_SPACE_ID,
            "{label} page stayed put"
        );
    }
    // An authored note moves.
    db.conn
        .lock()
        .await
        .execute_batch(&format!(
            "UPDATE pages SET title='Moving note',kind='authored',creation_kind='authored',
                              status='active' WHERE id='{ID}';"
        ))
        .await
        .unwrap();
    let version = integer(&db, "SELECT version FROM pages WHERE id='page-move'").await;
    db.set_page_space(ID, Some("work"), version, &ReadScope::Global)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_title_already_used_in_the_target_space_is_refused_privately() {
    let (db, _dir) = fixture().await;
    db.conn
        .lock()
        .await
        .execute_batch(
            "INSERT INTO pages (id,title,content,space,workspace,version,status,kind,
                                creation_kind,created_at,last_compiled,last_modified)
             VALUES ('private-twin','MOVING NOTE','secret body','work','work',1,'active',
                     'concept','distilled','2026-01-01','2026-01-01','2026-01-01');",
        )
        .await
        .unwrap();
    let before = snapshot(&db).await;
    let error = db
        .set_page_space(ID, Some("work"), 3, &ReadScope::Global)
        .await
        .unwrap_err();
    let message = error.to_string();
    assert!(!message.contains("private-twin") && !message.contains("secret"));
    assert_eq!(
        refusal::<()>(Err(error)),
        (SetPageSpaceErrorCode::PageSpaceTitleTaken, Some(3))
    );
    assert_eq!(snapshot(&db).await, before);
}
