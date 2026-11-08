// SPDX-License-Identifier: Apache-2.0
//! Two members, each with their own database, sharing one Space through one
//! folder (delivery criteria 1-5 of the shared Space plan).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use wenlan_types::requests::{CreateConceptRequest, UpdatePageRequest};
use wenlan_types::shared_space::{SharedSyncCounts, SharedSyncRun};
use wenlan_types::WriteSpaceTarget;

use super::*;
use crate::events::NoopEmitter;

const SPACE: &str = "Wedding";

struct Member {
    name: &'static str,
    db: MemoryDB,
    dir: tempfile::TempDir,
}

impl Member {
    async fn new(name: &'static str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = Self::open(dir.path(), "wenlan.db").await;
        Self { name, db, dir }
    }

    async fn open(dir: &Path, file: &str) -> MemoryDB {
        let db = MemoryDB::new(&dir.join(file), Arc::new(NoopEmitter))
            .await
            .unwrap();
        db.create_space(SPACE, None, false).await.unwrap();
        db.create_space("Private", None, false).await.unwrap();
        db
    }

    fn settings(&self) -> PathBuf {
        self.dir.path().join(SETTINGS_FILE)
    }

    fn member_id(&self) -> String {
        share_settings_at(&self.settings(), &space())
            .unwrap()
            .unwrap()
            .member_id
    }

    async fn sync(&self, folder: &Path) -> SharedSyncCounts {
        sync_space(
            &self.db,
            SyncTarget {
                scope: &space(),
                folder,
                member_name: self.name,
                settings_path: &self.settings(),
                knowledge_path: None,
            },
        )
        .await
        .unwrap()
    }

    async fn share(&self, scope: &Scope, folder: &Path) -> Result<SharedSyncCounts, WenlanError> {
        sync_space(
            &self.db,
            SyncTarget {
                scope,
                folder,
                member_name: self.name,
                settings_path: &self.settings(),
                knowledge_path: None,
            },
        )
        .await
    }

    /// Every automatic pass, by scope key.
    async fn auto_all(&self, person_name: Option<&str>) -> Vec<(String, SharedSyncRun)> {
        sync_shared_spaces(&self.db, &self.settings(), None, person_name).await
    }

    async fn write(&self, title: &str, content: &str, space: WriteSpaceTarget) -> String {
        crate::post_write::create_page(
            &self.db,
            CreateConceptRequest {
                title: title.to_string(),
                content: content.to_string(),
                summary: None,
                entity_id: None,
                space,
                source_memory_ids: Vec::new(),
                creation_kind: Some("authored".to_string()),
                workspace: None,
            },
            "unknown",
            None,
        )
        .await
        .unwrap()
        .id
    }

    async fn edit(&self, id: &str, content: &str) {
        let result = crate::post_write::update_page_preserving_sources(
            &self.db,
            id,
            UpdatePageRequest {
                content: content.to_string(),
                source_memory_ids: Vec::new(),
                expected_version: None,
                caller_id: None,
                operation_id: None,
            },
            "manual_edit",
            None,
        )
        .await
        .unwrap();
        assert!(result.wrote, "edit of {id} did not land");
    }

    /// The daemon's pass of every shared Space, as the scheduler runs it.
    async fn auto(&self) -> SharedSyncRun {
        sync_shared_spaces(&self.db, &self.settings(), None, None)
            .await
            .into_iter()
            .find(|(space, _)| space == SPACE)
            .expect("the shared Space had a pass")
            .1
    }

    async fn page(&self, id: &str) -> Option<Page> {
        self.db.get_page(id).await.unwrap()
    }

    async fn rename(&self, id: &str, title: &str) {
        let page = self.page(id).await.unwrap();
        self.db
            .rename_page(id, title, page.version, &ReadScope::Global, None)
            .await
            .unwrap();
    }

    async fn restore(&self, id: &str) {
        let page = self.page(id).await.unwrap();
        self.db
            .restore_page(id, None, page.version, &ReadScope::Global)
            .await
            .unwrap();
    }

    async fn content(&self, id: &str) -> String {
        self.db.get_page(id).await.unwrap().unwrap().content
    }

    async fn history(&self, id: &str) -> Vec<crate::db::PageHistoryEntry> {
        self.db.list_page_history(id, 100).await.unwrap()
    }
}

fn space() -> Scope {
    Scope::Space(SPACE.to_string())
}

fn zero() -> SharedSyncCounts {
    SharedSyncCounts::default()
}

/// Every file in the folder, path and bytes.
fn folder_files(folder: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                out.push((path, bytes));
            }
        }
    }
    out.sort();
    out
}

fn version_count(folder: &Path) -> usize {
    folder_files(folder)
        .iter()
        .filter(|(path, _)| path.extension().is_some_and(|ext| ext == "md"))
        .count()
}

/// A short pause, so versions written one after the other have different
/// times.
async fn tick() {
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
}

#[tokio::test]
async fn a_page_reaches_the_other_member_and_three_edits_catch_up_in_one_sync() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().join("Wedding (shared)");
    let a = Member::new("Qi-Xuan").await;
    let b = Member::new("Ana").await;

    let id = a
        .write(
            "Seating plan",
            "Table 1: Ana and Ben.",
            SPACE.to_string().into(),
        )
        .await;
    assert_eq!(a.sync(&folder).await.exported, 1);
    let got = b.sync(&folder).await;
    assert_eq!(got.applied, 1, "{got:?}");
    let page = b.db.get_page(&id).await.unwrap().expect("B has A's page");
    assert_eq!(page.content, "Table 1: Ana and Ben.");
    assert_eq!(page.title, "Seating plan");
    assert_eq!(page.space.as_deref(), Some(SPACE));
    // Who wrote it is on B's history: A, in person.
    let history = b.history(&id).await;
    assert_eq!(
        history[0].edited_by,
        format!("shared_person:{}", a.member_id())
    );

    // Nothing changed: nothing to write either way.
    assert_eq!(a.sync(&folder).await, zero());
    assert_eq!(b.sync(&folder).await, zero());

    // B is closed while A edits three times.
    for (n, text) in [
        "Table 1: Ana.",
        "Table 1: Ana, Cleo.",
        "Table 1: Ana, Cleo, Dev.",
    ]
    .iter()
    .enumerate()
    {
        a.edit(&id, text).await;
        let got = a.sync(&folder).await;
        assert_eq!(got.exported, 1, "edit {n}: {got:?}");
    }
    let got = b.sync(&folder).await;
    assert_eq!(got.applied, 1, "{got:?}");
    assert_eq!(b.content(&id).await, "Table 1: Ana, Cleo, Dev.");
    assert_eq!(a.sync(&folder).await, zero());
    assert_eq!(b.sync(&folder).await, zero());

    // And back: B's edit reaches A, named as B.
    b.edit(&id, "Table 1: Ana, Cleo, Dev, Eli.").await;
    assert_eq!(b.sync(&folder).await.exported, 1);
    assert_eq!(a.sync(&folder).await.applied, 1);
    assert_eq!(a.content(&id).await, "Table 1: Ana, Cleo, Dev, Eli.");
    assert_eq!(
        a.history(&id).await[0].edited_by,
        format!("shared_person:{}", b.member_id())
    );
    assert_eq!(a.sync(&folder).await, zero());
    assert_eq!(b.sync(&folder).await, zero());

    let space: SpaceFile =
        serde_json::from_slice(&std::fs::read(folder.join(SPACE_FILE)).unwrap()).unwrap();
    assert_eq!(space.format, 1);
    assert_eq!(space.space, SPACE);
    assert_eq!(
        space.members,
        vec![
            SpaceMember {
                id: a.member_id(),
                name: "Qi-Xuan".to_string()
            },
            SpaceMember {
                id: b.member_id(),
                name: "Ana".to_string()
            },
        ]
    );
    // Each member wrote only into its own folder.
    for (path, bytes) in folder_files(&folder) {
        let relative = path
            .strip_prefix(&folder)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if relative == SPACE_FILE {
            continue;
        }
        let text = String::from_utf8(bytes).unwrap();
        let author = if text.contains(&format!("member: {}", a.member_id())) {
            a.member_id()
        } else {
            b.member_id()
        };
        assert!(
            relative.starts_with(&format!("members/{author}/pages/{id}/")),
            "{relative}"
        );
    }
}

#[tokio::test]
async fn edits_made_at_the_same_time_keep_both_versions_in_history() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_path_buf();
    let a = Member::new("Qi-Xuan").await;
    let b = Member::new("Ana").await;
    let id = a
        .write("Menu", "Fish or pasta.", SPACE.to_string().into())
        .await;
    a.sync(&folder).await;
    b.sync(&folder).await;

    // Both edit before either has the other's edit.
    a.edit(&id, "Fish, pasta or risotto.").await;
    b.edit(&id, "Fish or pasta, and a cake.").await;
    assert_eq!(b.sync(&folder).await.exported, 1);
    tick().await;
    // A's edit is later, so it wins everywhere; B's stays a version.
    assert_eq!(a.sync(&folder).await.exported, 1);
    let got = b.sync(&folder).await;
    assert_eq!(got.applied, 1, "{got:?}");
    assert_eq!(got.conflicts, 1, "{got:?}");
    assert_eq!(b.content(&id).await, "Fish, pasta or risotto.");
    let texts: Vec<String> = b
        .history(&id)
        .await
        .into_iter()
        .map(|row| row.content)
        .collect();
    assert!(
        texts.contains(&"Fish or pasta, and a cake.".to_string()),
        "{texts:?}"
    );
    assert!(
        texts.contains(&"Fish, pasta or risotto.".to_string()),
        "{texts:?}"
    );
    assert_eq!(a.content(&id).await, "Fish, pasta or risotto.");
    // No text is lost from the folder either.
    let all: String = folder_files(&folder)
        .into_iter()
        .map(|(_, bytes)| String::from_utf8(bytes).unwrap())
        .collect();
    assert!(all.contains("Fish or pasta, and a cake."));
    assert_eq!(a.sync(&folder).await, zero());
    assert_eq!(b.sync(&folder).await, zero());

    // B's next edit is written over the version B took, and ends the split.
    b.edit(&id, "Fish, pasta or risotto, and a cake.").await;
    assert_eq!(b.sync(&folder).await.exported, 1);
    assert_eq!(a.sync(&folder).await.applied, 1);
    let read = version::read_folder(&folder);
    let head = version::head(&read.pages[&id]).unwrap();
    assert_eq!(head.leaves.len(), 1);
    assert_eq!(head.winner.page.body, "Fish, pasta or risotto, and a cake.");
}

#[tokio::test]
async fn nothing_outside_the_shared_space_reaches_the_folder() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_path_buf();
    let a = Member::new("Qi-Xuan").await;
    let memory = |id: &str, content: &str| crate::sources::RawDocument {
        source: "memory".to_string(),
        source_id: id.to_string(),
        title: id.to_string(),
        content: content.to_string(),
        memory_type: Some("fact".to_string()),
        space: Some("Private".to_string()),
        last_modified: 1_700_000_000,
        ..Default::default()
    };
    a.db.upsert_documents(vec![
        memory("mem_cited_quote", "We said fish is fine for Grandma."),
        memory("mem_uncited_secret", "memory-secret-uncited about money"),
    ])
    .await
    .unwrap();
    let salary = a
        .write(
            "Salary review",
            "salary-secret-beta in the other Space.",
            "Private".to_string().into(),
        )
        .await;
    let diary = a
        .write(
            "Diary entry",
            "diary-secret-gamma with no Space.",
            WriteSpaceTarget::Uncategorized,
        )
        .await;
    let menu = a
        .write(
            "Menu",
            "Fish or pasta [1]. Budget agreed [2].",
            SPACE.to_string().into(),
        )
        .await;
    let citations = serde_json::json!([
        {"occurrence": 1, "marker": 1, "source_kind": "memory", "locator": "mem_cited_quote",
         "score": 1.0, "status": "verified", "cited_version": 1},
        {"occurrence": 2, "marker": 2, "source_kind": "memory", "locator": salary,
         "score": 1.0, "status": "verified", "cited_version": 1},
    ]);
    a.db.set_page_citations_for_test(&menu, &citations.to_string())
        .await
        .unwrap();

    let got = a.sync(&folder).await;
    assert_eq!(got.exported, 1, "only the shared Space's page: {got:?}");

    let all: String = folder_files(&folder)
        .into_iter()
        .map(|(path, bytes)| {
            format!(
                "{}\n{}\n",
                path.display(),
                String::from_utf8(bytes).unwrap()
            )
        })
        .collect();
    for private in [
        "mem_cited_quote",
        "mem_uncited_secret",
        "memory-secret-uncited",
        salary.as_str(),
        "Salary review",
        "salary-secret-beta",
        diary.as_str(),
        "Diary entry",
        "diary-secret-gamma",
        "Private",
    ] {
        assert!(
            !all.contains(private),
            "{private:?} is in the folder:\n{all}"
        );
    }
    // The shared page and its cited words are there, without where they came
    // from.
    assert!(all.contains("Fish or pasta [1]. Budget agreed [2]."));
    assert!(all.contains("“We said fish is fine for Grandma.” (from Qi-Xuan's conversation)"));
    assert!(all.contains("[2]: (from Qi-Xuan's conversation) · shared:quote"));

    // The page as B gets it keeps its words, and a later sync of A writes
    // nothing new: the rewritten version is the page's version.
    assert_eq!(a.sync(&folder).await, zero());
    let b = Member::new("Ana").await;
    assert_eq!(b.sync(&folder).await.applied, 1);
    assert_eq!(
        b.content(&menu).await,
        "Fish or pasta [1]. Budget agreed [2]."
    );
    assert_eq!(b.sync(&folder).await, zero());
    assert_eq!(a.sync(&folder).await, zero());
}

#[tokio::test]
async fn a_rebuilt_database_takes_the_same_pages_without_new_versions() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_path_buf();
    let a = Member::new("Qi-Xuan").await;
    let mut b = Member::new("Ana").await;
    let first = a
        .write("Guests", "Ana, Ben, Cleo.", SPACE.to_string().into())
        .await;
    let second = a
        .write("Venue", "The old mill.", SPACE.to_string().into())
        .await;
    let archived = a
        .write("Old plan", "Beach wedding.", SPACE.to_string().into())
        .await;
    a.sync(&folder).await;
    b.sync(&folder).await;
    b.edit(&second, "The old mill, by the river.").await;
    b.sync(&folder).await;
    a.sync(&folder).await;
    a.db.archive_page(&archived).await.unwrap();
    a.sync(&folder).await;
    b.sync(&folder).await;
    let versions = version_count(&folder);
    let member = b.member_id();

    // B's database is gone; its settings file is not.
    b.db = Member::open(b.dir.path(), "rebuilt.db").await;
    assert!(b.db.get_page(&first).await.unwrap().is_none());
    let got = b.sync(&folder).await;
    assert_eq!(
        got,
        SharedSyncCounts {
            applied: 2,
            ..zero()
        }
    );
    assert_eq!(b.content(&first).await, a.content(&first).await);
    assert_eq!(b.content(&second).await, "The old mill, by the river.");
    assert_eq!(a.content(&second).await, "The old mill, by the river.");
    assert!(
        b.page(&archived).await.is_none(),
        "a page archived in the folder is not made again"
    );
    assert_eq!(version_count(&folder), versions, "no version written twice");
    assert_eq!(
        b.member_id(),
        member,
        "B is the same member after the rebuild"
    );
    assert_eq!(b.sync(&folder).await, zero());
    assert_eq!(a.sync(&folder).await, zero());
    let space: SpaceFile =
        serde_json::from_slice(&std::fs::read(folder.join(SPACE_FILE)).unwrap()).unwrap();
    assert_eq!(space.members.len(), 2);
}

#[tokio::test]
async fn a_page_here_in_another_space_is_never_written_over_or_shared() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_path_buf();
    let a = Member::new("Qi-Xuan").await;
    let b = Member::new("Ana").await;
    let id = a
        .write("Budget", "Shared budget.", SPACE.to_string().into())
        .await;
    a.sync(&folder).await;
    b.sync(&folder).await;
    // B moves the page out of the shared Space by hand.
    let moved = b.db.get_page(&id).await.unwrap().unwrap();
    assert_eq!(moved.space.as_deref(), Some(SPACE));
    b.db.reassign_memories_space(SPACE, "Private")
        .await
        .unwrap();
    assert_eq!(
        b.db.get_page(&id).await.unwrap().unwrap().space.as_deref(),
        Some("Private")
    );
    b.edit(&id, "Private budget notes.").await;
    a.edit(&id, "Shared budget, updated.").await;
    a.sync(&folder).await;
    let got = b.sync(&folder).await;
    assert_eq!(got.applied, 0, "{got:?}");
    assert_eq!(got.exported, 0, "{got:?}");
    assert_eq!(got.skipped, 1, "{got:?}");
    assert_eq!(b.content(&id).await, "Private budget notes.");
    let all: String = folder_files(&folder)
        .into_iter()
        .map(|(_, bytes)| String::from_utf8(bytes).unwrap())
        .collect();
    assert!(!all.contains("Private budget notes."));
}

/// Every file in the folder, as one text.
fn walk(folder: &Path) -> String {
    folder_files(folder)
        .into_iter()
        .map(|(_, bytes)| String::from_utf8(bytes).unwrap())
        .collect()
}

/// Sync both members until neither writes anything (at most five rounds).
async fn settle(a: &Member, b: &Member, folder: &Path) {
    for _ in 0..5 {
        let (got_a, got_b) = (a.sync(folder).await, b.sync(folder).await);
        if got_a == zero() && got_b == zero() {
            return;
        }
    }
    panic!("the two members did not settle");
}

fn counts(exported: u32, applied: u32) -> SharedSyncCounts {
    SharedSyncCounts {
        exported,
        applied,
        ..zero()
    }
}

#[tokio::test]
async fn the_daemon_pass_takes_a_change_without_the_route_and_idle_passes_write_nothing() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_path_buf();
    let a = Member::new("Qi-Xuan").await;
    let b = Member::new("Ana").await;
    let id = a
        .write("Cake", "Lemon or chocolate.", SPACE.to_string().into())
        .await;
    // Sharing starts with the route; from then on the daemon's pass runs.
    a.sync(&folder).await;
    b.sync(&folder).await;

    a.edit(&id, "Lemon, three tiers.").await;
    let run = a.auto().await;
    assert_eq!(run.counts, counts(1, 0), "{run:?}");
    assert_eq!(run.paused, None);
    let run = b.auto().await;
    assert_eq!(run.counts, counts(0, 1), "{run:?}");
    assert_eq!(b.content(&id).await, "Lemon, three tiers.");
    for _ in 0..2 {
        assert_eq!(a.auto().await.counts, zero());
        assert_eq!(b.auto().await.counts, zero());
    }
    // The last pass is kept for the app to show.
    let status = share_status_at(&b.settings(), &space()).unwrap().unwrap();
    assert_eq!(status.member_name, "Ana");
    assert_eq!(status.folder, folder.to_string_lossy());
    let last = status.last.expect("a pass ran");
    assert_eq!((last.counts, last.paused), (zero(), None));
    assert!(
        share_status_at(&b.settings(), &Scope::Space("Private".into()))
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_missing_folder_pauses_the_space_and_nothing_is_archived_or_deleted() {
    let place = tempfile::tempdir().unwrap();
    let folder = place.path().join("Wedding (shared)");
    let away = place.path().join("unplugged");
    let a = Member::new("Qi-Xuan").await;
    let b = Member::new("Ana").await;
    let first = a
        .write("Guests", "Ana, Ben.", SPACE.to_string().into())
        .await;
    let second = a
        .write("Venue", "The old mill.", SPACE.to_string().into())
        .await;
    a.sync(&folder).await;
    b.sync(&folder).await;

    // The drive is unplugged: the folder is not there.
    std::fs::rename(&folder, &away).unwrap();
    a.edit(&first, "Ana, Ben, Cleo.").await;
    for member in [&a, &b, &a, &b] {
        let run = member.auto().await;
        assert_eq!(run.counts, zero(), "{run:?}");
        assert!(run.paused.is_some(), "{run:?}");
    }
    assert!(!folder.exists(), "a paused pass never makes the folder");
    let paused = share_status_at(&b.settings(), &space()).unwrap().unwrap();
    assert!(paused.last.unwrap().paused.is_some());
    for member in [&a, &b] {
        for id in [&first, &second] {
            let page = member.page(id).await.expect("still here");
            assert_eq!(page.status, "active");
        }
    }

    // It is back: the pass carries on where it was.
    std::fs::rename(&away, &folder).unwrap();
    let run = a.auto().await;
    assert_eq!((run.counts, run.paused), (counts(1, 0), None));
    let run = b.auto().await;
    assert_eq!((run.counts, run.paused), (counts(0, 1), None));
    assert_eq!(b.content(&first).await, "Ana, Ben, Cleo.");
    assert_eq!(b.page(&second).await.unwrap().status, "active");
    assert_eq!(a.auto().await.counts, zero());
    assert_eq!(b.auto().await.counts, zero());
}

#[tokio::test]
async fn another_members_sentences_are_kept_from_this_ai_but_not_from_this_person() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_path_buf();
    let a = Member::new("Qi-Xuan").await;
    let b = Member::new("Ana").await;
    // A's AI tool writes the page, then A adds a line in person.
    let id = crate::post_write::create_page(
        &a.db,
        CreateConceptRequest {
            title: "Venue".to_string(),
            content: "The venue is the old mill.".to_string(),
            summary: None,
            entity_id: None,
            space: SPACE.to_string().into(),
            source_memory_ids: Vec::new(),
            creation_kind: Some("authored".to_string()),
            workspace: None,
        },
        "claude-code",
        None,
    )
    .await
    .unwrap()
    .id;
    a.sync(&folder).await;
    b.sync(&folder).await;
    assert_eq!(
        b.history(&id).await[0].edited_by,
        format!("shared_ai:{}", a.member_id())
    );
    a.edit(&id, "The venue is the old mill. We booked it for June.")
        .await;
    a.sync(&folder).await;
    b.sync(&folder).await;

    let page = b.page(&id).await.unwrap();
    let rows = crate::sentences::page_authorship(&b.db, &page)
        .await
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| (row.author, row.shared))
            .collect::<Vec<_>>(),
        [
            (wenlan_types::authorship::SentenceAuthor::Ai, true),
            (wenlan_types::authorship::SentenceAuthor::Person, true),
        ]
    );
    let view = crate::sentences::page_sentences(&b.db, &id).await.unwrap();
    assert!(view.sentences.iter().all(|sentence| sentence.protected));

    // B's AI tool rewrites the page: A's sentences, its AI's included, stay;
    // the sentence it adds lands.
    let ai_write = |content: &'static str, version: i64| {
        crate::post_write::update_page(
            &b.db,
            &id,
            UpdatePageRequest {
                content: content.to_string(),
                source_memory_ids: Vec::new(),
                expected_version: Some(version),
                caller_id: None,
                operation_id: None,
            },
            "agent_refresh",
            false,
            None,
            None,
        )
    };
    let result = ai_write(
        "The venue is a barn. We booked it for May. Parking is free.",
        page.version,
    )
    .await
    .unwrap();
    assert!(result.wrote, "{result:?}");
    let content = b.content(&id).await;
    assert!(
        content.starts_with("The venue is the old mill. We booked it for June."),
        "{content}"
    );
    assert!(content.contains("Parking is free."), "{content}");
    // Its own sentence is the AI's to change, as before.
    let page = b.page(&id).await.unwrap();
    ai_write(
        "The venue is the old mill. We booked it for June. Parking costs five euros.",
        page.version,
    )
    .await
    .unwrap();
    assert_eq!(
        b.content(&id).await,
        "The venue is the old mill. We booked it for June. Parking costs five euros."
    );

    // B in person can change A's sentence; the line is then B's.
    b.edit(
        &id,
        "The venue is the old mill by the river. We booked it for June. Parking costs five euros.",
    )
    .await;
    let page = b.page(&id).await.unwrap();
    let rows = crate::sentences::page_authorship(&b.db, &page)
        .await
        .unwrap();
    assert_eq!(
        (rows[0].author, rows[0].shared),
        (wenlan_types::authorship::SentenceAuthor::Person, false)
    );
    assert!(rows[1].shared, "A's other line is still A's");
    settle(&a, &b, &folder).await;
    assert_eq!(a.content(&id).await, b.content(&id).await);
}

#[tokio::test]
async fn a_rename_on_either_side_reaches_the_other() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_path_buf();
    let a = Member::new("Qi-Xuan").await;
    let b = Member::new("Ana").await;
    let id = a
        .write("Menu", "Fish or pasta.", SPACE.to_string().into())
        .await;
    a.sync(&folder).await;
    b.sync(&folder).await;

    a.rename(&id, "Dinner menu").await;
    assert_eq!(a.sync(&folder).await, counts(1, 0));
    assert_eq!(b.sync(&folder).await, counts(0, 1));
    assert_eq!(b.page(&id).await.unwrap().title, "Dinner menu");
    assert_eq!(b.content(&id).await, "Fish or pasta.");

    b.rename(&id, "Wedding dinner").await;
    assert_eq!(b.sync(&folder).await, counts(1, 0));
    assert_eq!(a.sync(&folder).await, counts(0, 1));
    assert_eq!(a.page(&id).await.unwrap().title, "Wedding dinner");
    assert_eq!(a.sync(&folder).await, zero());
    assert_eq!(b.sync(&folder).await, zero());
}

#[tokio::test]
async fn a_title_taken_here_gets_the_writers_name_and_both_pages_stay() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_path_buf();
    let a = Member::new("Qi-Xuan").await;
    let b = Member::new("Ana").await;
    // Each makes a page called Budget before either has the other's.
    let mine = a
        .write("Budget", "Venue 4000.", SPACE.to_string().into())
        .await;
    let hers = b
        .write("Budget", "Flowers 300.", SPACE.to_string().into())
        .await;
    settle(&a, &b, &folder).await;

    for member in [&a, &b] {
        let pages = member
            .db
            .list_pages_by_space("active", Some(SPACE), 100, 0)
            .await
            .unwrap();
        assert_eq!(pages.len(), 2, "no page lost or merged");
        assert_eq!(member.content(&mine).await, "Venue 4000.");
        assert_eq!(member.content(&hers).await, "Flowers 300.");
        assert_eq!(member.page(&mine).await.unwrap().title, "Budget (Qi-Xuan)");
        assert_eq!(member.page(&hers).await.unwrap().title, "Budget (Ana)");
    }
}

#[tokio::test]
async fn an_archive_reaches_the_other_member_and_a_restore_brings_it_back() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_path_buf();
    let a = Member::new("Qi-Xuan").await;
    let b = Member::new("Ana").await;
    let id = a
        .write("Hotel", "Rooms for twelve.", SPACE.to_string().into())
        .await;
    a.sync(&folder).await;
    b.sync(&folder).await;

    a.db.archive_page(&id).await.unwrap();
    assert_eq!(a.sync(&folder).await, counts(1, 0));
    assert_eq!(b.sync(&folder).await, counts(0, 1));
    let page = b.page(&id).await.unwrap();
    assert_eq!(page.status, "archived");
    assert_eq!(page.content, "Rooms for twelve.");
    assert_eq!(a.sync(&folder).await, zero());
    assert_eq!(b.sync(&folder).await, zero());

    // Restored on B (the archive came from A): back on A too.
    b.restore(&id).await;
    assert_eq!(b.sync(&folder).await, counts(1, 0));
    assert_eq!(a.sync(&folder).await, counts(0, 1));
    assert_eq!(a.page(&id).await.unwrap().status, "active");
    assert_eq!(a.content(&id).await, "Rooms for twelve.");
    assert_eq!(a.sync(&folder).await, zero());
    assert_eq!(b.sync(&folder).await, zero());
}

#[tokio::test]
async fn a_page_deleted_here_is_not_made_again_and_is_archived_for_the_others() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_path_buf();
    let a = Member::new("Qi-Xuan").await;
    let b = Member::new("Ana").await;
    let id = a
        .write("Band", "The Swing Trio.", SPACE.to_string().into())
        .await;
    a.sync(&folder).await;
    b.sync(&folder).await;

    b.db.delete_page(&id).await.unwrap();
    assert_eq!(b.sync(&folder).await, counts(1, 0), "B writes an archive");
    assert!(b.page(&id).await.is_none(), "not made again on B");
    assert_eq!(a.sync(&folder).await, counts(0, 1));
    let page = a.page(&id).await.unwrap();
    assert_eq!(page.status, "archived", "archived, not gone, on A");
    assert_eq!(page.content, "The Swing Trio.");
    for _ in 0..2 {
        assert_eq!(b.sync(&folder).await, zero());
        assert_eq!(a.sync(&folder).await, zero());
    }
    assert!(b.page(&id).await.is_none());

    // A restores it: back on both.
    a.restore(&id).await;
    assert_eq!(a.sync(&folder).await, counts(1, 0));
    assert_eq!(b.sync(&folder).await, counts(0, 1));
    let page = b.page(&id).await.expect("back on B");
    assert_eq!(
        (page.status.as_str(), page.content.as_str()),
        ("active", "The Swing Trio.")
    );
    assert_eq!(a.sync(&folder).await, zero());
    assert_eq!(b.sync(&folder).await, zero());
}

#[tokio::test]
async fn one_page_is_shared_alone_and_lands_with_no_space() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().join("Venue (shared)");
    let a = Member::new("Qi-Xuan").await;
    let b = Member::new("Ana").await;
    let venue = a
        .write(
            "Venue",
            "The old mill, 150 seats.",
            "Private".to_string().into(),
        )
        .await;
    let other = a
        .write(
            "Budget",
            "Not for anyone else.",
            "Private".to_string().into(),
        )
        .await;
    let page = Scope::Page(venue.clone());

    assert_eq!(a.share(&page, &folder).await.unwrap(), counts(1, 0));
    let peek = peek_folder(&folder)
        .unwrap()
        .expect("the folder shares something");
    assert_eq!(peek.kind, wenlan_types::shared_space::SharedKind::Page);
    assert_eq!(peek.name, "Venue");
    assert_eq!(peek.page_id.as_deref(), Some(venue.as_str()));
    assert_eq!(peek.members.len(), 1);

    // B joins with the page id the folder names; B has no such page yet.
    assert_eq!(b.share(&page, &folder).await.unwrap(), counts(0, 1));
    let here = b.page(&venue).await.expect("made on B");
    assert_eq!(here.content, "The old mill, 150 seats.");
    assert_eq!(here.space, None, "no Space on B");
    assert!(b.page(&other).await.is_none(), "only the shared page");
    let text = walk(&folder);
    assert!(
        !text.contains("Not for anyone else."),
        "the other page never leaves A"
    );

    // Both ways, by the daemon's pass.
    b.edit(&venue, "The old mill, 120 to 150 seats.").await;
    let run = b.auto_all(None).await;
    assert_eq!(run.len(), 1);
    assert_eq!(run[0].0, format!("page:{venue}"));
    assert_eq!(run[0].1.counts, counts(1, 0), "{run:?}");
    let run = a.auto_all(None).await;
    assert_eq!(run[0].1.counts, counts(0, 1), "{run:?}");
    assert_eq!(a.content(&venue).await, "The old mill, 120 to 150 seats.");
    assert_eq!(
        a.page(&venue).await.unwrap().space.as_deref(),
        Some("Private"),
        "A keeps it in its own Space"
    );
    let status = share_status_at(&a.settings(), &page).unwrap().unwrap();
    assert_eq!(status.kind, wenlan_types::shared_space::SharedKind::Page);
    assert_eq!(status.target, venue);
    let mut names: Vec<_> = status.members.iter().map(|m| m.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["Ana", "Qi-Xuan"]);
}

#[tokio::test]
async fn a_folder_is_used_only_for_what_it_shares() {
    let place = tempfile::tempdir().unwrap();
    let space_folder = place.path().join("Wedding");
    let page_folder = place.path().join("Venue");
    let a = Member::new("Qi-Xuan").await;
    let venue = a
        .write("Venue", "The old mill.", "Private".to_string().into())
        .await;
    let guests = a
        .write("Guests", "Ana, Ben.", "Private".to_string().into())
        .await;
    a.sync(&space_folder).await;
    a.share(&Scope::Page(venue.clone()), &page_folder)
        .await
        .unwrap();

    let error = a
        .share(&Scope::Page(guests.clone()), &page_folder)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("another page"), "{error}");
    let error = a
        .share(&Scope::Page(guests.clone()), &space_folder)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("whole Space"), "{error}");
    let error = a
        .share(&Scope::Space("Private".into()), &page_folder)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("one page"), "{error}");
    // A page that is neither here nor the folder's cannot be joined.
    let error = a
        .share(&Scope::Page("nothere".into()), &page_folder)
        .await
        .unwrap_err();
    assert!(matches!(error, WenlanError::NotFound(_)), "{error}");
}

#[tokio::test]
async fn a_page_of_a_shared_space_is_not_shared_alone_and_sharing_its_space_takes_it_over() {
    let place = tempfile::tempdir().unwrap();
    let a = Member::new("Qi-Xuan").await;
    let inside = a.write("Cake", "Lemon.", SPACE.to_string().into()).await;
    let later = a
        .write("Venue", "The old mill.", "Private".to_string().into())
        .await;
    a.sync(&place.path().join("Wedding")).await;
    let error = a
        .share(&Scope::Page(inside.clone()), &place.path().join("Cake"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("shared already"), "{error}");

    // Shared alone first, then its whole Space: it goes on with the Space.
    a.share(&Scope::Page(later.clone()), &place.path().join("Venue"))
        .await
        .unwrap();
    a.share(
        &Scope::Space("Private".into()),
        &place.path().join("Private"),
    )
    .await
    .unwrap();
    assert!(
        share_settings_at(&a.settings(), &Scope::Page(later.clone()))
            .unwrap()
            .is_none()
    );
    let keys: Vec<_> = a
        .auto_all(None)
        .await
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(keys, ["Private", SPACE]);
}

#[tokio::test]
async fn leaving_stops_the_passes_and_keeps_everything() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_path_buf();
    let a = Member::new("Qi-Xuan").await;
    let id = a.write("Cake", "Lemon.", SPACE.to_string().into()).await;
    a.sync(&folder).await;
    assert!(leave_at(&a.settings(), &space()).unwrap());
    assert!(!leave_at(&a.settings(), &space()).unwrap());
    assert!(a.auto_all(None).await.is_empty());
    assert!(a.page(&id).await.is_some());
    assert!(
        walk(&folder).contains("Lemon."),
        "the folder keeps what was shared"
    );
}

#[tokio::test]
async fn the_profile_name_is_what_the_others_see() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_path_buf();
    let a = Member::new("Qi-Xuan").await;
    a.sync(&folder).await;
    a.auto_all(Some("  Qi-Xuan Lu ")).await;
    let status = share_status_at(&a.settings(), &space()).unwrap().unwrap();
    assert_eq!(status.member_name, "Qi-Xuan Lu");
    assert_eq!(
        status.members,
        vec![wenlan_types::shared_space::SharedMember {
            id: status.member_id.clone(),
            name: "Qi-Xuan Lu".into()
        }]
    );
    // An empty profile name changes nothing.
    a.auto_all(Some(" ")).await;
    assert_eq!(
        share_status_at(&a.settings(), &space())
            .unwrap()
            .unwrap()
            .member_name,
        "Qi-Xuan Lu"
    );
}
