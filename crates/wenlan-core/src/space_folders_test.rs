// SPDX-License-Identifier: Apache-2.0
use super::*;

fn page(id: &str, file: &str, space: Option<&str>) -> Placement {
    Placement {
        id: id.into(),
        file: file.into(),
        space: space.map(str::to_string),
    }
}

fn input(names: &[&str]) -> LayoutInput {
    LayoutInput {
        spaces: names.iter().map(|n| n.to_string()).collect(),
        ..Default::default()
    }
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

fn mv(id: &str, from: &str, folder: &str) -> Step {
    Step::Move {
        id: id.into(),
        from: from.into(),
        folder: folder.into(),
    }
}

fn keep(id: &str, folder: &str) -> Step {
    Step::Keep {
        id: id.into(),
        folder: folder.into(),
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
    assert_eq!(folder_name("COM¹"), "COM¹ (Space)");
    assert_eq!(folder_name("   "), "(Space)");
}

#[test]
fn team_spaces_sit_in_the_team_folder_apart_from_a_personal_space_of_its_name() {
    let mut layout_input = input(&["文瀾", "產品"]);
    layout_input.teams = BTreeMap::from([("產品".to_string(), Some("Acme".to_string()))]);
    let layout = layout(&layout_input);
    assert_eq!(layout["文瀾"], "文瀾");
    assert_eq!(layout["產品"], "Acme/產品");

    let mut layout_input = input(&["Acme", "產品"]);
    layout_input.teams = BTreeMap::from([("產品".to_string(), Some("Acme".to_string()))]);
    assert_eq!(super::layout(&layout_input)["產品"], "Acme 2/產品");
}

#[test]
fn a_team_with_no_name_yet_is_left_out() {
    let mut layout_input = input(&["產品"]);
    layout_input.teams = BTreeMap::from([("產品".to_string(), None)]);
    let layout = layout(&layout_input);
    assert!(!layout.contains_key("產品"));
    let pages = [page("a", "產品/x.md", Some("產品"))];
    assert!(plan(
        &pages,
        &layout,
        &BTreeSet::new(),
        &record(&[("a", "產品")]),
        &[]
    )
    .is_empty());
}

#[test]
fn no_two_spaces_share_a_folder() {
    let layout = layout(&input(&["A/B", "A-B", "Work", "work"]));
    let folders: BTreeSet<String> = layout.values().map(|f| key(f)).collect();
    assert_eq!(folders.len(), 4, "{layout:?}");
}

#[test]
fn an_existing_folder_keeps_its_spelling_and_vault_folders_are_never_taken() {
    let mut layout_input = input(&["Wenlan", "Notes"]);
    layout_input.top_folders = vec!["wenlan".into(), "Notes".into()];
    layout_input.skip = vec!["Notes".into()];
    let layout = layout(&layout_input);
    assert_eq!(layout["Wenlan"], "wenlan");
    assert_eq!(layout["Notes"], "Notes 2");
}

#[test]
fn first_pass_moves_pages_into_their_space_keeping_the_persons_folders() {
    let layout = layout(&input(&["文瀾", "個人"]));
    let pages = [
        page("a", "定位.md", Some("文瀾")),
        page("b", "呂奇軒/個人品牌/計畫.md", Some("個人")),
        page("c", "沒有空間.md", None),
        page("d", "文瀾/已經在.md", Some("文瀾")),
        page("e", "archive/舊.md", Some("文瀾")),
        page("f", "_sources/來源.md", Some("文瀾")),
        page("g", "個人/x/錯的.md", Some("文瀾")),
        page("h", "個人/無.md", None),
    ];
    let steps = plan(&pages, &layout, &BTreeSet::new(), &Record::default(), &[]);
    assert_eq!(
        steps,
        vec![
            mv("a", "定位.md", "文瀾"),
            mv("b", "呂奇軒/個人品牌/計畫.md", "個人/呂奇軒/個人品牌"),
            keep("c", ""),
            keep("d", "文瀾"),
            // In another Space's folder: only the folders inside it come along.
            mv("g", "個人/x/錯的.md", "文瀾/x"),
        ]
    );
}

#[test]
fn a_page_moved_to_another_space_in_wenlan_moves_its_file() {
    let layout = layout(&input(&["文瀾", "個人"]));
    let pages = [page("a", "文瀾/發布/清單.md", Some("個人"))];
    let steps = plan(
        &pages,
        &layout,
        &BTreeSet::new(),
        &record(&[("a", "文瀾")]),
        &[],
    );
    assert_eq!(steps, vec![mv("a", "文瀾/發布/清單.md", "個人/發布")]);
}

#[test]
fn a_page_given_a_space_moves_out_of_the_top() {
    let layout = layout(&input(&["文瀾"]));
    let pages = [page("a", "my/x.md", Some("文瀾"))];
    let steps = plan(
        &pages,
        &layout,
        &BTreeSet::new(),
        &record(&[("a", "")]),
        &[],
    );
    assert_eq!(steps, vec![mv("a", "my/x.md", "文瀾/my")]);
}

#[test]
fn a_renamed_space_moves_its_pages_to_the_new_folder() {
    let layout = layout(&input(&["新名"]));
    let pages = [page("a", "舊名/x.md", Some("新名"))];
    let steps = plan(
        &pages,
        &layout,
        &BTreeSet::new(),
        &record(&[("a", "舊名")]),
        &[],
    );
    assert_eq!(steps, vec![mv("a", "舊名/x.md", "新名")]);
}

#[test]
fn a_file_dragged_into_another_personal_space_folder_moves_the_page() {
    let layout = layout(&input(&["文瀾", "個人"]));
    let pages = [
        page("a", "個人/sub/x.md", Some("文瀾")),
        page("b", "y.md", Some("文瀾")),
        page("c", "其他/z.md", Some("文瀾")),
        page("d", "文瀾/w.md", None),
        page("e", "個人/v.md", Some("個人")),
    ];
    let placed = record(&[
        ("a", "文瀾"),
        ("b", "文瀾"),
        ("c", "文瀾"),
        ("d", ""),
        ("e", ""),
    ]);
    assert_eq!(
        plan(&pages, &layout, &BTreeSet::new(), &placed, &[]),
        vec![
            Step::SetSpace {
                id: "a".into(),
                space: "個人".into()
            },
            // b and c, out of every Space's folder: left there, keep their Space.
            Step::SetSpace {
                id: "d".into(),
                space: "文瀾".into()
            },
            keep("e", "個人"),
        ]
    );
}

#[test]
fn team_spaces_never_change_from_the_folder() {
    let mut layout_input = input(&["文瀾", "產品"]);
    layout_input.teams = BTreeMap::from([("產品".to_string(), Some("Acme".to_string()))]);
    let layout = layout(&layout_input);
    let teams = BTreeSet::from(["產品".to_string()]);
    let pages = [
        // Dragged into the team's folder: not shared by that.
        page("a", "Acme/產品/x.md", Some("文瀾")),
        page("b", "Acme/產品/y.md", None),
        // Dragged out of the team's folder: not taken off the team.
        page("c", "文瀾/z.md", Some("產品")),
    ];
    let placed = record(&[("a", "文瀾"), ("b", ""), ("c", "Acme/產品")]);
    assert!(plan(&pages, &layout, &teams, &placed, &[]).is_empty());
}

#[test]
fn a_renamed_space_folder_renames_the_space() {
    let mut layout_input = input(&["文瀾", "個人"]);
    layout_input.top_folders = vec!["Wenlan".into(), "個人".into(), "其他".into()];
    let layout = layout(&layout_input);
    let pages = [
        page("a", "Wenlan/x.md", Some("文瀾")),
        page("b", "Wenlan/sub/y.md", Some("文瀾")),
        page("c", "個人/z.md", Some("個人")),
    ];
    let placed = record(&[("a", "文瀾"), ("b", "文瀾"), ("c", "個人")]);
    assert_eq!(
        folder_renames(&pages, &layout, &BTreeSet::new(), &layout_input, &placed),
        vec![("文瀾".to_string(), "Wenlan".to_string())]
    );
    // Its pages spread over two folders, or one that is already a Space's: no rename.
    let spread = [
        page("a", "Wenlan/x.md", Some("文瀾")),
        page("b", "其他/y.md", Some("文瀾")),
    ];
    assert!(folder_renames(&spread, &layout, &BTreeSet::new(), &layout_input, &placed).is_empty());
    let into_other = [page("a", "個人/x.md", Some("文瀾"))];
    assert!(folder_renames(
        &into_other,
        &layout,
        &BTreeSet::new(),
        &layout_input,
        &placed
    )
    .is_empty());
}

#[test]
fn turned_on_again_or_with_a_lost_record_nothing_nests() {
    let layout = layout(&input(&["A", "B"]));
    let pages = [
        page("a", "A/sub/x.md", Some("B")),
        page("b", "B/y.md", Some("B")),
    ];
    let steps = plan(&pages, &layout, &BTreeSet::new(), &Record::default(), &[]);
    assert_eq!(steps, vec![mv("a", "A/sub/x.md", "B/sub"), keep("b", "B")]);
}

#[test]
fn folder_names_match_in_any_case_or_unicode_form() {
    let layout = layout(&input(&["Café"]));
    let nfd = "Cafe\u{301}";
    let pages = [
        page("a", &format!("{nfd}/x.md"), Some("Café")),
        page("b", "CAFÉ/y.md", Some("Café")),
    ];
    let steps = plan(
        &pages,
        &layout,
        &BTreeSet::new(),
        &record(&[("a", "Café"), ("b", "Café")]),
        &[],
    );
    assert_eq!(steps, vec![keep("a", "Café"), keep("b", "Café")]);
}

#[test]
fn joining_a_team_moves_the_space_under_the_team_folder() {
    let mut layout_input = input(&["產品"]);
    layout_input.teams = BTreeMap::from([("產品".to_string(), Some("Acme".to_string()))]);
    let layout = layout(&layout_input);
    let pages = [page("a", "產品/x.md", Some("產品"))];
    let steps = plan(
        &pages,
        &layout,
        &BTreeSet::new(),
        &record(&[("a", "產品")]),
        &[],
    );
    assert_eq!(steps, vec![mv("a", "產品/x.md", "Acme/產品")]);
}

#[test]
fn linked_vault_folders_are_left_alone() {
    let layout = layout(&input(&["文瀾"]));
    let pages = [page("a", "My Vault/x.md", Some("文瀾"))];
    assert!(plan(
        &pages,
        &layout,
        &BTreeSet::new(),
        &Record::default(),
        &["My Vault".into()]
    )
    .is_empty());
}

#[test]
fn turning_off_keeps_the_record_for_turning_on_again() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!is_on(dir.path()));
    turn_on(dir.path()).unwrap();
    assert!(is_on(dir.path()));
    let mut kept = read_record(dir.path()).unwrap().unwrap();
    kept.placed.insert("a".into(), "文瀾".into());
    save_record(dir.path(), &kept).unwrap();
    turn_off(dir.path()).unwrap();
    assert!(!is_on(dir.path()));
    turn_on(dir.path()).unwrap();
    assert_eq!(read_record(dir.path()).unwrap().unwrap(), kept);
}
