// SPDX-License-Identifier: Apache-2.0
use super::*;

fn page(id: &str, file: &str, space: Option<&str>) -> Placement {
    Placement {
        id: id.into(),
        file: file.into(),
        space: space.map(str::to_string),
    }
}

fn spaces(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

fn record(placed: &[(&str, &str)]) -> Record {
    Record {
        format: 1,
        placed: placed
            .iter()
            .map(|(id, folder)| (id.to_string(), folder.to_string()))
            .collect(),
    }
}

#[test]
fn folder_names_are_safe_on_every_system() {
    assert_eq!(folder_name("文瀾"), "文瀾");
    assert_eq!(folder_name("a/b:c"), "a-b-c");
    assert_eq!(folder_name(".hidden"), "hidden");
    assert_eq!(folder_name("trailing. "), "trailing");
    assert_eq!(folder_name("archive"), "archive (Space)");
    assert_eq!(folder_name("CON"), "CON (Space)");
    assert_eq!(folder_name("   "), "(Space)");
}

#[test]
fn team_spaces_sit_in_the_team_folder_apart_from_a_personal_space_of_its_name() {
    let teams = BTreeMap::from([("產品".to_string(), "Acme".to_string())]);
    let layout = layout(&spaces(&["文瀾", "產品"]), &teams);
    assert_eq!(layout["文瀾"], "文瀾");
    assert_eq!(layout["產品"], "Acme/產品");

    let layout = super::layout(&spaces(&["Acme", "產品"]), &teams);
    assert_eq!(layout["產品"], "Acme (team)/產品");
}

#[test]
fn first_pass_moves_pages_into_their_space_keeping_the_persons_folders() {
    let layout = layout(&spaces(&["文瀾", "個人"]), &BTreeMap::new());
    let pages = [
        page("a", "定位.md", Some("文瀾")),
        page("b", "呂奇軒/個人品牌/計畫.md", Some("個人")),
        page("c", "沒有空間.md", None),
        page("d", "文瀾/已經在.md", Some("文瀾")),
        page("e", "archive/舊.md", Some("文瀾")),
        page("f", "_sources/來源.md", Some("文瀾")),
    ];
    let steps = plan(&pages, &layout, &Record::default(), &[]);
    assert_eq!(
        steps,
        vec![
            Step::Move {
                id: "a".into(),
                from: "定位.md".into(),
                folder: "文瀾".into()
            },
            Step::Move {
                id: "b".into(),
                from: "呂奇軒/個人品牌/計畫.md".into(),
                folder: "個人/呂奇軒/個人品牌".into()
            },
            Step::Keep {
                id: "c".into(),
                folder: "".into()
            },
            Step::Keep {
                id: "d".into(),
                folder: "文瀾".into()
            },
        ]
    );
}

#[test]
fn a_page_moved_to_another_space_in_wenlan_moves_its_file() {
    let layout = layout(&spaces(&["文瀾", "個人"]), &BTreeMap::new());
    let pages = [page("a", "文瀾/發布/清單.md", Some("個人"))];
    let steps = plan(&pages, &layout, &record(&[("a", "文瀾")]), &[]);
    assert_eq!(
        steps,
        vec![Step::Move {
            id: "a".into(),
            from: "文瀾/發布/清單.md".into(),
            folder: "個人/發布".into()
        }]
    );
}

#[test]
fn a_renamed_space_moves_its_pages_to_the_new_folder() {
    let layout = layout(&spaces(&["新名"]), &BTreeMap::new());
    let pages = [page("a", "舊名/x.md", Some("新名"))];
    let steps = plan(&pages, &layout, &record(&[("a", "舊名")]), &[]);
    assert_eq!(
        steps,
        vec![Step::Move {
            id: "a".into(),
            from: "舊名/x.md".into(),
            folder: "新名".into()
        }]
    );
}

#[test]
fn a_file_the_person_moved_into_another_space_folder_moves_the_page() {
    let layout = layout(&spaces(&["文瀾", "個人"]), &BTreeMap::new());
    let pages = [
        page("a", "個人/x.md", Some("文瀾")),
        page("b", "y.md", Some("文瀾")),
        page("c", "文瀾/z.md", None),
    ];
    let steps = plan(
        &pages,
        &layout,
        &record(&[("a", "文瀾"), ("b", "文瀾"), ("c", "")]),
        &[],
    );
    assert_eq!(
        steps,
        vec![
            Step::SetSpace {
                id: "a".into(),
                space: Some("個人".into())
            },
            Step::SetSpace {
                id: "b".into(),
                space: None
            },
            Step::SetSpace {
                id: "c".into(),
                space: Some("文瀾".into())
            },
        ]
    );
}

#[test]
fn a_page_left_where_it_was_put_is_kept() {
    let layout = layout(&spaces(&["文瀾"]), &BTreeMap::new());
    let pages = [
        page("a", "文瀾/發布/x.md", Some("文瀾")),
        page("b", "my/y.md", None),
    ];
    let steps = plan(&pages, &layout, &record(&[("a", "文瀾"), ("b", "")]), &[]);
    assert_eq!(
        steps,
        vec![
            Step::Keep {
                id: "a".into(),
                folder: "文瀾".into()
            },
            Step::Keep {
                id: "b".into(),
                folder: "".into()
            },
        ]
    );
}

#[test]
fn joining_a_team_moves_the_space_under_the_team_folder() {
    let teams = BTreeMap::from([("產品".to_string(), "Acme".to_string())]);
    let layout = layout(&spaces(&["產品"]), &teams);
    let pages = [page("a", "產品/x.md", Some("產品"))];
    let steps = plan(&pages, &layout, &record(&[("a", "產品")]), &[]);
    assert_eq!(
        steps,
        vec![Step::Move {
            id: "a".into(),
            from: "產品/x.md".into(),
            folder: "Acme/產品".into()
        }]
    );
}

#[test]
fn linked_vault_folders_are_left_alone() {
    let layout = layout(&spaces(&["文瀾"]), &BTreeMap::new());
    let pages = [page("a", "My Vault/x.md", Some("文瀾"))];
    assert!(plan(&pages, &layout, &Record::default(), &["My Vault".into()]).is_empty());
}

#[test]
fn record_file_is_the_switch() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!is_on(dir.path()));
    turn_on(dir.path()).unwrap();
    assert!(is_on(dir.path()));
    assert_eq!(read_record(dir.path()).unwrap().unwrap().placed.len(), 0);
}
