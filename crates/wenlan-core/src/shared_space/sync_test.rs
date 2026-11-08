// SPDX-License-Identifier: Apache-2.0
//! Two members, each with their own database, sharing one Space through one
//! folder (delivery criteria 1-5 of the shared Space plan).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use wenlan_types::requests::{CreateConceptRequest, UpdatePageRequest};
use wenlan_types::shared_space::SharedSyncCounts;
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
        share_settings_at(&self.settings(), SPACE)
            .unwrap()
            .unwrap()
            .member_id
    }

    async fn sync(&self, folder: &Path) -> SharedSyncCounts {
        sync_space(
            &self.db,
            SyncTarget {
                space: SPACE,
                folder,
                member_name: self.name,
                settings_path: &self.settings(),
                knowledge_path: None,
            },
        )
        .await
        .unwrap()
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

    async fn content(&self, id: &str) -> String {
        self.db.get_page(id).await.unwrap().unwrap().content
    }

    async fn history(&self, id: &str) -> Vec<crate::db::PageHistoryEntry> {
        self.db.list_page_history(id, 100).await.unwrap()
    }
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
    a.sync(&folder).await;
    b.sync(&folder).await;
    b.edit(&second, "The old mill, by the river.").await;
    b.sync(&folder).await;
    a.sync(&folder).await;
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
