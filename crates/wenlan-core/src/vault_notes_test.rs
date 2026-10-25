// SPDX-License-Identifier: Apache-2.0
use super::*;
use wenlan_types::sources::{SourceType, SyncStatus};

fn vault(id: &str, root: &Path) -> Source {
    Source {
        id: id.into(),
        source_type: SourceType::Obsidian,
        path: root.to_path_buf(),
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

fn settings(pairs: &[(&str, AiChanges)]) -> BTreeMap<String, AiChanges> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn code(error: WenlanError) -> String {
    match error {
        WenlanError::Validation(code) | WenlanError::Conflict(code) => code,
        other => panic!("expected a code, got {other:?}"),
    }
}

#[test]
fn a_note_with_nothing_set_is_never() {
    let found = effective(None, "Work/plan.md");
    assert_eq!(
        (found.value, found.from),
        (AiChanges::Never, AiChangesFrom::Default)
    );
    let found = effective(Some(&settings(&[])), "Work/plan.md");
    assert_eq!(found.from, AiChangesFrom::Default);
}

#[test]
fn the_file_wins_then_the_nearest_folder_then_the_vault() {
    let set = settings(&[
        ("", AiChanges::AskFirst),
        ("Work/", AiChanges::Never),
        ("Work/Drafts/", AiChanges::AskFirst),
        ("Work/Drafts/final.md", AiChanges::Never),
    ]);
    let at = |path| {
        let found = effective(Some(&set), path);
        (found.value, found.from)
    };
    assert_eq!(at("Ideas.md"), (AiChanges::AskFirst, AiChangesFrom::Vault));
    assert_eq!(
        at("Work/plan.md"),
        (AiChanges::Never, AiChangesFrom::Folder)
    );
    assert_eq!(
        at("Work/Drafts/x.md"),
        (AiChanges::AskFirst, AiChangesFrom::Folder)
    );
    assert_eq!(
        at("Work/Drafts/final.md"),
        (AiChanges::Never, AiChangesFrom::File)
    );
    assert_eq!(
        at("Work/Drafts/"),
        (AiChanges::AskFirst, AiChangesFrom::Folder)
    );
    assert_eq!(at(""), (AiChanges::AskFirst, AiChangesFrom::Vault));
    // `Work/` is not a folder of `Workshop.md`.
    assert_eq!(
        at("Workshop.md"),
        (AiChanges::AskFirst, AiChangesFrom::Vault)
    );
}

#[test]
fn a_pdf_is_always_never() {
    let set = settings(&[
        ("", AiChanges::AskFirst),
        ("paper.PDF", AiChanges::AskFirst),
    ]);
    let found = effective(Some(&set), "paper.PDF");
    assert_eq!(
        (found.value, found.from),
        (AiChanges::Never, AiChangesFrom::Pdf)
    );
}

#[test]
fn keys_and_paths_stay_inside_the_vault() {
    assert_eq!(normalize_key("").unwrap(), "");
    assert_eq!(normalize_key("Work/").unwrap(), "Work/");
    assert_eq!(normalize_key("Work/plan.md").unwrap(), "Work/plan.md");
    for bad in [
        "/etc/x.md",
        "../x.md",
        "Work/../x.md",
        "a//b.md",
        "a\\b.md",
        "./x.md",
    ] {
        assert!(normalize_key(bad).is_err(), "{bad}");
    }
    let root = Path::new("/v/My notes");
    assert_eq!(
        vault_relative(root, Path::new("/v/My notes/Work/plan.md")).unwrap(),
        "Work/plan.md"
    );
    for bad in [
        "/v/other/x.md",
        "/v/My notes",
        "/v/My notes/../x.md",
        "x.md",
    ] {
        assert!(vault_relative(root, Path::new(bad)).is_err(), "{bad}");
    }
}

#[test]
fn only_the_persons_markdown_notes_take_proposals() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    std::fs::create_dir_all(root.join(".obsidian")).unwrap();
    std::fs::create_dir_all(root.join("Wenlan")).unwrap();
    std::fs::write(root.join("note.md"), "mine").unwrap();
    std::fs::write(root.join("paper.pdf"), "pdf").unwrap();
    std::fs::write(root.join(".obsidian/app.md"), "x").unwrap();
    std::fs::write(root.join("Wenlan/page.md"), "x").unwrap();
    let source = vault("vn-only", &root);
    let pages = root.join("Wenlan");

    assert!(note_file(&source, &pages, "note.md").is_ok());
    assert_eq!(
        code(note_file(&source, &pages, "paper.pdf").unwrap_err()),
        "note_not_markdown"
    );
    assert_eq!(
        code(note_file(&source, &pages, ".obsidian/app.md").unwrap_err()),
        "note_path_invalid"
    );
    assert_eq!(
        code(note_file(&source, &pages, "Wenlan/page.md").unwrap_err()),
        "note_is_wenlan_page"
    );
    assert!(matches!(
        note_file(&source, &pages, "missing.md").unwrap_err(),
        WenlanError::NotFound(_)
    ));
}

#[cfg(unix)]
#[test]
fn a_link_out_of_the_vault_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("secret.md"), "x").unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.join("linked")).unwrap();
    std::os::unix::fs::symlink(elsewhere.join("secret.md"), root.join("file.md")).unwrap();
    let source = vault("vn-link", &root);
    let pages = tmp.path().join("pages");
    assert_eq!(
        code(note_file(&source, &pages, "linked/secret.md").unwrap_err()),
        "note_outside_vault"
    );
    assert_eq!(
        code(note_file(&source, &pages, "file.md").unwrap_err()),
        "note_path_invalid"
    );
}

#[test]
fn a_never_note_takes_no_proposal_and_nothing_is_written() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("note.md"), "mine").unwrap();
    let source = vault("vn-never", &root);
    let pages = tmp.path().join("pages");

    let error = propose(
        &source,
        None,
        &pages,
        "note.md",
        "AI".into(),
        "why".into(),
        "claude",
    )
    .unwrap_err();
    assert_eq!(code(error), "note_ai_changes_never");
    assert_eq!(
        std::fs::read_to_string(root.join("note.md")).unwrap(),
        "mine"
    );
    assert!(list_proposals(Some("vn-never")).is_empty());
}

#[test]
fn an_accepted_proposal_is_written_and_a_declined_one_is_not() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    std::fs::create_dir_all(root.join("Work")).unwrap();
    std::fs::write(root.join("Work/plan.md"), "Launch in September.\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.join("Work/plan.md"),
            std::fs::Permissions::from_mode(0o640),
        )
        .unwrap();
    }
    let source = vault("vn-accept", &root);
    let pages = tmp.path().join("pages");
    let set = settings(&[("Work/", AiChanges::AskFirst)]);

    let proposal = propose(
        &source,
        Some(&set),
        &pages,
        "Work/plan.md",
        "Launch in October.\n".into(),
        " the call moved it ".into(),
        "claude",
    )
    .unwrap();
    assert_eq!(proposal.reason, "the call moved it");
    assert_eq!(
        std::fs::read_to_string(root.join("Work/plan.md")).unwrap(),
        "Launch in September.\n"
    );
    assert_eq!(list_proposals(Some("vn-accept")), vec![proposal.clone()]);
    assert_eq!(
        current_text(&source, &pages, "Work/plan.md").unwrap(),
        "Launch in September.\n"
    );

    // Proposing the same text again is not a change.
    let error = propose(
        &source,
        Some(&set),
        &pages,
        "Work/plan.md",
        "Launch in September.\n".into(),
        String::new(),
        "claude",
    )
    .unwrap_err();
    assert_eq!(code(error), "note_proposal_unchanged");

    let written = accept(&source, &pages, &proposal.id).unwrap();
    assert_eq!(written, root.join("Work/plan.md"));
    assert_eq!(
        std::fs::read_to_string(&written).unwrap(),
        "Launch in October.\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&written).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640, "the note keeps its permissions");
    }
    assert!(list_proposals(Some("vn-accept")).is_empty());
    let leftovers: Vec<_> = std::fs::read_dir(root.join("Work"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(
        leftovers.len(),
        1,
        "no temporary file is left: {leftovers:?}"
    );

    let again = propose(
        &source,
        Some(&set),
        &pages,
        "Work/plan.md",
        "Launch in November.\n".into(),
        String::new(),
        "claude",
    )
    .unwrap();
    assert!(remove_proposal(&again.id).unwrap());
    assert!(!remove_proposal(&again.id).unwrap());
    assert_eq!(
        std::fs::read_to_string(&written).unwrap(),
        "Launch in October.\n"
    );
}

#[test]
fn an_edit_made_after_the_proposal_is_never_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("note.md"), "v1").unwrap();
    let source = vault("vn-race", &root);
    let pages = tmp.path().join("pages");
    let set = settings(&[("", AiChanges::AskFirst)]);

    let proposal = propose(
        &source,
        Some(&set),
        &pages,
        "note.md",
        "AI v2".into(),
        String::new(),
        "claude",
    )
    .unwrap();
    std::fs::write(root.join("note.md"), "my v2").unwrap();
    assert_eq!(
        code(accept(&source, &pages, &proposal.id).unwrap_err()),
        "note_changed_since_proposal"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("note.md")).unwrap(),
        "my v2"
    );
    assert!(
        list_proposals(Some("vn-race")).is_empty(),
        "the stale proposal is dropped"
    );
}

#[test]
fn switching_a_note_to_never_drops_its_proposal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.md"), "a").unwrap();
    std::fs::write(root.join("b.md"), "b").unwrap();
    let source = vault("vn-drop", &root);
    let pages = tmp.path().join("pages");
    let mut set = settings(&[("", AiChanges::AskFirst)]);
    propose(
        &source,
        Some(&set),
        &pages,
        "a.md",
        "A".into(),
        String::new(),
        "claude",
    )
    .unwrap();
    propose(
        &source,
        Some(&set),
        &pages,
        "b.md",
        "B".into(),
        String::new(),
        "claude",
    )
    .unwrap();

    set.insert("a.md".into(), AiChanges::Never);
    assert_eq!(drop_disallowed("vn-drop", Some(&set)).unwrap(), 1);
    let left: Vec<_> = list_proposals(Some("vn-drop"))
        .into_iter()
        .map(|p| p.path)
        .collect();
    assert_eq!(left, vec!["b.md".to_string()]);
}

#[test]
fn a_bad_proposal_id_reads_nothing_outside_the_proposals() {
    for id in ["../config", "np_../../x", "np_0123456789abcdeg", ""] {
        assert!(
            matches!(load_proposal(id).unwrap_err(), WenlanError::NotFound(_)),
            "{id}"
        );
    }
}
