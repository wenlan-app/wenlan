// SPDX-License-Identifier: Apache-2.0
//! `space_folders` against a real database and pages folder.

use super::MemoryDB;
use crate::space_folders::{arrange, turn_on, PassStats};
use std::collections::BTreeSet;
use std::path::Path;

// ---- With a real database and pages folder -------------------------------

async fn project(db: &MemoryDB, root: &Path, id: &str) {
    let page = db.get_page(id).await.unwrap().unwrap();
    crate::export::knowledge::KnowledgeProjectionWrite::new(root.to_path_buf(), db)
        .write_page_gated(db, &page)
        .await
        .unwrap();
}

fn md_files(root: &Path) -> BTreeSet<String> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeSet<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                walk(base, &path, out);
            } else if name.ends_with(".md") && !matches!(name.as_str(), "index.md" | "log.md") {
                out.insert(
                    path.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(root, root, &mut out);
    out
}

async fn page_space(db: &MemoryDB, id: &str) -> Option<String> {
    db.get_page(id)
        .await
        .unwrap()
        .unwrap()
        .space
        .filter(|space| space != super::UNFILED_SPACE_ID)
}

#[tokio::test]
async fn pages_follow_their_space_and_their_folder_on_disk() {
    let (db, _db_dir) = super::tests::test_db().await;
    let pages_dir = tempfile::tempdir().unwrap();
    let root = pages_dir.path();
    let settings = pages_dir.path().join("no-shared-spaces.json");
    db.conn
        .lock()
        .await
        .execute_batch(
            "INSERT INTO spaces (id,name,created_at,updated_at) VALUES
                 ('s-1','文瀾',1,1), ('s-2','個人',1,1);
             INSERT INTO pages
                 (id,title,summary,content,space,source_memory_ids,version,status,
                  created_at,last_compiled,last_modified,workspace,creation_kind,review_status)
             VALUES
                 ('p-a','定位','s','A body','文瀾','[]',1,'active','2026-01-01T00:00:00Z',
                  '2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','文瀾','authored','confirmed'),
                 ('p-b','計畫','s','B body','個人','[]',1,'active','2026-01-01T00:00:00Z',
                  '2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','個人','authored','confirmed'),
                 ('p-c','散頁','s','C body','00000000-0000-4000-8000-000000000001','[]',1,'active','2026-01-01T00:00:00Z',
                  '2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','00000000-0000-4000-8000-000000000001','authored','confirmed'),
                 ('p-e','舊空間頁','s','E body','old-space','[]',1,'active','2026-01-01T00:00:00Z',
                  '2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','old-space','authored','confirmed');",
        )
        .await
        .unwrap();
    for id in ["p-a", "p-b", "p-c", "p-e"] {
        project(&db, root, id).await;
    }
    // The person's own folder, made before: its page keeps it, inside its Space.
    crate::export::knowledge::create_knowledge_folder(root, "", "品牌").unwrap();
    let b_file = md_files(root)
        .into_iter()
        .find(|f| f.contains("計畫"))
        .unwrap();
    crate::export::knowledge::move_projected_page(root, "p-b", &b_file, "品牌", "t-1").unwrap();
    let before = md_files(root);

    // Off: nothing moves.
    let stats = arrange(&db, root, &settings, &[]).await.unwrap();
    assert_eq!(stats, PassStats::default());
    assert_eq!(md_files(root), before);

    // On: every page goes into its Space's folder.
    turn_on(root).unwrap();
    let stats = arrange(&db, root, &settings, &[]).await.unwrap();
    assert_eq!(stats.moved, 3, "{:?}", md_files(root));
    let files = md_files(root);
    // A Space only named on its pages, never registered, still gets a folder.
    assert!(files.contains("old-space/舊空間頁.md"), "{files:?}");
    assert!(files.contains("文瀾/定位.md"), "{files:?}");
    assert!(files.contains("個人/品牌/計畫.md"), "{files:?}");
    assert!(files.contains("散頁.md"), "{files:?}");
    // A second pass changes nothing.
    assert_eq!(
        arrange(&db, root, &settings, &[]).await.unwrap(),
        PassStats::default()
    );

    // A new page is written straight into its Space's folder.
    db.conn
        .lock()
        .await
        .execute_batch(
            "INSERT INTO pages
                 (id,title,summary,content,space,source_memory_ids,version,status,
                  created_at,last_compiled,last_modified,workspace,creation_kind,review_status)
             VALUES ('p-d','新頁','s','D body','個人','[]',1,'active','2026-01-01T00:00:00Z',
                  '2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','個人','authored','confirmed');",
        )
        .await
        .unwrap();
    project(&db, root, "p-d").await;
    assert!(
        md_files(root).contains("個人/新頁.md"),
        "{:?}",
        md_files(root)
    );

    // The Space is renamed: its folder's pages move to the new name.
    db.update_space("文瀾", "Wenlan", None).await.unwrap();
    let stats = arrange(&db, root, &settings, &[]).await.unwrap();
    assert_eq!(stats.moved, 1);
    assert!(
        md_files(root).contains("Wenlan/定位.md"),
        "{:?}",
        md_files(root)
    );
    assert!(
        !root.join("文瀾").exists(),
        "the emptied old folder is gone"
    );
    // The moved file says its new Space.
    let text = std::fs::read_to_string(root.join("Wenlan/定位.md")).unwrap();
    assert!(text.contains("Wenlan") && !text.contains("文瀾"), "{text}");

    // Moved to another Space in Wenlan: the file follows, with its folder.
    let page = db.get_page("p-b").await.unwrap().unwrap();
    db.set_page_space(
        "p-b",
        Some("Wenlan"),
        page.version,
        &crate::read_scope::ReadScope::Global,
    )
    .await
    .unwrap();
    arrange(&db, root, &settings, &[]).await.unwrap();
    assert!(
        md_files(root).contains("Wenlan/品牌/計畫.md"),
        "{:?}",
        md_files(root)
    );

    // Dragged in Finder into another Space's folder: the page moves Space.
    std::fs::rename(root.join("Wenlan/定位.md"), root.join("個人/定位.md")).unwrap();
    crate::export::knowledge::follow_external_moves(root).unwrap();
    let stats = arrange(&db, root, &settings, &[]).await.unwrap();
    assert_eq!(stats.respaced, 1, "{stats:?}");
    assert_eq!(page_space(&db, "p-a").await.as_deref(), Some("個人"));
    assert!(md_files(root).contains("個人/定位.md"));
    let text = std::fs::read_to_string(root.join("個人/定位.md")).unwrap();
    assert!(text.contains("個人"), "{text}");
    assert_eq!(
        arrange(&db, root, &settings, &[]).await.unwrap(),
        PassStats::default()
    );

    // Dragged out of every Space's folder: left there, keeps its Space.
    std::fs::rename(root.join("個人/定位.md"), root.join("定位.md")).unwrap();
    crate::export::knowledge::follow_external_moves(root).unwrap();
    assert_eq!(
        arrange(&db, root, &settings, &[]).await.unwrap(),
        PassStats::default()
    );
    assert_eq!(page_space(&db, "p-a").await.as_deref(), Some("個人"));
    assert!(md_files(root).contains("定位.md"));

    // The person renames a Space's folder in Finder: the Space is renamed.
    std::fs::rename(root.join("Wenlan"), root.join("工作")).unwrap();
    crate::export::knowledge::follow_external_moves(root).unwrap();
    let stats = arrange(&db, root, &settings, &[]).await.unwrap();
    assert_eq!(stats.renamed, 1, "{stats:?}");
    assert_eq!(page_space(&db, "p-b").await.as_deref(), Some("工作"));
    // Next pass: everything is where it belongs, nothing moves.
    let stats = arrange(&db, root, &settings, &[]).await.unwrap();
    assert_eq!(stats.moved, 0, "{stats:?}");
    assert!(
        md_files(root).contains("工作/品牌/計畫.md"),
        "{:?}",
        md_files(root)
    );
    assert!(!root.join("Wenlan").exists());

    // Off, then on again: nothing nests, nothing moves back.
    crate::space_folders::turn_off(root).unwrap();
    crate::space_folders::turn_on(root).unwrap();
    assert_eq!(
        arrange(&db, root, &settings, &[]).await.unwrap(),
        PassStats::default()
    );
}
