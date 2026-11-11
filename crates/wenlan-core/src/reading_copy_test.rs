use super::*;
use std::fs;

const UNFILED: &str = "未分類";

fn page(id: &str, title: &str, space: Option<&str>, body: &str) -> CopyPage {
    CopyPage {
        id: id.into(),
        title: title.into(),
        space: space.map(String::from),
        body: body.into(),
        modified: "2026-11-11T00:00:00Z".into(),
    }
}

fn run(dir: &Path, pages: &[CopyPage], state: &mut CopyState) -> CopyResult {
    write_reading_copy(dir, pages, state, "2026-11-11", UNFILED).unwrap()
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap()
}

#[test]
fn first_write_then_second_pass_is_quiet() {
    let tmp = tempfile::tempdir().unwrap();
    let mut st = CopyState::default();
    let pages = vec![
        page("a1", "Alpha", Some("Work"), "body A"),
        page("b2", "Beta", None, "body B"),
    ];
    let r = run(tmp.path(), &pages, &mut st);
    assert_eq!((r.written, r.removed, r.unchanged), (2, 0, 0));
    let a = read(tmp.path(), "Work/Alpha.md");
    assert_eq!(
        a,
        "---\ntitle: \"Alpha\"\nspace: \"Work\"\nupdated: \"2026-11-11T00:00:00Z\"\n---\n\nbody A"
    );
    assert!(!a.contains("a1"));
    assert!(read(tmp.path(), "未分類/Beta.md").contains("space: \"未分類\""));
    let r = run(tmp.path(), &pages, &mut st);
    assert_eq!((r.written, r.removed, r.unchanged), (0, 0, 2));
}

#[test]
fn body_edit_writes_one_file() {
    let tmp = tempfile::tempdir().unwrap();
    let mut st = CopyState::default();
    let mut pages = vec![
        page("a1", "A", Some("W"), "x"),
        page("b2", "B", Some("W"), "y"),
    ];
    run(tmp.path(), &pages, &mut st);
    pages[0].body = "x2".into();
    let r = run(tmp.path(), &pages, &mut st);
    assert_eq!((r.written, r.unchanged), (1, 1));
    assert!(read(tmp.path(), "W/A.md").ends_with("x2"));
}

#[test]
fn rename_and_move_remove_old_file_and_empty_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let mut st = CopyState::default();
    let mut pages = vec![page("a1", "Old", Some("W"), "x")];
    run(tmp.path(), &pages, &mut st);
    pages[0].title = "New".into();
    let r = run(tmp.path(), &pages, &mut st);
    assert_eq!((r.written, r.removed), (1, 1));
    assert!(!tmp.path().join("W/Old.md").exists());
    assert!(tmp.path().join("W/New.md").exists());

    pages[0].space = Some("Home".into());
    let r = run(tmp.path(), &pages, &mut st);
    assert_eq!((r.written, r.removed), (1, 1));
    assert!(tmp.path().join("Home/New.md").exists());
    assert!(!tmp.path().join("W").exists(), "empty Space folder removed");
    assert_eq!(st.files.len(), 1);
}

#[test]
fn archived_or_deleted_page_is_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut st = CopyState::default();
    let pages = vec![
        page("a1", "A", Some("W"), "x"),
        page("b2", "B", Some("W"), "y"),
    ];
    run(tmp.path(), &pages, &mut st);
    let r = run(tmp.path(), &pages[1..], &mut st);
    assert_eq!(r.removed, 1);
    assert!(!tmp.path().join("W/A.md").exists());
    assert!(
        tmp.path().join("W/B.md").exists(),
        "folder kept while non-empty"
    );
}

#[test]
fn same_title_collision_is_deterministic() {
    let tmp = tempfile::tempdir().unwrap();
    let mut st = CopyState::default();
    let p1 = vec![
        page("z9", "Same", Some("W"), "z"),
        page("a1", "Same", Some("W"), "a"),
        page("m5", "same", Some("W"), "m"),
    ];
    run(tmp.path(), &p1, &mut st);
    assert!(read(tmp.path(), "W/Same.md").ends_with("\n\na"));
    assert!(read(tmp.path(), "W/same (2).md").ends_with("\n\nm"));
    assert!(read(tmp.path(), "W/Same (3).md").ends_with("\n\nz"));
    // Input order does not matter and nothing is renamed on a repeat pass.
    let mut p2 = p1.clone();
    p2.reverse();
    let r = run(tmp.path(), &p2, &mut st);
    assert_eq!((r.written, r.removed, r.unchanged), (0, 0, 3));
}

#[test]
fn unsafe_names_are_sanitized() {
    let tmp = tempfile::tempdir().unwrap();
    let mut st = CopyState::default();
    let long = "長".repeat(300);
    let pages = vec![
        page("a1", "a/b\\c:d*e?f\"g<h>i|j\u{7}.  ", Some("Sp:ace/x"), "1"),
        page("b2", "CON", Some("W"), "2"),
        page("c3", "nul.txt", Some("W"), "3"),
        page("d4", "  ..  ", Some("W"), "4"),
        page("e5", &long, Some("W"), "5"),
    ];
    run(tmp.path(), &pages, &mut st);
    assert!(tmp.path().join("Spacex/abcdefghij.md").exists());
    assert!(tmp.path().join("W/CON_.md").exists());
    assert!(tmp.path().join("W/nul.txt_.md").exists());
    assert!(tmp.path().join("W/d4.md").exists());
    for rel in st.files.keys() {
        let name = rel.rsplit('/').next().unwrap();
        assert!(name.len() <= 190, "{name}");
    }
    assert!(st.files.keys().any(|k| k.starts_with("W/長長")));
}

#[test]
fn file_not_in_state_is_never_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut st = CopyState::default();
    run(tmp.path(), &[page("a1", "A", Some("W"), "x")], &mut st);
    fs::write(tmp.path().join("W/mine.md"), "mine").unwrap();
    fs::create_dir_all(tmp.path().join("收件匣")).unwrap();
    fs::write(tmp.path().join("收件匣/note.md"), "n").unwrap();
    let r = run(tmp.path(), &[], &mut st);
    assert_eq!(r.removed, 1);
    assert!(tmp.path().join("W/mine.md").exists());
    assert!(tmp.path().join("W").exists());
    assert!(tmp.path().join("收件匣/note.md").exists());
}

#[test]
fn mass_removal_is_held_back() {
    let tmp = tempfile::tempdir().unwrap();
    let mut st = CopyState::default();
    let pages: Vec<CopyPage> = (0..30)
        .map(|i| page(&format!("id{i:02}"), &format!("P{i}"), Some("W"), "x"))
        .collect();
    run(tmp.path(), &pages, &mut st);
    let r = run(tmp.path(), &[], &mut st);
    assert!(r.removal_held_back);
    assert_eq!(r.removed, 0);
    assert_eq!(st.files.len(), 30);
    assert!(tmp.path().join("W/P0.md").exists());

    // 21 of 100 is more than 20 but not more than half: allowed.
    let tmp = tempfile::tempdir().unwrap();
    let mut st = CopyState::default();
    let pages: Vec<CopyPage> = (0..100)
        .map(|i| page(&format!("id{i:03}"), &format!("P{i}"), Some("W"), "x"))
        .collect();
    run(tmp.path(), &pages, &mut st);
    let r = run(tmp.path(), &pages[21..], &mut st);
    assert!(!r.removal_held_back);
    assert_eq!(r.removed, 21);
    // Exactly 20 of 30 (over half but not over 20) is allowed too.
    let tmp = tempfile::tempdir().unwrap();
    let mut st = CopyState::default();
    run(tmp.path(), &pages[..30], &mut st);
    let r = run(tmp.path(), &pages[20..30], &mut st);
    assert_eq!(r.removed, 20);
}

#[test]
fn external_edit_is_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    let mut st = CopyState::default();
    let pages = vec![page("a1", "A", Some("W"), "x")];
    run(tmp.path(), &pages, &mut st);
    fs::write(tmp.path().join("W/A.md"), "phone edit").unwrap();
    let r = run(tmp.path(), &pages, &mut st);
    assert_eq!(r.written, 1);
    assert!(read(tmp.path(), "W/A.md").ends_with("\n\nx"));
}

#[test]
fn instructions_written_once_and_on_change() {
    let tmp = tempfile::tempdir().unwrap();
    let mut st = CopyState::default();
    let pages = vec![page("a1", "A", Some("W"), "x")];
    write_reading_copy(tmp.path(), &pages, &mut st, "T1", UNFILED).unwrap();
    let text = read(tmp.path(), INSTRUCTIONS_FILE);
    assert!(text.contains("T1") && text.contains("收件匣"));
    assert!(!tmp.path().join("收件匣").exists());
    write_reading_copy(tmp.path(), &pages, &mut st, "T2", UNFILED).unwrap();
    assert!(
        read(tmp.path(), INSTRUCTIONS_FILE).contains("T1"),
        "no change, no rewrite"
    );
    let changed = vec![page("a1", "A", Some("W"), "y")];
    write_reading_copy(tmp.path(), &changed, &mut st, "T3", UNFILED).unwrap();
    assert!(read(tmp.path(), INSTRUCTIONS_FILE).contains("T3"));
    fs::remove_file(tmp.path().join(INSTRUCTIONS_FILE)).unwrap();
    write_reading_copy(tmp.path(), &changed, &mut st, "T4", UNFILED).unwrap();
    assert!(read(tmp.path(), INSTRUCTIONS_FILE).contains("T4"));
}

#[test]
fn no_temp_files_left_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let mut st = CopyState::default();
    run(tmp.path(), &[page("a1", "A", Some("W"), "x")], &mut st);
    for entry in fs::read_dir(tmp.path().join("W")).unwrap() {
        assert!(!entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with('.'));
    }
}

#[test]
fn case_only_rename_leaves_one_file() {
    let tmp = tempfile::tempdir().unwrap();
    let mut st = CopyState::default();
    run(tmp.path(), &[page("a1", "Title", Some("W"), "x")], &mut st);
    run(tmp.path(), &[page("a1", "title", Some("W"), "x")], &mut st);
    let names: Vec<String> = fs::read_dir(tmp.path().join("W"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["title.md".to_string()]);
    assert_eq!(st.files.len(), 1);
}
