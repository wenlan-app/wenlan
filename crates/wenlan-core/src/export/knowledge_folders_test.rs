use super::*;

fn seeded() -> (tempfile::TempDir, KnowledgeWriter, Page, String) {
    let dir = tempfile::tempdir().unwrap();
    let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
    let page = super::super::tests::test_concept();
    writer.write_page_for_test(&page).unwrap();
    let source = writer.page_filename(&page.id).unwrap();
    (dir, writer, page, source)
}

#[test]
fn physical_nested_folders_are_idempotent_and_exclude_controls() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        create_knowledge_folder(dir.path(), "", "Research").unwrap(),
        "Research"
    );
    assert_eq!(
        create_knowledge_folder(dir.path(), "Research", "Rust").unwrap(),
        "Research/Rust"
    );
    create_knowledge_folder(dir.path(), "Research", "Rust").unwrap();
    std::fs::create_dir(dir.path().join("archive")).unwrap();
    std::fs::create_dir(dir.path().join("_sources")).unwrap();
    assert_eq!(
        list_knowledge_folders(dir.path())
            .unwrap()
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["Research", "Research/Rust"]
    );
    assert!(create_knowledge_folder(dir.path(), "", "research").is_err());
}

#[test]
fn portable_paths_fail_closed() {
    for invalid in [
        "/root",
        "../root",
        "a//b",
        "a/./b",
        "a/../b",
        "C:root",
        "a\\b",
        "a/",
        ".wenlan",
        "a/archive",
        "_sources",
        "CON",
        "aux.txt",
        "LPT1",
        "a.",
        "a ",
        "a\0b",
        "a*b",
    ] {
        assert!(
            validate_knowledge_folder_path(invalid).is_err(),
            "accepted {invalid:?}"
        );
    }
    assert_eq!(
        validate_knowledge_folder_path("研究/Rust").unwrap(),
        "研究/Rust"
    );
    assert_eq!(validate_knowledge_folder_path("").unwrap(), "");
}

#[test]
fn first_placement_and_future_saves_keep_the_recorded_folder() {
    let dir = tempfile::tempdir().unwrap();
    let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
    create_knowledge_folder(dir.path(), "", "Notes").unwrap();
    create_knowledge_folder(dir.path(), "Notes", "Rust").unwrap();
    let mut page = super::super::tests::test_concept();
    let guard = writer.begin_test_write();
    writer
        .write_page_in_folder(&guard, &page, Some("Notes/Rust"))
        .unwrap();
    let path = writer.page_filename(&page.id).unwrap();
    assert_eq!(path, "Notes/Rust/rust-ownership.md");
    page.version += 1;
    page.content.push_str("\nUpdated");
    writer
        .write_page_in_folder(&guard, &page, Some(""))
        .unwrap();
    assert_eq!(
        writer.page_filename(&page.id).as_deref(),
        Some(path.as_str())
    );
    assert!(std::fs::read_to_string(dir.path().join(&path))
        .unwrap()
        .contains("Updated"));
    assert!(!dir.path().join("rust-ownership.md").exists());
    assert_eq!(writer.live_page_filenames(&[&page.id])[&page.id], path);
    assert!(std::fs::read_to_string(dir.path().join("index.md"))
        .unwrap()
        .contains("/Notes/Rust/rust-ownership.md"));
}

#[cfg(unix)]
#[test]
fn legacy_local_paths_survive_save_discovery_and_unrelated_writes() {
    for legacy in ["con.md", ".md", "Project: notes/con.md"] {
        let (dir, writer, mut page, source) = seeded();
        std::fs::create_dir_all(dir.path().join("Project: notes")).unwrap();
        std::fs::write(
            dir.path().join("Project: notes/local.md"),
            b"local user prose",
        )
        .unwrap();
        std::fs::rename(dir.path().join(&source), dir.path().join(legacy)).unwrap();
        let mut state = writer.load_state();
        state.pages.get_mut(&page.id).unwrap().file = legacy.into();
        writer.save_state(&state).unwrap();
        let before = std::fs::read(dir.path().join(legacy)).unwrap();
        writer.write_page_for_test(&page).unwrap();
        assert_eq!(std::fs::read(dir.path().join(legacy)).unwrap(), before);
        assert_eq!(writer.live_page_filenames(&[&page.id])[&page.id], legacy);
        assert!(knowledge_markdown_paths(dir.path())
            .unwrap()
            .contains(&legacy.into()));
        assert!(list_knowledge_folders(dir.path())
            .unwrap()
            .iter()
            .any(|f| f.path == "Project: notes"));
        let mut unrelated = page.clone();
        unrelated.id = "page_unrelated".into();
        unrelated.title = "Unrelated note".into();
        writer.write_page_for_test(&unrelated).unwrap();
        create_knowledge_folder(dir.path(), "", "New").unwrap();
        page.version += 1;
        page.content.push_str("\nLegacy location updated");
        writer.write_page_for_test(&page).unwrap();
        assert_eq!(writer.page_filename(&page.id).as_deref(), Some(legacy));
        assert!(std::fs::read_to_string(dir.path().join(legacy))
            .unwrap()
            .contains("Legacy location updated"));
        assert_eq!(
            std::fs::read(dir.path().join("Project: notes/local.md")).unwrap(),
            b"local user prose"
        );
        assert!(create_knowledge_folder(dir.path(), "", "Project: another").is_err());
    }
}

#[cfg(unix)]
#[test]
fn hidden_and_nonportable_origin_copies_are_found_and_evicted() {
    let (dir, writer, page, source) = seeded();
    std::fs::create_dir(dir.path().join("Project: notes")).unwrap();
    for copy in [".copy.md", "Project: notes/con.md"] {
        std::fs::copy(dir.path().join(&source), dir.path().join(copy)).unwrap();
    }
    let bytes = std::fs::read(dir.path().join(&source)).unwrap();
    assert_eq!(writer.projected_files().unwrap()[&page.id].len(), 3);
    assert!(writer.write_page_for_test(&page).is_err());
    let guard = writer.begin_test_write();
    assert_eq!(
        writer
            .evict_projected_pages(&guard, std::slice::from_ref(&page.id))
            .unwrap(),
        1
    );
    for removed in [source.as_str(), ".copy.md", "Project: notes/con.md"] {
        assert!(!dir.path().join(removed).exists());
    }
    for archived in [source.as_str(), ".copy.md", "con.md"] {
        assert_eq!(
            std::fs::read(dir.path().join("archive").join(archived)).unwrap(),
            bytes
        );
    }
}

#[test]
fn encoded_folder_index_links_stay_owned_and_update_after_move_and_save() {
    for (folder, encoded) in [
        ("Work (2026)", "Work%20%282026%29"),
        ("#", "%23"),
        ("%", "%25"),
    ] {
        let (dir, writer, mut page, source) = seeded();
        create_knowledge_folder(dir.path(), "", folder).unwrap();
        let moved =
            move_projected_page(dir.path(), &page.id, &source, folder, "index-encoding").unwrap();
        assert_eq!(moved, format!("{folder}/{source}"));
        let index = std::fs::read_to_string(dir.path().join("index.md")).unwrap();
        assert!(index.contains(&format!("(/{encoded}/{source})")), "{index}");
        assert!(index_body_is_generated(
            index.split_once("---\n\n").unwrap().1
        ));
        let target = format!("/{encoded}/{source}");
        let mut decoded = Vec::new();
        let mut bytes = target.as_bytes().iter().copied();
        while let Some(byte) = bytes.next() {
            decoded.push(if byte == b'%' {
                let pair = [bytes.next().unwrap(), bytes.next().unwrap()];
                u8::from_str_radix(std::str::from_utf8(&pair).unwrap(), 16).unwrap()
            } else {
                byte
            });
        }
        assert_eq!(String::from_utf8(decoded).unwrap(), format!("/{moved}"));
        page.title = "Updated index title".into();
        page.version += 1;
        writer.write_page_for_test(&page).unwrap();
        let index = std::fs::read_to_string(dir.path().join("index.md")).unwrap();
        assert!(
            index.contains(&format!("[Updated index title](/{encoded}/{source})")),
            "{index}"
        );
        assert_eq!(
            writer.page_filename(&page.id).as_deref(),
            Some(moved.as_str())
        );
        let mut other = page.clone();
        other.id = "page_another".into();
        other.title = "Another title".into();
        writer
            .write_page_in_folder(&writer.begin_test_write(), &other, Some(folder))
            .unwrap();
        let index = std::fs::read_to_string(dir.path().join("index.md")).unwrap();
        assert!(
            index.contains(&format!("[Another title](/{encoded}/another-title.md)")),
            "{index}"
        );
    }
}

#[test]
fn title_slugs_allocate_visible_portable_filenames() {
    for (title, expected) in [
        ("📒", "note.md"),
        ("CON", "con-page.md"),
        ("COM¹", "com¹-page.md"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let writer = KnowledgeWriter::new_for_test(dir.path().to_path_buf());
        let mut page = super::super::tests::test_concept();
        page.title = title.into();
        writer.write_page_for_test(&page).unwrap();
        assert_eq!(writer.page_filename(&page.id).as_deref(), Some(expected));
    }
}

#[test]
fn moving_preserves_external_prose_and_identity_without_re_rendering() {
    let (dir, writer, page, source) = seeded();
    create_knowledge_folder(dir.path(), "", "Notes").unwrap();
    let raw = std::fs::read_to_string(dir.path().join(&source))
        .unwrap()
        .replace("Rust uses ownership", "My external prose uses ownership");
    std::fs::write(dir.path().join(&source), &raw).unwrap();
    let target = move_projected_page(dir.path(), &page.id, &source, "Notes", "move-1").unwrap();
    assert_eq!(target, format!("Notes/{source}"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join(&target)).unwrap(),
        raw
    );
    assert!(!dir.path().join(&source).exists());
    assert_eq!(writer.page_filename(&page.id).unwrap(), target);
    assert!(
        writer.write_page_for_test(&page).is_err(),
        "external prose must stay protected until watcher adoption"
    );
    assert_eq!(
        move_projected_page(dir.path(), &page.id, &source, "Notes", "move-1").unwrap(),
        target
    );
}

#[test]
fn nested_reconcile_removal_and_truth_eviction_cover_every_copy() {
    let (dir, writer, mut page, source) = seeded();
    create_knowledge_folder(dir.path(), "", "Notes").unwrap();
    let target =
        move_projected_page(dir.path(), &page.id, &source, "Notes", "nested-lifecycle").unwrap();
    page.version += 1;
    page.content.push_str("\nNewer canonical content");
    let stats = writer.reconcile_for_test(&[page.clone()]).unwrap();
    assert_eq!(stats.rewritten, 1);
    assert!(std::fs::read_to_string(dir.path().join(&target))
        .unwrap()
        .contains("Newer canonical content"));
    std::fs::copy(
        dir.path().join(&target),
        dir.path().join("Notes/conflict-copy.md"),
    )
    .unwrap();
    assert_eq!(writer.projected_files().unwrap()[&page.id].len(), 2);
    let before = std::fs::read(dir.path().join(&target)).unwrap();
    let guard = writer.begin_test_write();
    assert_eq!(
        writer
            .evict_projected_pages(&guard, &[page.id.clone()])
            .unwrap(),
        1
    );
    assert!(!dir.path().join(&target).exists());
    assert!(!dir.path().join("Notes/conflict-copy.md").exists());
    assert!(!writer.load_state().pages.contains_key(&page.id));
    assert_eq!(
        std::fs::read(dir.path().join("archive").join(&source)).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(dir.path().join("archive/conflict-copy.md")).unwrap(),
        before
    );
    assert!(knowledge_markdown_paths(dir.path())
        .unwrap()
        .iter()
        .all(|path| !path.starts_with("archive/")));

    writer
        .write_page_in_folder(&guard, &page, Some("Notes"))
        .unwrap();
    writer.remove_page(&guard, &page.id).unwrap();
    assert!(!dir.path().join(&target).exists());
}

#[test]
fn move_destination_collision_keeps_both_files() {
    let (dir, _writer, page, source) = seeded();
    create_knowledge_folder(dir.path(), "", "Notes").unwrap();
    let before = std::fs::read(dir.path().join(&source)).unwrap();
    std::fs::write(
        dir.path().join("Notes").join(&source),
        b"existing user note",
    )
    .unwrap();
    assert!(move_projected_page(dir.path(), &page.id, &source, "Notes", "collision").is_err());
    assert_eq!(std::fs::read(dir.path().join(&source)).unwrap(), before);
    assert_eq!(
        std::fs::read(dir.path().join("Notes").join(&source)).unwrap(),
        b"existing user note"
    );
    assert!(!dir.path().join(".wenlan").join(MOVE_JOURNAL).exists());
}

#[test]
fn every_durable_move_edge_recovers_before_an_ordinary_write() {
    for edge in ["journal", "stage", "installed", "state"] {
        let (dir, writer, page, source) = seeded();
        create_knowledge_folder(dir.path(), "", "Notes").unwrap();
        let mut interrupt = |checkpoint: &str| {
            if checkpoint == edge {
                Err(WenlanError::Conflict("simulated_interrupt".into()))
            } else {
                Ok(())
            }
        };
        assert!(move_projected_page_with_checkpoint(
            dir.path(),
            &page.id,
            &source,
            "Notes",
            edge,
            &mut interrupt
        )
        .is_err());
        writer.write_page_for_test(&page).unwrap();
        let expected = if edge == "journal" {
            source.clone()
        } else {
            format!("Notes/{source}")
        };
        assert_eq!(writer.page_filename(&page.id).unwrap(), expected);
        assert_eq!(
            knowledge_markdown_paths(dir.path())
                .unwrap()
                .into_iter()
                .filter(|path| path.ends_with(&source))
                .collect::<Vec<_>>(),
            [expected]
        );
        assert!(!dir.path().join(".wenlan").join(MOVE_JOURNAL).exists());
    }
}

#[test]
fn raced_destination_or_source_never_gets_overwritten_or_deleted() {
    for edge in ["stage", "installed"] {
        let (dir, writer, page, source) = seeded();
        create_knowledge_folder(dir.path(), "", "Notes").unwrap();
        let before = std::fs::read(dir.path().join(&source)).unwrap();
        let target = format!("Notes/{source}");
        let raced = if edge == "stage" { &target } else { &source };
        let mut race = |checkpoint: &str| {
            if checkpoint == edge {
                std::fs::write(dir.path().join(raced), b"external racing save")?;
            }
            Ok(())
        };
        assert!(move_projected_page_with_checkpoint(
            dir.path(),
            &page.id,
            &source,
            "Notes",
            edge,
            &mut race
        )
        .is_err());
        assert_eq!(
            std::fs::read(dir.path().join(raced)).unwrap(),
            b"external racing save"
        );
        let journal: MoveJournal = serde_json::from_slice(
            &std::fs::read(dir.path().join(".wenlan").join(MOVE_JOURNAL)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(dir.path().join(".wenlan").join(&journal.stage)).unwrap(),
            before
        );
        assert!(writer.write_page_for_test(&page).is_err());
        assert_eq!(
            std::fs::read(dir.path().join(raced)).unwrap(),
            b"external racing save"
        );
    }
}

#[test]
fn duplicate_origin_external_move_and_corrupt_state_block_mutation() {
    let (dir, writer, page, source) = seeded();
    create_knowledge_folder(dir.path(), "", "Notes").unwrap();
    std::fs::copy(dir.path().join(&source), dir.path().join("Notes/copy.md")).unwrap();
    assert!(writer.write_page_for_test(&page).is_err());
    assert!(move_projected_page(dir.path(), &page.id, &source, "Notes", "duplicate").is_err());
    std::fs::remove_file(dir.path().join(&source)).unwrap();
    assert!(
        writer.write_page_for_test(&page).is_err(),
        "an external move must not recreate the old path"
    );
    std::fs::write(dir.path().join(".wenlan/state.json"), b"{corrupt").unwrap();
    assert!(create_knowledge_folder(dir.path(), "", "New").is_err());
    assert!(writer.write_page_for_test(&page).is_err());
    assert_eq!(
        std::fs::read(dir.path().join(".wenlan/state.json")).unwrap(),
        b"{corrupt"
    );
}

#[test]
fn duplicate_state_members_are_rejected_without_rewriting_state() {
    let (dir, writer, page, _source) = seeded();
    let entry = serde_json::to_string(&writer.load_state().pages[&page.id]).unwrap();
    let raw = format!(
        r#"{{"schema_version":2,"pages":{{"{}":{},"{}":{}}}}}"#,
        page.id, entry, page.id, entry
    );
    std::fs::write(dir.path().join(".wenlan/state.json"), &raw).unwrap();
    assert!(writer.write_page_for_test(&page).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".wenlan/state.json")).unwrap(),
        raw
    );
}

#[test]
fn an_editor_writing_the_open_moved_inode_leaves_its_latest_bytes_recoverable() {
    let (dir, writer, page, source) = seeded();
    create_knowledge_folder(dir.path(), "", "Notes").unwrap();
    let mut race = |checkpoint: &str| {
        if checkpoint == "stage" {
            let journal: MoveJournal = serde_json::from_slice(&std::fs::read(
                dir.path().join(".wenlan").join(MOVE_JOURNAL),
            )?)
            .unwrap();
            std::fs::write(
                dir.path().join(".wenlan").join(journal.stage),
                b"latest external editor bytes",
            )?;
        }
        Ok(())
    };
    assert!(move_projected_page_with_checkpoint(
        dir.path(),
        &page.id,
        &source,
        "Notes",
        "open-editor",
        &mut race
    )
    .is_err());
    let journal: MoveJournal = serde_json::from_slice(
        &std::fs::read(dir.path().join(".wenlan").join(MOVE_JOURNAL)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        std::fs::read(dir.path().join(".wenlan").join(journal.stage)).unwrap(),
        b"latest external editor bytes"
    );
    assert!(writer.write_page_for_test(&page).is_err());
    assert!(!dir.path().join("Notes").join(&source).exists());
}

#[cfg(unix)]
#[test]
fn nofollow_nested_symlink_never_reads_or_writes_outside_root() {
    let (dir, _writer, page, source) = seeded();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("outside.md"), b"private outside data").unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("Linked")).unwrap();
    assert!(list_knowledge_folders(dir.path()).unwrap().is_empty());
    assert!(create_knowledge_folder(dir.path(), "Linked", "New").is_err());
    assert!(move_projected_page(dir.path(), &page.id, &source, "Linked", "symlink").is_err());
    assert!(read_knowledge_markdown(dir.path(), "Linked/outside.md").is_err());
    assert_eq!(
        std::fs::read(outside.path().join("outside.md")).unwrap(),
        b"private outside data"
    );
}
