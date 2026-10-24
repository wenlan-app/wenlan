// SPDX-License-Identifier: Apache-2.0
use super::*;
use wenlan_types::sources::SyncStatus;

fn source(source_type: SourceType, path: &Path) -> Source {
    Source {
        id: "s".into(),
        source_type,
        path: path.to_path_buf(),
        status: SyncStatus::Active,
        last_sync: None,
        file_count: 0,
        memory_count: 0,
        last_sync_errors: 0,
        last_sync_error_detail: None,
        space: None,
        queued_files: 0,
        waiting_files: 0,
    }
}

fn code(result: Result<PathBuf, WenlanError>) -> String {
    match result {
        Err(WenlanError::Validation(code)) => code,
        other => panic!("expected a validation code, got {other:?}"),
    }
}

#[test]
fn a_new_folder_inside_a_vault_is_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let current = tmp.path().join("pages");
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(vault.join(".obsidian")).unwrap();
    let target = vault.join("Wenlan");
    let sources = [source(SourceType::Obsidian, &vault)];
    assert_eq!(check_target(&current, &target, &sources).unwrap(), target);
}

#[test]
fn an_empty_folder_is_accepted_even_with_finder_litter() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("Wenlan");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join(".DS_Store"), b"x").unwrap();
    assert!(check_target(&tmp.path().join("pages"), &target, &[]).is_ok());
}

#[test]
fn refuses_what_would_lose_or_mix_files() {
    let tmp = tempfile::tempdir().unwrap();
    let current = tmp.path().join("pages");
    std::fs::create_dir(&current).unwrap();

    assert_eq!(
        code(check_target(&current, Path::new("relative/Wenlan"), &[])),
        "pages_folder_not_absolute"
    );
    assert_eq!(
        code(check_target(&current, &tmp.path().join("a/../pages"), &[])),
        "pages_folder_not_absolute"
    );
    assert_eq!(
        code(check_target(&current, &current, &[])),
        "pages_folder_unchanged"
    );
    assert_eq!(
        code(check_target(&current, &current.join("inner"), &[])),
        "pages_folder_nested"
    );
    assert_eq!(
        code(check_target(&current, tmp.path(), &[])),
        "pages_folder_nested"
    );

    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(vault.join(".obsidian")).unwrap();
    assert_eq!(
        code(check_target(&current, &vault.join(".obsidian/Wenlan"), &[])),
        "pages_folder_in_app_folder"
    );

    let full = tmp.path().join("full");
    std::fs::create_dir(&full).unwrap();
    std::fs::write(full.join("note.md"), b"mine").unwrap();
    assert_eq!(
        code(check_target(&current, &full, &[])),
        "pages_folder_not_empty"
    );

    let file = tmp.path().join("file.md");
    std::fs::write(&file, b"mine").unwrap();
    assert_eq!(
        code(check_target(&current, &file, &[])),
        "pages_folder_not_a_folder"
    );

    assert_eq!(
        code(check_target(
            &current,
            &tmp.path().join("missing/Wenlan"),
            &[]
        )),
        "pages_folder_parent_missing"
    );

    let bundle = tmp.path().join("bundle");
    std::fs::create_dir(&bundle).unwrap();
    let sources = [source(SourceType::Okf, &bundle)];
    assert_eq!(
        code(check_target(&current, &bundle.join("Wenlan"), &sources)),
        "pages_folder_overlaps_bundle"
    );
}

#[cfg(unix)]
#[test]
fn refuses_a_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(
        code(check_target(&tmp.path().join("pages"), &link, &[])),
        "pages_folder_symlink"
    );
}

#[test]
fn a_scan_leaves_out_the_pages_folder_only_when_it_is_inside_the_root() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    let pages = vault.join("Wenlan");
    std::fs::create_dir_all(pages.join("_sources")).unwrap();
    let mine = vault.join("note.md");
    let page = pages.join("page.md");
    let stub = pages.join("_sources/mem_1.md");
    for path in [&mine, &page, &stub] {
        std::fs::write(path, b"x").unwrap();
    }
    let scanned = vec![mine.clone(), page, stub];
    assert_eq!(
        without_pages_folder(&vault, &pages, scanned.clone()),
        vec![mine]
    );

    let elsewhere = tmp.path().join("pages");
    assert_eq!(
        without_pages_folder(&vault, &elsewhere, scanned.clone()),
        scanned
    );
}

fn fill(dir: &Path) {
    std::fs::create_dir_all(dir.join(".wenlan")).unwrap();
    std::fs::create_dir_all(dir.join("_sources")).unwrap();
    std::fs::write(dir.join("alpha.md"), b"alpha").unwrap();
    std::fs::write(dir.join(".wenlan/state.json"), b"{}").unwrap();
    std::fs::write(dir.join("_sources/mem_1.md"), b"stub").unwrap();
}

#[test]
fn moves_every_entry_and_keeps_the_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let from = tmp.path().join("pages");
    let to = tmp.path().join("vault/Wenlan");
    fill(&from);
    std::fs::create_dir(tmp.path().join("vault")).unwrap();

    assert_eq!(move_pages_folder(&from, &to).unwrap(), 3);
    assert_eq!(std::fs::read(to.join("alpha.md")).unwrap(), b"alpha");
    assert_eq!(std::fs::read(to.join(".wenlan/state.json")).unwrap(), b"{}");
    assert_eq!(
        std::fs::read(to.join("_sources/mem_1.md")).unwrap(),
        b"stub"
    );
    assert!(from.is_dir(), "the old folder stays, empty");
    assert_eq!(std::fs::read_dir(&from).unwrap().count(), 0);

    // Running again after it finished changes nothing.
    assert_eq!(move_pages_folder(&from, &to).unwrap(), 0);
    assert_eq!(std::fs::read(to.join("alpha.md")).unwrap(), b"alpha");
}

#[test]
fn an_interrupted_copy_is_finished_not_duplicated() {
    let tmp = tempfile::tempdir().unwrap();
    let from = tmp.path().join("pages");
    let to = tmp.path().join("Wenlan");
    fill(&from);
    // The last run copied `_sources` into place but stopped before removing
    // the original, and left a half copy of `alpha.md` under a private name.
    std::fs::create_dir_all(&to).unwrap();
    copy_tree(&from.join("_sources"), &to.join("_sources")).unwrap();
    std::fs::write(to.join(format!("{MOVING_PREFIX}alpha.md")), b"alp").unwrap();

    assert_eq!(move_pages_folder(&from, &to).unwrap(), 3);
    assert!(!from.join("_sources").exists());
    assert_eq!(
        std::fs::read(to.join("_sources/mem_1.md")).unwrap(),
        b"stub"
    );
    assert_eq!(std::fs::read(to.join("alpha.md")).unwrap(), b"alpha");
    assert!(!to.join(format!("{MOVING_PREFIX}alpha.md")).exists());
}

#[test]
fn a_different_file_already_at_the_new_name_stops_the_move() {
    let tmp = tempfile::tempdir().unwrap();
    let from = tmp.path().join("pages");
    let to = tmp.path().join("Wenlan");
    fill(&from);
    std::fs::create_dir(&to).unwrap();
    std::fs::write(to.join("alpha.md"), b"someone else's").unwrap();

    let error = move_pages_folder(&from, &to).unwrap_err();
    assert!(
        matches!(error, WenlanError::Conflict(ref m) if m.starts_with("pages_folder_move_conflict"))
    );
    assert_eq!(
        std::fs::read(to.join("alpha.md")).unwrap(),
        b"someone else's"
    );
    assert_eq!(std::fs::read(from.join("alpha.md")).unwrap(), b"alpha");
}

#[test]
fn the_cross_volume_copy_keeps_names_bytes_and_modes() {
    let tmp = tempfile::tempdir().unwrap();
    let from = tmp.path().join("pages");
    fill(&from);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            from.join("alpha.md"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let copy = tmp.path().join("copy");
    copy_tree(&from, &copy).unwrap();
    assert!(same_tree(&from, &copy).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(copy.join("alpha.md"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    std::fs::write(copy.join("_sources/mem_1.md"), b"changed").unwrap();
    assert!(!same_tree(&from, &copy).unwrap());
}

#[test]
fn a_missing_old_folder_moves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let to = tmp.path().join("Wenlan");
    assert_eq!(
        move_pages_folder(&tmp.path().join("never"), &to).unwrap(),
        0
    );
    assert!(to.is_dir());
}
