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
                member_id: None,
                host: None,
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
                member_id: None,
                host: None,
            },
        )
        .await
    }

    /// Joins `folder` as `member_id`: this person's own other computer.
    async fn join_as(
        &self,
        folder: &Path,
        member_id: &str,
    ) -> Result<SharedSyncCounts, WenlanError> {
        sync_space(
            &self.db,
            SyncTarget {
                scope: &space(),
                folder,
                member_name: self.name,
                settings_path: &self.settings(),
                knowledge_path: None,
                member_id: Some(member_id),
                host: None,
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
        .filter(|(path, _)| {
            path.extension()
                .is_some_and(|ext| ext == version::VERSION_EXT)
        })
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
    // The two edits in between are in B's history too, as on A.
    let texts: Vec<String> = b
        .history(&id)
        .await
        .into_iter()
        .map(|row| row.content)
        .collect();
    assert_eq!(
        texts[..3],
        [
            "Table 1: Ana, Cleo, Dev.",
            "Table 1: Ana, Cleo.",
            "Table 1: Ana."
        ]
    );
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
    let b_edit = b.page(&id).await.unwrap().version;
    tick().await;
    // A's edit is later, so it wins everywhere; B's stays a version.
    assert_eq!(a.sync(&folder).await.exported, 1);
    let got = b.sync(&folder).await;
    assert_eq!(got.applied, 1, "{got:?}");
    assert_eq!(got.conflicts, 1, "{got:?}");
    assert_eq!(b.content(&id).await, "Fish, pasta or risotto.");
    // B's page says which of its versions lost, so the app can tell B once.
    let b_now = b.page(&id).await.unwrap();
    let changelog: Vec<serde_json::Value> =
        serde_json::from_str(&b.db.get_page_changelog(&id).await.unwrap()).unwrap();
    let last = changelog.last().unwrap();
    assert_eq!(last["version"], serde_json::json!(b_now.version));
    assert_eq!(last["concurrent_with"], serde_json::json!(b_edit));
    let writer = b_now.last_edited_by.unwrap_or_default();
    assert!(
        writer.starts_with("shared_") && writer.ends_with(&a.member_id()),
        "{writer}"
    );
    // A's own edit won, so A has nothing to be told.
    let a_log = a.db.get_page_changelog(&id).await.unwrap();
    assert!(!a_log.contains("concurrent_with"), "{a_log}");
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
    // The page's rows name who wrote it, from the first version on.
    assert_eq!(
        b.page(&id).await.unwrap().last_edited_by,
        Some(format!("shared_ai:{}", a.member_id()))
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
    // The app names who wrote each one.
    assert_eq!(
        view.sentences
            .iter()
            .map(|sentence| sentence.shared_by.clone())
            .collect::<Vec<_>>(),
        [
            Some(format!("shared_ai:{}", a.member_id())),
            Some(format!("shared_person:{}", a.member_id())),
        ]
    );

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

/// Turns every version `member` wrote into a cloud file that is not
/// downloaded yet (an iCloud placeholder), so a pass cannot read it.
fn evict_versions_of(folder: &Path, member: &str) -> usize {
    let mut evicted = 0;
    for (path, _) in folder_files(&folder.join(version::MEMBERS_DIR).join(member)) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if version::version_sha(&name).is_some() {
            std::fs::rename(&path, path.with_file_name(format!(".{name}.icloud"))).unwrap();
            evicted += 1;
        }
    }
    evicted
}

#[tokio::test]
async fn a_version_this_member_cannot_read_yet_is_never_written_over_the_others_edit() {
    // Found on two real computers: the Mac's own last version was a Google
    // Drive file not downloaded yet, and the pass shared the Mac's older
    // page over the Windows edit as if it had been edited here.
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().join("Wedding (shared)");
    let a = Member::new("Qi-Xuan").await;
    let b = Member::new("Ana").await;
    let id = a
        .write("Seating plan", "Table 1: Ana.", SPACE.to_string().into())
        .await;
    assert_eq!(a.sync(&folder).await.exported, 1);
    assert_eq!(b.sync(&folder).await.applied, 1);
    b.edit(&id, "Table 1: Ana.\nTable 2: Ben.").await;
    assert_eq!(b.sync(&folder).await.exported, 1);
    assert_eq!(evict_versions_of(&folder, &a.member_id()), 1);
    let before = version_count(&folder);
    let got = a.sync(&folder).await;
    assert_eq!(got.exported, 0, "{got:?}");
    assert_eq!(got.applied, 1, "{got:?}");
    assert_eq!(a.content(&id).await, "Table 1: Ana.\nTable 2: Ben.");
    assert_eq!(version_count(&folder), before, "nothing new was written");
    assert_eq!(a.sync(&folder).await.exported, 0);
}

#[tokio::test]
async fn an_edit_made_over_a_version_not_readable_yet_waits_for_it() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().join("Wedding (shared)");
    let a = Member::new("Qi-Xuan").await;
    let b = Member::new("Ana").await;
    let id = a
        .write("Seating plan", "Table 1: Ana.", SPACE.to_string().into())
        .await;
    assert_eq!(a.sync(&folder).await.exported, 1);
    assert_eq!(b.sync(&folder).await.applied, 1);
    b.edit(&id, "Table 1: Ana.\nTable 2: Ben.").await;
    assert_eq!(b.sync(&folder).await.exported, 1);

    // A edits while its own last version is not downloaded yet: the pass
    // waits instead of writing A's edit over a version it cannot see.
    a.edit(&id, "Table 1: Ana, Cleo.").await;
    evict_versions_of(&folder, &a.member_id());
    let before = version_count(&folder);
    let got = a.sync(&folder).await;
    assert_eq!((got.exported, got.applied), (0, 0), "{got:?}");
    assert!(got.skipped > 0, "{got:?}");
    assert_eq!(version_count(&folder), before);
    assert_eq!(a.content(&id).await, "Table 1: Ana, Cleo.");
}

#[tokio::test]
async fn two_computers_of_one_person_are_one_member() {
    let folder = tempfile::tempdir().unwrap();
    let mac = Member::new("Lucian").await;
    let windows = Member::new("Lucian").await;
    let id = mac
        .write("Plan", "We marry in May.", SPACE.to_string().into())
        .await;
    mac.sync(folder.path()).await;
    let me = mac.member_id();

    // A member id the folder does not list is refused.
    assert!(windows
        .join_as(folder.path(), "someone_else")
        .await
        .is_err());
    let counts = windows.join_as(folder.path(), &me).await.unwrap();
    assert_eq!(counts.applied, 1);
    assert_eq!(windows.member_id(), me);

    // Taken as the person's own, not as another member's.
    let rows = windows.db.list_page_history(&id, 1).await.unwrap();
    assert_eq!(rows[0].edited_by, OWN_DEVICE_PERSON);
    let file = read_space_file(folder.path()).unwrap().unwrap();
    assert_eq!(file.members.len(), 1, "one person is one member");

    windows.edit(&id, "We marry in June.").await;
    windows.sync(folder.path()).await;
    let counts = mac.sync(folder.path()).await;
    assert_eq!(counts.applied, 1);
    assert_eq!(mac.page(&id).await.unwrap().content, "We marry in June.");
    let rows = mac.db.list_page_history(&id, 1).await.unwrap();
    assert_eq!(rows[0].edited_by, OWN_DEVICE_PERSON);
    assert_eq!(
        crate::authorship::version_author(&rows[0].edited_by, "authored"),
        crate::authorship::VersionAuthor::Person
    );

    // Joining again as someone else is refused once joined.
    assert!(windows.join_as(folder.path(), "another").await.is_err());

    // Different profile names on the two computers do not rename the
    // member back and forth on every pass; changing one's name does once.
    let names = || {
        read_space_file(folder.path())
            .unwrap()
            .unwrap()
            .members
            .into_iter()
            .map(|member| member.name)
            .collect::<Vec<_>>()
    };
    windows.auto_all(Some("h1646")).await;
    assert_eq!(names(), ["h1646"], "the name changed on that computer");
    mac.auto_all(Some("Lucian")).await;
    windows.auto_all(Some("h1646")).await;
    mac.auto_all(Some("Lucian")).await;
    assert_eq!(names(), ["h1646"], "no pass renames it back");
    mac.auto_all(Some("Lucian Chen")).await;
    assert_eq!(names(), ["Lucian Chen"]);
}

#[tokio::test]
async fn a_member_who_may_only_read_takes_the_shared_version_over_their_edit() {
    let folder = tempfile::tempdir().unwrap();
    let amy = Member::new("Amy").await;
    let bo = Member::new("Bo").await;
    let id = amy
        .write("Plan", "We marry in May.", SPACE.to_string().into())
        .await;
    amy.sync(folder.path()).await;
    bo.sync(folder.path()).await;
    update_share(&bo.settings(), &space(), |settings| {
        settings.read_only = true
    })
    .unwrap()
    .unwrap();

    bo.edit(&id, "We marry in July.").await;
    let counts = bo.sync(folder.path()).await;
    assert_eq!(counts.exported, 0, "a reader writes no version");
    assert_eq!(bo.page(&id).await.unwrap().content, "We marry in May.");
    let read = version::read_folder(folder.path());
    assert_eq!(read.pages[&id].len(), 1);
}

// ===== The whole library, between one person's own computers =====

fn library() -> Scope {
    Scope::Library
}

impl Member {
    async fn lib_sync(
        &self,
        folder: &Path,
        member_id: Option<&str>,
    ) -> Result<SharedSyncCounts, WenlanError> {
        sync_space(
            &self.db,
            SyncTarget {
                scope: &library(),
                folder,
                member_name: self.name,
                settings_path: &self.settings(),
                knowledge_path: None,
                member_id,
                host: None,
            },
        )
        .await
    }

    fn library_member_id(&self) -> String {
        share_settings_at(&self.settings(), &library())
            .unwrap()
            .unwrap()
            .member_id
    }

    async fn space_of(&self, id: &str) -> Option<String> {
        self.page(id).await.unwrap().space
    }

    async fn move_to(&self, id: &str, space: Option<&str>) {
        let page = self.page(id).await.unwrap();
        self.db
            .set_page_space(id, space, page.version, &ReadScope::Global)
            .await
            .unwrap();
    }

    async fn archive(&self, id: &str) {
        let page = self.page(id).await.unwrap();
        self.db
            .archive_page_at_version(id, page.version)
            .await
            .unwrap()
            .unwrap();
    }
}

/// A's library, then B joined as the same person.
async fn library_pair(folder: &Path) -> (Member, Member) {
    let a = Member::new("Lucian").await;
    let b = Member::new("h1646").await;
    a.lib_sync(folder, None).await.unwrap();
    let me = a.library_member_id();
    b.lib_sync(folder, Some(&me)).await.unwrap();
    (a, b)
}

async fn lib_settle(a: &Member, b: &Member, folder: &Path) {
    let me = a.library_member_id();
    for _ in 0..5 {
        let got_a = a.lib_sync(folder, None).await.unwrap();
        let got_b = b.lib_sync(folder, Some(&me)).await.unwrap();
        if got_a == zero() && got_b == zero() {
            return;
        }
    }
    panic!("the two computers did not settle");
}

#[tokio::test]
async fn the_library_reaches_the_other_computer_in_the_same_spaces_with_unfiled_and_archived() {
    let folder = tempfile::tempdir().unwrap();
    let a = Member::new("Lucian").await;
    let b = Member::new("h1646").await;
    a.db.create_space("Travel", None, false).await.unwrap();
    let plan = a
        .write("Plan", "We marry in May.", SPACE.to_string().into())
        .await;
    let trip = a
        .write("Trip", "Lisbon in June.", "Travel".to_string().into())
        .await;
    let loose = a
        .write("Loose", "No Space yet.", WriteSpaceTarget::Uncategorized)
        .await;
    let old = a
        .write("Old", "Gone but kept.", "Private".to_string().into())
        .await;
    a.archive(&old).await;

    // The first computer makes the folder and mints the person's id.
    let counts = a.lib_sync(folder.path(), None).await.unwrap();
    assert_eq!(counts.exported, 4, "archived pages go too");
    let file = read_space_file(&library::engine_folder(folder.path()))
        .unwrap()
        .unwrap();
    assert!(file.library);
    assert_eq!(file.page, None);
    assert_eq!(file.members.len(), 1);
    assert_eq!(file.members[0].id, a.library_member_id());
    let peek = peek_folder(folder.path()).unwrap().unwrap();
    assert_eq!(peek.kind, wenlan_types::shared_space::SharedKind::Library);

    // The second computer joins with that id and takes everything.
    assert!(b.db.get_space("Travel").await.unwrap().is_none());
    let counts = b
        .lib_sync(folder.path(), Some(&a.library_member_id()))
        .await
        .unwrap();
    assert_eq!(counts.applied, 4);
    assert_eq!(b.library_member_id(), a.library_member_id());
    assert!(
        b.db.get_space("Travel").await.unwrap().is_some(),
        "the Space is made on the other computer"
    );
    assert_eq!(b.space_of(&plan).await.as_deref(), Some(SPACE));
    assert_eq!(b.space_of(&trip).await.as_deref(), Some("Travel"));
    assert_eq!(b.space_of(&loose).await, None);
    assert_eq!(b.page(&loose).await.unwrap().status, "active");
    let old_here = b.page(&old).await.unwrap();
    assert_eq!(old_here.status, "archived");
    assert_eq!(old_here.space.as_deref(), Some("Private"));
    assert_eq!(b.content(&trip).await, "Lisbon in June.");
    // One person is one member.
    let file = read_space_file(&library::engine_folder(folder.path()))
        .unwrap()
        .unwrap();
    assert_eq!(file.members.len(), 1);

    lib_settle(&a, &b, folder.path()).await;
    // The Unfiled page carries no Space; the others do.
    let text = walk(&library::engine_folder(folder.path()));
    assert_eq!(text.matches("\nspace: Travel\n").count(), 1);
    assert_eq!(text.matches("\nspace: Wedding\n").count(), 1);
}

#[tokio::test]
async fn an_edit_on_the_other_computer_arrives_as_the_persons_own_writing() {
    let folder = tempfile::tempdir().unwrap();
    let (a, b) = library_pair(folder.path()).await;
    let id = a
        .write("Plan", "We marry in May.", SPACE.to_string().into())
        .await;
    lib_settle(&a, &b, folder.path()).await;
    assert_eq!(b.content(&id).await, "We marry in May.");
    let rows = b.db.list_page_history(&id, 1).await.unwrap();
    assert_eq!(rows[0].edited_by, OWN_DEVICE_PERSON);

    b.edit(&id, "We marry in June.").await;
    tick().await;
    lib_settle(&a, &b, folder.path()).await;
    assert_eq!(a.content(&id).await, "We marry in June.");
    let rows = a.db.list_page_history(&id, 1).await.unwrap();
    assert_eq!(rows[0].edited_by, OWN_DEVICE_PERSON, "not another person");
    assert_eq!(
        crate::authorship::version_author(&rows[0].edited_by, "authored"),
        crate::authorship::VersionAuthor::Person
    );
    // Still one member, and nobody's writing is filed under another id.
    let file = read_space_file(&library::engine_folder(folder.path()))
        .unwrap()
        .unwrap();
    assert_eq!(file.members.len(), 1);
    assert!(a
        .history(&id)
        .await
        .iter()
        .all(|row| shared_writer(&row.edited_by).is_none()));
}

#[tokio::test]
async fn moving_a_page_to_another_space_or_making_a_space_follows_to_the_other_computer() {
    let folder = tempfile::tempdir().unwrap();
    let (a, b) = library_pair(folder.path()).await;
    let plan = a
        .write("Plan", "We marry in May.", SPACE.to_string().into())
        .await;
    let notes = a
        .write("Notes", "Some notes.", "Private".to_string().into())
        .await;
    lib_settle(&a, &b, folder.path()).await;
    assert_eq!(b.space_of(&plan).await.as_deref(), Some(SPACE));

    // A move to another Space that is there on both.
    a.move_to(&plan, Some("Private")).await;
    let counts = a.lib_sync(folder.path(), None).await.unwrap();
    assert_eq!(counts.exported, 1, "a move is a change");
    let counts = b
        .lib_sync(folder.path(), Some(&a.library_member_id()))
        .await
        .unwrap();
    assert_eq!(counts.applied, 1);
    assert_eq!(counts.conflicts, 0);
    assert_eq!(b.space_of(&plan).await.as_deref(), Some("Private"));
    assert_eq!(b.content(&plan).await, "We marry in May.");

    // A new Space on A, with a page moved into it.
    a.db.create_space("Garden", None, false).await.unwrap();
    a.move_to(&notes, Some("Garden")).await;
    lib_settle(&a, &b, folder.path()).await;
    assert!(b.db.get_space("Garden").await.unwrap().is_some());
    assert_eq!(b.space_of(&notes).await.as_deref(), Some("Garden"));

    // Out of every Space, then moved on the other computer.
    a.move_to(&notes, None).await;
    lib_settle(&a, &b, folder.path()).await;
    assert_eq!(b.space_of(&notes).await, None);
    b.move_to(&notes, Some(SPACE)).await;
    tick().await;
    lib_settle(&a, &b, folder.path()).await;
    assert_eq!(a.space_of(&notes).await.as_deref(), Some(SPACE));
    // Settled: nothing written when nothing changed.
    assert_eq!(
        a.lib_sync(folder.path(), None).await.unwrap(),
        zero(),
        "idle passes write nothing"
    );
}

#[tokio::test]
async fn pages_with_their_own_share_keep_their_own_folder() {
    let library_folder = tempfile::tempdir().unwrap();
    let wedding_folder = tempfile::tempdir().unwrap();
    let page_folder = tempfile::tempdir().unwrap();
    let a = Member::new("Lucian").await;
    let plan = a
        .write("Plan", "We marry in May.", SPACE.to_string().into())
        .await;
    let notes = a
        .write("Notes", "Some notes.", "Private".to_string().into())
        .await;
    let alone = a
        .write("Alone", "Shared by itself.", "Private".to_string().into())
        .await;
    a.share(&space(), wedding_folder.path()).await.unwrap();
    a.share(&Scope::Page(alone.clone()), page_folder.path())
        .await
        .unwrap();

    let counts = a.lib_sync(library_folder.path(), None).await.unwrap();
    assert_eq!(counts.exported, 1, "only Notes is in the library");
    let text = walk(&library::engine_folder(library_folder.path()));
    assert!(text.contains("Some notes."));
    assert!(
        !text.contains("We marry in May."),
        "a shared Space stays out"
    );
    assert!(
        !text.contains("Shared by itself."),
        "a shared page stays out"
    );
    assert_eq!(version_count(wedding_folder.path()), 1);
    assert_eq!(version_count(page_folder.path()), 1);

    // The daemon's pass runs it with the others.
    let keys: Vec<String> = a
        .auto_all(None)
        .await
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(
        keys,
        [
            SPACE.to_string(),
            format!("page:{alone}"),
            "library:".to_string()
        ]
    );
    let shares = shares_at(&a.settings()).unwrap();
    assert!(shares.iter().any(|(scope, _)| *scope == Scope::Library));
    assert_eq!(
        share_status_at(&a.settings(), &library())
            .unwrap()
            .unwrap()
            .kind,
        wenlan_types::shared_space::SharedKind::Library
    );

    // The other computer does not get them through the library either, and
    // a page here that the folder never names is left alone.
    let b = Member::new("h1646").await;
    let counts = b
        .lib_sync(library_folder.path(), Some(&a.library_member_id()))
        .await
        .unwrap();
    assert_eq!(counts.applied, 1);
    assert!(b.page(&notes).await.is_some());
    assert!(b.page(&plan).await.is_none());
    assert!(b.page(&alone).await.is_none());

    // A page that gets its own share later leaves the library: nothing of
    // it is written there again, and it is not touched.
    a.edit(&notes, "Some more notes.").await;
    a.share(&Scope::Page(notes.clone()), page_folder.path())
        .await
        .unwrap_err(); // that folder shares another page
    let other = tempfile::tempdir().unwrap();
    a.share(&Scope::Page(notes.clone()), other.path())
        .await
        .unwrap();
    assert_eq!(
        a.lib_sync(library_folder.path(), None).await.unwrap(),
        zero()
    );
    assert_eq!(
        version_count(&library::engine_folder(library_folder.path())),
        1
    );

    // Stopping leaves the pages and the folder as they are.
    assert!(leave_at(&a.settings(), &library()).unwrap());
    assert!(share_status_at(&a.settings(), &library())
        .unwrap()
        .is_none());
    assert!(library::engine_folder(library_folder.path())
        .join(SPACE_FILE)
        .exists());
    assert!(a.page(&notes).await.is_some());
    let raw = std::fs::read_to_string(a.settings()).unwrap();
    assert!(!raw.contains("\"library\""), "left out when not shared");
}

#[tokio::test]
async fn a_library_folder_is_not_a_space_folder_and_titles_clash_only_within_a_space() {
    let folder = tempfile::tempdir().unwrap();
    let a = Member::new("Lucian").await;
    a.lib_sync(folder.path(), None).await.unwrap();
    // Neither kind of folder is used for the other.
    assert!(a
        .share(&space(), &library::engine_folder(folder.path()))
        .await
        .is_err());
    let space_folder = tempfile::tempdir().unwrap();
    a.share(&space(), space_folder.path()).await.unwrap();
    // A library keeps its files in a subfolder of the chosen one, so the
    // chosen folder may hold a Space's `space.json` beside it.
    let page_folder = tempfile::tempdir().unwrap();
    let id = a
        .write("Alone", "By itself.", WriteSpaceTarget::Uncategorized)
        .await;
    a.share(&Scope::Page(id.clone()), page_folder.path())
        .await
        .unwrap();
    assert!(a
        .share(&Scope::Page(id), &library::engine_folder(folder.path()))
        .await
        .is_err());

    // The same title in another Space is no clash; in the same Space it is.
    let a_plan_wedding = a
        .write("Plan", "Wedding plan.", "Private".to_string().into())
        .await;
    let a_plan_private = a
        .write("Budget", "Private budget.", "Private".to_string().into())
        .await;
    let b = Member::new("h1646").await;
    let b_plan_other = b
        .write("Plan", "Own plan, other Space.", SPACE.to_string().into())
        .await;
    let b_budget_same = b
        .write("Budget", "Own budget.", "Private".to_string().into())
        .await;
    a.lib_sync(folder.path(), None).await.unwrap();
    b.lib_sync(folder.path(), Some(&a.library_member_id()))
        .await
        .unwrap();
    assert_eq!(b.page(&a_plan_wedding).await.unwrap().title, "Plan");
    assert_eq!(b.page(&b_plan_other).await.unwrap().title, "Plan");
    assert_eq!(
        b.page(&a_plan_private).await.unwrap().title,
        "Budget (Lucian)"
    );
    assert_eq!(b.page(&b_budget_same).await.unwrap().title, "Budget");
}

// ===== Layout, computers, the main computer and its jobs =====

fn engine(chosen: &Path) -> PathBuf {
    library::engine_folder(chosen)
}

fn copy_marker(chosen: &Path) -> PathBuf {
    library::read_folder(chosen).join(crate::reading_copy::INSTRUCTIONS_FILE)
}

fn this_device(member: &Member) -> String {
    share_settings_at(&member.settings(), &library())
        .unwrap()
        .unwrap()
        .device_id
}

fn age(path: &Path, seconds: u64) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(seconds))
        .unwrap();
}

#[tokio::test]
async fn the_engine_lives_in_a_subfolder_of_the_chosen_folder_and_peek_finds_it() {
    let chosen = tempfile::tempdir().unwrap();
    let a = Member::new("Lucian").await;
    a.lib_sync(chosen.path(), None).await.unwrap();
    assert!(!chosen.path().join(SPACE_FILE).exists());
    assert!(engine(chosen.path()).join(SPACE_FILE).exists());
    assert_eq!(
        library::ENGINE_DIR,
        "同步資料（請勿打開）",
        "the folder name is a constant of the format"
    );
    // Settings keep the folder the person chose.
    let settings = share_settings_at(&a.settings(), &library())
        .unwrap()
        .unwrap();
    assert_eq!(Path::new(&settings.folder), chosen.path());
    let peek = peek_folder(chosen.path()).unwrap().unwrap();
    assert_eq!(peek.kind, wenlan_types::shared_space::SharedKind::Library);
    // A folder with no engine subfolder shares nothing.
    let empty = tempfile::tempdir().unwrap();
    assert!(peek_folder(empty.path()).unwrap().is_none());
    // The status reads the members from the engine subfolder.
    let status = share_status_at(&a.settings(), &library()).unwrap().unwrap();
    assert_eq!(status.members.len(), 1);
    assert_eq!(status.folder, chosen.path().to_string_lossy());
}

#[tokio::test]
async fn computers_are_added_once_and_the_first_is_main_and_can_be_changed() {
    let chosen = tempfile::tempdir().unwrap();
    let (a, b) = library_pair(chosen.path()).await;
    let (id_a, id_b) = (this_device(&a), this_device(&b));
    assert_ne!(id_a, id_b);
    let file = read_space_file(&engine(chosen.path())).unwrap().unwrap();
    assert_eq!(file.devices.len(), 2);
    assert_eq!(file.main_device, id_a, "the first to share is main");
    lib_settle(&a, &b, chosen.path()).await;

    // Later passes never rewrite space.json.
    let path = engine(chosen.path()).join(SPACE_FILE);
    let before = (
        std::fs::read(&path).unwrap(),
        std::fs::metadata(&path).unwrap().modified().unwrap(),
    );
    tick().await;
    for _ in 0..3 {
        a.lib_sync(chosen.path(), None).await.unwrap();
        b.lib_sync(chosen.path(), Some(&id_a_member(&a)))
            .await
            .unwrap();
    }
    a.auto_all(None).await;
    let after = (
        std::fs::read(&path).unwrap(),
        std::fs::metadata(&path).unwrap().modified().unwrap(),
    );
    assert_eq!(before.0, after.0.clone());
    assert_eq!(before.1, after.1, "the file was not written again");
    // The device id is kept across passes.
    assert_eq!(this_device(&a), id_a);

    let status = share_status_at(&b.settings(), &library()).unwrap().unwrap();
    assert_eq!(status.devices.len(), 2);
    assert_eq!(status.main_device.as_deref(), Some(id_a.as_str()));
    assert_eq!(status.this_device.as_deref(), Some(id_b.as_str()));
    // A Space's status carries none of it.
    a.share(&space(), tempfile::tempdir().unwrap().path())
        .await
        .unwrap();
    let space_status = share_status_at(&a.settings(), &space()).unwrap().unwrap();
    assert!(space_status.devices.is_empty());
    assert!(space_status.main_device.is_none() && space_status.this_device.is_none());

    // The person picks another main computer; only a listed one will do.
    assert!(library::set_main_device(chosen.path(), "nobody").is_err());
    library::set_main_device(chosen.path(), &id_b).unwrap();
    let status = share_status_at(&a.settings(), &library()).unwrap().unwrap();
    assert_eq!(status.main_device.as_deref(), Some(id_b.as_str()));
}

fn id_a_member(a: &Member) -> String {
    a.library_member_id()
}

#[tokio::test]
async fn the_read_copy_is_written_only_by_the_main_computer() {
    let chosen = tempfile::tempdir().unwrap();
    let (a, b) = library_pair(chosen.path()).await;
    let plan = a
        .write("Plan", "We marry in May.", SPACE.to_string().into())
        .await;
    lib_settle(&a, &b, chosen.path()).await;
    let copy = library::read_folder(chosen.path());
    assert!(copy_marker(chosen.path()).exists());
    assert!(
        walk(&copy).contains("We marry in May."),
        "the main computer wrote the page"
    );
    assert!(a
        .db
        .get_app_metadata(library::COPY_STATE_KEY)
        .await
        .unwrap()
        .is_some());
    assert!(b
        .db
        .get_app_metadata(library::COPY_STATE_KEY)
        .await
        .unwrap()
        .is_none());

    // Not the main computer: it writes nothing there.
    std::fs::remove_dir_all(&copy).unwrap();
    b.edit(&plan, "We marry in June.").await;
    b.lib_sync(chosen.path(), Some(&a.library_member_id()))
        .await
        .unwrap();
    assert!(!copy.exists(), "a computer that is not main writes no copy");

    // Once the person makes it main, it does.
    library::set_main_device(chosen.path(), &this_device(&b)).unwrap();
    b.lib_sync(chosen.path(), Some(&a.library_member_id()))
        .await
        .unwrap();
    assert!(walk(&copy).contains("We marry in June."));
    let status = share_status_at(&b.settings(), &library()).unwrap().unwrap();
    assert_eq!(status.reading_copy_error, None);
}

#[tokio::test]
async fn an_unwritable_read_copy_is_shown_in_the_status_and_never_fails_the_sync() {
    let chosen = tempfile::tempdir().unwrap();
    let a = Member::new("Lucian").await;
    a.lib_sync(chosen.path(), None).await.unwrap();
    // A file where the folder should be.
    std::fs::remove_dir_all(library::read_folder(chosen.path())).unwrap();
    std::fs::write(library::read_folder(chosen.path()), "in the way").unwrap();
    a.write("Plan", "We marry in May.", SPACE.to_string().into())
        .await;
    let counts = a.lib_sync(chosen.path(), None).await.unwrap();
    assert_eq!(counts.exported, 1);
    let status = share_status_at(&a.settings(), &library()).unwrap().unwrap();
    assert!(status.reading_copy_error.is_some());
    assert!(status.last.unwrap().error.is_none());
}

#[tokio::test]
async fn the_inbox_is_taken_only_by_the_main_computer_and_files_are_moved_not_deleted() {
    let chosen = tempfile::tempdir().unwrap();
    let (a, b) = library_pair(chosen.path()).await;
    let inbox = library::inbox_folder(chosen.path());
    std::fs::create_dir_all(inbox.join("sub")).unwrap();
    let ready = inbox.join("Trip idea.md");
    std::fs::write(&ready, "Lisbon in June, book the flights early.").unwrap();
    age(&ready, 60);
    let text = inbox.join("Note.TXT");
    std::fs::write(&text, "A plain text note from my phone.").unwrap();
    age(&text, 60);
    let fresh = inbox.join("Fresh.md");
    std::fs::write(&fresh, "Still arriving from the cloud drive.").unwrap();
    let hidden = inbox.join(".hidden.md");
    std::fs::write(&hidden, "A dotfile is never taken.").unwrap();
    age(&hidden, 60);
    let other = inbox.join("Photo.png");
    std::fs::write(&other, "not text").unwrap();
    age(&other, 60);
    let nested = inbox.join("sub/Nested.md");
    std::fs::write(&nested, "Only the top level is taken.").unwrap();
    age(&nested, 60);
    let short = inbox.join("Short.md");
    std::fs::write(&short, "hi").unwrap();
    age(&short, 60);

    // B is not main: nothing is taken.
    b.lib_sync(chosen.path(), Some(&a.library_member_id()))
        .await
        .unwrap();
    assert!(ready.exists() && text.exists());
    assert!(b
        .db
        .list_memories(None, None, None, None, 50)
        .await
        .unwrap()
        .is_empty());

    // A is.
    a.lib_sync(chosen.path(), None).await.unwrap();
    assert!(!ready.exists() && !text.exists());
    let taken = inbox.join(library::TAKEN_DIR);
    assert_eq!(
        std::fs::read_to_string(taken.join("Trip idea.md")).unwrap(),
        "Lisbon in June, book the flights early."
    );
    assert!(taken.join("Note.TXT").exists());
    for left in [&fresh, &hidden, &other, &nested, &short] {
        assert!(left.exists(), "{} stays", left.display());
    }
    let memories =
        a.db.list_memories(None, None, None, None, 50)
            .await
            .unwrap();
    assert_eq!(memories.len(), 2);
    let trip = memories.iter().find(|m| m.title == "Trip idea").unwrap();
    assert!(trip.source_id.starts_with("manual_"));
    assert!(trip.content.contains("Lisbon in June"));
    assert_eq!(trip.space, None);

    // Taken once: another pass takes nothing, and a name clash is numbered.
    a.lib_sync(chosen.path(), None).await.unwrap();
    assert_eq!(
        a.db.list_memories(None, None, None, None, 50)
            .await
            .unwrap()
            .len(),
        2
    );
    std::fs::write(&ready, "A second idea with the same file name.").unwrap();
    age(&ready, 60);
    a.lib_sync(chosen.path(), None).await.unwrap();
    assert!(taken.join("Trip idea.md").exists() && taken.join("Trip idea (2).md").exists());
    assert_eq!(
        a.db.list_memories(None, None, None, None, 50)
            .await
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn only_the_main_computer_lets_ai_write_pages() {
    let chosen = tempfile::tempdir().unwrap();
    let a = Member::new("Lucian").await;
    // Sharing nothing: no gate.
    assert!(library::page_writing_allowed_at(&a.settings()));
    let (a, b) = library_pair(chosen.path()).await;
    assert!(library::page_writing_allowed_at(&a.settings()), "main");
    assert!(
        !library::page_writing_allowed_at(&b.settings()),
        "the other computer waits for the main one"
    );
    library::set_main_device(chosen.path(), &this_device(&b)).unwrap();
    assert!(library::page_writing_allowed_at(&b.settings()));
    assert!(!library::page_writing_allowed_at(&a.settings()));
    // The folder cannot be read now: the last answer stands.
    std::fs::rename(engine(chosen.path()), chosen.path().join("away")).unwrap();
    assert!(library::page_writing_allowed_at(&b.settings()));
    assert!(!library::page_writing_allowed_at(&a.settings()));
}

#[tokio::test]
async fn a_pass_that_would_archive_most_of_the_library_archives_none() {
    let chosen = tempfile::tempdir().unwrap();
    let (a, b) = library_pair(chosen.path()).await;
    let mut ids = Vec::new();
    for n in 0..25 {
        ids.push(
            a.write(
                &format!("Page {n}"),
                &format!("Body {n}."),
                SPACE.to_string().into(),
            )
            .await,
        );
    }
    lib_settle(&a, &b, chosen.path()).await;
    // A few archives are applied as always.
    for id in &ids[..3] {
        a.archive(id).await;
    }
    lib_settle(&a, &b, chosen.path()).await;
    assert_eq!(b.page(&ids[0]).await.unwrap().status, "archived");

    // Then the rest at once: more than 20, and most of the library.
    for id in &ids[3..] {
        a.archive(id).await;
    }
    a.lib_sync(chosen.path(), None).await.unwrap();
    let counts = b
        .lib_sync(chosen.path(), Some(&a.library_member_id()))
        .await
        .unwrap();
    assert_eq!(counts.applied, 0);
    assert_eq!(counts.held_back, 22);
    for id in &ids[3..] {
        assert_eq!(b.page(id).await.unwrap().status, "active");
    }
    let last = share_status_at(&b.settings(), &library())
        .unwrap()
        .unwrap()
        .last
        .unwrap();
    assert!(last.held_back.is_some(), "the status says why");
    // Still held on the next pass; nothing is lost.
    let counts = b
        .lib_sync(chosen.path(), Some(&a.library_member_id()))
        .await
        .unwrap();
    assert_eq!(counts.held_back, 22);
}

// ===== The library's memories =====

impl Member {
    /// A memory saved the way a capture is stored.
    async fn save_memory(&self, id: &str, text: &str, space: Option<&str>) {
        self.db
            .upsert_documents(vec![crate::sources::RawDocument {
                source: "memory".to_string(),
                source_id: id.to_string(),
                title: text.chars().take(30).collect(),
                content: text.to_string(),
                last_modified: chrono::Utc::now().timestamp(),
                memory_type: Some("fact".to_string()),
                space: space.map(str::to_string),
                source_agent: Some("claude-code".to_string()),
                ..Default::default()
            }])
            .await
            .unwrap();
    }

    async fn memory_text(&self, id: &str) -> Option<String> {
        self.db
            .get_memory_detail(id)
            .await
            .unwrap()
            .map(|memory| memory.content)
    }

    /// Rows of the memory here, and whether they are forgotten.
    async fn memory_rows(&self, id: &str) -> (usize, bool) {
        let rows: Vec<_> = self
            .db
            .export_memory_rows()
            .await
            .unwrap()
            .into_iter()
            .filter(|row| row.source_id == id)
            .collect();
        let forgotten = rows
            .iter()
            .all(|row| row.supersede_mode.as_deref() == Some("evicted"));
        (rows.len(), !rows.is_empty() && forgotten)
    }
}

fn memory_files(folder: &Path) -> Vec<PathBuf> {
    folder_files(folder)
        .into_iter()
        .map(|(path, _)| path)
        .filter(|path| path.to_string_lossy().contains("/memories/"))
        .collect()
}

#[tokio::test]
async fn a_memory_reaches_the_other_computer_with_its_id_and_is_queued_to_be_found() {
    let chosen = tempfile::tempdir().unwrap();
    let (a, b) = library_pair(chosen.path()).await;
    let engine = library::engine_folder(chosen.path());
    a.save_memory(
        "mem_otters",
        "The harbour survey counted nine otters near the old pier.",
        Some(SPACE),
    )
    .await;

    let got = a.lib_sync(chosen.path(), None).await.unwrap();
    assert_eq!(got.memories_exported, 1);
    let files = memory_files(&engine);
    assert_eq!(files.len(), 1);
    assert!(files[0].to_string_lossy().ends_with(".wlv"));
    assert!(files[0]
        .to_string_lossy()
        .contains(&format!("{}/memories/mem_otters/", a.library_member_id())));

    let got = b
        .lib_sync(chosen.path(), Some(&a.library_member_id()))
        .await
        .unwrap();
    assert_eq!(got.memories_applied, 1);
    assert_eq!(got.memories_exported, 0, "taken, not written back");
    assert_eq!(
        b.memory_text("mem_otters").await.as_deref(),
        Some("The harbour survey counted nine otters near the old pier.")
    );
    assert_eq!(
        b.db.get_memory_detail("mem_otters")
            .await
            .unwrap()
            .unwrap()
            .space
            .as_deref(),
        Some(SPACE)
    );

    // Queued like a new capture: no vector yet, no enrichment receipt, so
    // the catch-up and the ambient classification lane take it.
    assert_eq!(b.db.count_null_memory_embeddings().await.unwrap(), 1);
    assert!(b
        .db
        .get_enrichment_steps("mem_otters")
        .await
        .unwrap()
        .is_empty());
    assert!(b
        .db
        .get_classification_candidate_for("mem_otters", crate::post_ingest::ENRICHMENT_MAX_ATTEMPTS)
        .await
        .unwrap()
        .is_some());
    // The catch-up embeds it, and B finds it.
    b.db.recover_null_memory_embeddings(&NoopEmitter)
        .await
        .unwrap();
    assert_eq!(b.db.count_null_memory_embeddings().await.unwrap(), 0);
    let found =
        b.db.search_memory(
            "otters near the pier",
            5,
            None,
            &ReadScope::Global,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(
        found.iter().any(|hit| hit.source_id == "mem_otters"),
        "B finds the memory A saved"
    );

    // Nothing more to do, and no churn from enrichment on either side.
    lib_settle(&a, &b, chosen.path()).await;
    assert_eq!(memory_files(&engine).len(), 1);
}

#[tokio::test]
async fn an_edit_on_the_other_computer_and_a_delete_reach_back() {
    let chosen = tempfile::tempdir().unwrap();
    let (a, b) = library_pair(chosen.path()).await;
    a.save_memory("mem_tea", "Mara drinks green tea in the morning.", None)
        .await;
    lib_settle(&a, &b, chosen.path()).await;
    assert!(b.memory_text("mem_tea").await.is_some());

    // B edits it (the same id, new text, as a replace).
    tick().await;
    b.save_memory("mem_tea", "Mara now drinks black tea in the morning.", None)
        .await;
    lib_settle(&a, &b, chosen.path()).await;
    assert_eq!(
        a.memory_text("mem_tea").await.as_deref(),
        Some("Mara now drinks black tea in the morning.")
    );
    assert!(!a.memory_rows("mem_tea").await.1);

    // A forgets it; B forgets it too (restorable), and it is not made again.
    tick().await;
    a.db.forget_memory("mem_tea", "person", "test")
        .await
        .unwrap();
    lib_settle(&a, &b, chosen.path()).await;
    assert_eq!(b.memory_rows("mem_tea").await, (1, true));
    // A restore on A brings it back on B.
    tick().await;
    assert!(a
        .db
        .restore_forgotten_memory("mem_tea", "person")
        .await
        .unwrap());
    lib_settle(&a, &b, chosen.path()).await;
    assert_eq!(b.memory_rows("mem_tea").await, (1, false));

    // A permanent delete on A is forgotten for the others too.
    tick().await;
    a.db.delete_by_source_id("memory", "mem_tea").await.unwrap();
    lib_settle(&a, &b, chosen.path()).await;
    assert_eq!(b.memory_rows("mem_tea").await, (1, true));
    assert_eq!(a.memory_rows("mem_tea").await.0, 0, "not made again on A");
}

#[tokio::test]
async fn a_rebuilt_database_takes_the_memories_by_content_and_writes_nothing() {
    let chosen = tempfile::tempdir().unwrap();
    let (a, mut b) = library_pair(chosen.path()).await;
    for n in 0..3 {
        a.save_memory(
            &format!("mem_{n}"),
            &format!("Memory number {n} about the lighthouse keeper."),
            None,
        )
        .await;
    }
    lib_settle(&a, &b, chosen.path()).await;
    let engine = library::engine_folder(chosen.path());
    let files = memory_files(&engine).len();
    assert_eq!(files, 3);

    // B's database is rebuilt from the same folder: it takes them again by
    // content, writes no version, and a database that had the memories
    // already (a restore) writes none either.
    b.db = Member::open(b.dir.path(), "rebuilt.db").await;
    let got = b
        .lib_sync(chosen.path(), Some(&a.library_member_id()))
        .await
        .unwrap();
    assert_eq!(got.memories_applied, 3);
    assert_eq!(got.memories_exported, 0);
    assert_eq!(memory_files(&engine).len(), files);
    lib_settle(&a, &b, chosen.path()).await;
    assert_eq!(memory_files(&engine).len(), files);

    // A rebuilt A that already has them (same ids, same text) is in step.
    let mut a = a;
    let held =
        a.db.get_app_metadata(&format!("shared_space.held_memories:{LIBRARY_KEY}"))
            .await
            .unwrap();
    assert!(held.is_some());
    a.db = Member::open(a.dir.path(), "rebuilt.db").await;
    for n in 0..3 {
        a.save_memory(
            &format!("mem_{n}"),
            &format!("Memory number {n} about the lighthouse keeper."),
            None,
        )
        .await;
    }
    let got = a.lib_sync(chosen.path(), None).await.unwrap();
    assert_eq!(got.memories_exported, 0, "{got:?}");
    assert_eq!(got.memories_applied, 0, "{got:?}");
    assert_eq!(memory_files(&engine).len(), files);
}

#[tokio::test]
async fn documents_originals_and_memories_of_a_shared_space_stay_out() {
    let chosen = tempfile::tempdir().unwrap();
    let wedding = tempfile::tempdir().unwrap();
    let a = Member::new("Lucian").await;
    a.share(&space(), wedding.path()).await.unwrap();
    a.save_memory(
        "mem_shared_space",
        "The caterer needs a head count by May.",
        Some(SPACE),
    )
    .await;
    a.save_memory(
        "mem_kept",
        "A plain memory about a garden gate.",
        Some("Private"),
    )
    .await;
    // A document read from a folder, a saved web page, an imported chat.
    for (source, id) in [
        ("local_files", "doc::notes.md"),
        ("webpage", "https://example.com/post"),
        ("memory", "import_claude_conv1"),
        ("file", "zotero::paper"),
    ] {
        a.db.upsert_documents(vec![crate::sources::RawDocument {
            source: source.to_string(),
            source_id: id.to_string(),
            title: "Source material".to_string(),
            content: "Long source material that is not a memory the person saved.".to_string(),
            last_modified: chrono::Utc::now().timestamp(),
            ..Default::default()
        }])
        .await
        .unwrap();
    }
    let got = a.lib_sync(chosen.path(), None).await.unwrap();
    assert_eq!(got.memories_exported, 1);
    let engine = library::engine_folder(chosen.path());
    let files = memory_files(&engine);
    assert_eq!(files.len(), 1);
    let text = walk(&engine);
    assert!(text.contains("garden gate"));
    assert!(
        !text.contains("caterer"),
        "a Space with its own share stays out"
    );
    assert!(
        !text.contains("source material"),
        "originals are not written"
    );

    // The Space share carries pages only: no memory folder in it.
    a.share(&space(), wedding.path()).await.unwrap();
    assert!(memory_files(wedding.path()).is_empty());
    assert!(!walk(wedding.path()).contains("caterer"));

    // And the other computer does not get the Space's memory.
    let b = Member::new("h1646").await;
    b.lib_sync(chosen.path(), Some(&a.library_member_id()))
        .await
        .unwrap();
    assert!(b.memory_text("mem_kept").await.is_some());
    assert!(b.memory_text("mem_shared_space").await.is_none());
    assert!(b.memory_text("import_claude_conv1").await.is_none());
}

#[tokio::test]
async fn a_pass_that_would_forget_most_memories_forgets_none() {
    let chosen = tempfile::tempdir().unwrap();
    let (a, b) = library_pair(chosen.path()).await;
    for n in 0..25 {
        a.save_memory(
            &format!("mem_{n}"),
            &format!("Note {n} about the weekly market stall."),
            None,
        )
        .await;
    }
    lib_settle(&a, &b, chosen.path()).await;
    for n in 0..25 {
        a.db.forget_memory(&format!("mem_{n}"), "person", "test")
            .await
            .unwrap();
    }
    a.lib_sync(chosen.path(), None).await.unwrap();
    let got = b
        .lib_sync(chosen.path(), Some(&a.library_member_id()))
        .await
        .unwrap();
    assert_eq!(got.memories_applied, 0);
    assert_eq!(got.memories_held_back, 25);
    assert_eq!(b.memory_rows("mem_0").await, (1, false));
}

#[tokio::test]
async fn a_second_pass_does_not_read_a_version_file_again() {
    let chosen = tempfile::tempdir().unwrap();
    let a = Member::new("Lucian").await;
    a.lib_sync(chosen.path(), None).await.unwrap();
    a.save_memory("mem_once", "A memory read once, however many passes.", None)
        .await;
    a.lib_sync(chosen.path(), None).await.unwrap();
    let engine = library::engine_folder(chosen.path());
    let first = memories::read_folder(&engine);
    assert_eq!(first.memories.len(), 1);
    // Break the file's bytes: the name no longer matches. A pass that read it
    // again would skip it; one that knows its sha does not look.
    let file = memory_files(&engine).remove(0);
    let version = first.memories["mem_once"][0].clone();
    std::fs::write(&file, b"changed").unwrap();
    let second = memories::read_folder(&engine);
    assert!(Arc::ptr_eq(&version, &second.memories["mem_once"][0]));
}

#[test]
fn an_automatic_pass_waits_for_typing_to_stop() {
    let row = |by: &str, at: i64| crate::db::PageHistoryEntry {
        version: 3,
        title: None,
        content: String::new(),
        source_memory_ids: vec![],
        edited_by: by.into(),
        created_at: at,
    };
    let base = Some(900); // last in step 100 s ago
                          // The editor saved 30 s ago: still typing.
    assert!(still_typing(
        Some(&row("manual_edit", 970)),
        base,
        1_000,
        120
    ));
    // Two minutes since the last save: written now.
    assert!(!still_typing(
        Some(&row("manual_edit", 880)),
        base,
        1_000,
        120
    ));
    // Typing on, but the last version is ten minutes old: written now.
    assert!(!still_typing(
        Some(&row("manual_edit", 990)),
        Some(400),
        1_000,
        120
    ));
    // A new page (no version yet) is written at once.
    assert!(!still_typing(
        Some(&row("manual_edit", 990)),
        None,
        1_000,
        120
    ));
    // An AI edit, or a sync the person asked for, is written at once.
    assert!(!still_typing(
        Some(&row("agent_refresh", 990)),
        base,
        1_000,
        120
    ));
    assert!(!still_typing(Some(&row("fs_edit", 990)), base, 1_000, 0));
    assert!(still_typing(Some(&row("fs_edit", 990)), base, 1_000, 120));
    assert!(!still_typing(None, base, 1_000, 120));
}
