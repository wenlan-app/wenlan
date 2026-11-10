// SPDX-License-Identifier: Apache-2.0
use super::*;
use std::time::{Duration, SystemTime};

struct Setup {
    _dir: tempfile::TempDir,
    vault: PathBuf,
    pages: PathBuf,
    state: PathBuf,
    link: LinkedVault,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("second-brain");
    let pages = dir.path().join("pages");
    let state = dir.path().join("data/linked-vaults");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::create_dir_all(&pages).unwrap();
    let link = LinkedVault {
        path: vault.clone(),
        folder: "second-brain".into(),
        ai_edits: None,
        linked_at: None,
    };
    Setup {
        _dir: dir,
        vault,
        pages,
        state,
        link,
    }
}

impl Setup {
    fn wiki(&self, rel: &str) -> PathBuf {
        self.pages.join("second-brain").join(rel)
    }

    /// A pass a minute from now, so every file has settled.
    fn pass(&self) -> BridgeStats {
        bridge_pass_at(
            &self.link,
            &self.pages,
            &self.state,
            SystemTime::now() + Duration::from_secs(60),
        )
        .unwrap()
    }

    fn put_vault(&self, rel: &str, text: &str) {
        let path = self.vault.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn put_wiki(&self, rel: &str, text: &str) {
        let path = self.wiki(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn vault_text(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.vault.join(rel)).ok()
    }

    fn wiki_text(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.wiki(rel)).ok()
    }

    /// What the page watcher does when it makes a Wiki copy a page.
    fn stamp(&self, rel: &str, id: &str) {
        let text = self.wiki_text(rel).unwrap();
        let stamped = match text.strip_prefix("---\n") {
            Some(rest) => format!("---\norigin_id: {id}\n{rest}"),
            None => format!("---\norigin_id: {id}\n---\n\n{text}"),
        };
        std::fs::write(self.wiki(rel), stamped).unwrap();
    }

    /// Every file under the vault, relative, with its bytes.
    fn vault_snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else {
                    let rel = path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    out.insert(rel, std::fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.vault, &self.vault, &mut out);
        out
    }
}

const NOTE: &str =
    "---\ntags: [book]\naliases: [DW]\n---\nFour hours a day.\n\nSee [[Atomic Habits]].\n";

#[test]
fn linking_a_vault_changes_nothing_in_it() {
    let s = setup();
    s.put_vault("讀書筆記/深度工作.md", NOTE);
    s.put_vault("收件匣.md", "no frontmatter\n");
    s.put_vault(".obsidian/app.json", "{}");
    s.put_vault("attachments/cover.png", "png");
    let before = s.vault_snapshot();

    let stats = s.pass();
    assert_eq!(stats.to_wiki, 2, "{stats:?}");
    assert_eq!(s.wiki_text("讀書筆記/深度工作.md").as_deref(), Some(NOTE));
    assert_eq!(
        s.wiki_text("收件匣.md").as_deref(),
        Some("no frontmatter\n")
    );
    assert!(!s.wiki(".obsidian/app.json").exists());

    // The page watcher stamps the copies; a later pass still leaves the vault alone.
    s.stamp("讀書筆記/深度工作.md", "page_1");
    s.stamp("收件匣.md", "page_2");
    let mut stats = s.pass();
    stats.new_pages.sort();
    assert_eq!(
        stats,
        BridgeStats {
            new_pages: vec!["page_1".into(), "page_2".into()],
            ..BridgeStats::default()
        }
    );
    assert_eq!(
        s.vault_snapshot(),
        before,
        "the vault must be byte-identical"
    );
}

#[test]
fn an_edit_in_obsidian_reaches_the_page_and_keeps_its_id() {
    let s = setup();
    s.put_vault("a.md", NOTE);
    s.pass();
    s.stamp("a.md", "page_1");
    s.pass();

    let edited = NOTE.replace("Four hours", "Three hours");
    s.put_vault("a.md", &edited);
    let stats = s.pass();
    assert_eq!(stats.to_wiki, 1, "{stats:?}");
    let copy = s.wiki_text("a.md").unwrap();
    assert!(
        copy.starts_with("---\norigin_id: page_1\ntags: [book]"),
        "{copy}"
    );
    assert!(copy.contains("Three hours"));
    assert_eq!(s.vault_text("a.md").as_deref(), Some(edited.as_str()));
}

#[test]
fn an_edit_in_wenlan_is_saved_to_the_note_without_the_id_line() {
    let s = setup();
    s.put_vault("a.md", NOTE);
    s.put_vault("plain.md", "Hello\n");
    s.pass();
    s.stamp("a.md", "page_1");
    s.stamp("plain.md", "page_2");
    s.pass();

    // What the page writer puts in the file after an edit in the app.
    s.put_wiki(
        "a.md",
        "---\norigin_id: page_1\ntags: [book]\naliases: [DW]\n---\n\nFive hours a day.\n\nSee [[Atomic Habits]].\n",
    );
    s.put_wiki("plain.md", "---\norigin_id: page_2\n---\n\nHello there\n");
    let stats = s.pass();
    assert_eq!(stats.to_vault, 2, "{stats:?}");
    assert_eq!(
        s.vault_text("a.md").as_deref(),
        Some("---\ntags: [book]\naliases: [DW]\n---\n\nFive hours a day.\n\nSee [[Atomic Habits]].\n")
    );
    assert_eq!(s.vault_text("plain.md").as_deref(), Some("Hello there\n"));
    // And it settles: the next pass copies nothing back.
    assert_eq!(s.pass(), BridgeStats::default());
}

#[test]
fn a_rewrite_that_only_moves_blank_lines_leaves_the_note_alone() {
    let s = setup();
    s.put_vault("a.md", NOTE);
    s.pass();
    s.stamp("a.md", "page_1");
    s.pass();
    // The page writer puts a blank line after the frontmatter.
    s.put_wiki(
        "a.md",
        "---\norigin_id: page_1\ntags: [book]\naliases: [DW]\n---\n\nFour hours a day.\n\nSee [[Atomic Habits]].\n",
    );
    let stats = s.pass();
    assert_eq!(stats.to_vault, 0, "{stats:?}");
    assert_eq!(s.vault_text("a.md").as_deref(), Some(NOTE));
}

#[test]
fn a_page_made_in_the_folder_becomes_a_note() {
    let s = setup();
    s.put_wiki("專案/新想法.md", "---\norigin_id: page_9\n---\n\nAn idea\n");
    let stats = s.pass();
    assert_eq!(stats.to_vault, 1, "{stats:?}");
    assert_eq!(s.vault_text("專案/新想法.md").as_deref(), Some("An idea\n"));
}

#[test]
fn both_sides_changed_keeps_the_vault() {
    let s = setup();
    s.put_vault("a.md", "one\n");
    s.pass();
    s.stamp("a.md", "page_1");
    s.pass();
    s.put_vault("a.md", "from obsidian\n");
    s.put_wiki("a.md", "---\norigin_id: page_1\n---\n\nfrom wenlan\n");
    let stats = s.pass();
    assert_eq!((stats.vault_won, stats.to_wiki), (1, 1), "{stats:?}");
    assert_eq!(s.vault_text("a.md").as_deref(), Some("from obsidian\n"));
    assert_eq!(
        s.wiki_text("a.md").as_deref(),
        Some("---\norigin_id: page_1\n---\n\nfrom obsidian\n")
    );
}

#[test]
fn a_note_renamed_in_obsidian_keeps_its_page() {
    let s = setup();
    s.put_vault("a.md", NOTE);
    s.pass();
    s.stamp("a.md", "page_1");
    s.pass();
    std::fs::create_dir_all(s.vault.join("書")).unwrap();
    std::fs::rename(s.vault.join("a.md"), s.vault.join("書/b.md")).unwrap();
    let stats = s.pass();
    assert_eq!(stats.renamed, 1, "{stats:?}");
    assert_eq!(stats.removed_from_wiki, 0);
    assert!(!s.wiki("a.md").exists());
    assert!(s
        .wiki_text("書/b.md")
        .unwrap()
        .contains("origin_id: page_1"));
}

#[test]
fn a_page_renamed_in_wenlan_renames_the_note() {
    let s = setup();
    s.put_vault("a.md", "---\ntitle: A\n---\nbody\n");
    s.pass();
    s.stamp("a.md", "page_1");
    s.pass();
    // A rename in the app moves the file and updates its title line.
    std::fs::remove_file(s.wiki("a.md")).unwrap();
    s.put_wiki("B.md", "---\norigin_id: page_1\ntitle: B\n---\n\nbody\n");
    let stats = s.pass();
    assert_eq!((stats.renamed, stats.moved_to_trash), (1, 0), "{stats:?}");
    assert!(s.vault_text("a.md").is_none());
    assert_eq!(
        s.vault_text("B.md").as_deref(),
        Some("---\ntitle: B\n---\n\nbody\n")
    );
}

#[test]
fn a_note_deleted_in_obsidian_removes_its_copy() {
    let s = setup();
    s.put_vault("a.md", "x\n");
    s.pass();
    s.stamp("a.md", "page_1");
    s.pass();
    std::fs::remove_file(s.vault.join("a.md")).unwrap();
    let stats = s.pass();
    assert_eq!(stats.removed_from_wiki, 1, "{stats:?}");
    assert!(!s.wiki("a.md").exists());
}

#[test]
fn a_page_deleted_in_wenlan_moves_the_note_to_obsidian_trash() {
    let s = setup();
    s.put_vault("a.md", "x\n");
    s.pass();
    s.stamp("a.md", "page_1");
    s.pass();
    std::fs::remove_file(s.wiki("a.md")).unwrap();
    let stats = s.pass();
    assert_eq!(stats.moved_to_trash, 1, "{stats:?}");
    assert!(s.vault_text("a.md").is_none());
    assert_eq!(s.vault_text(".trash/a.md").as_deref(), Some("x\n"));
}

#[test]
fn a_vault_that_is_gone_deletes_nothing() {
    let s = setup();
    for n in 0..30 {
        s.put_vault(&format!("n{n}.md"), &format!("note {n}\n"));
    }
    s.pass();
    // The whole vault disappears (a drive unplugged, a sync app mid-way).
    let moved = s.vault.with_file_name("elsewhere");
    std::fs::rename(&s.vault, &moved).unwrap();
    assert!(bridge_pass(&s.link, &s.pages, &s.state).is_err());
    // Or only its notes go at once.
    std::fs::create_dir_all(&s.vault).unwrap();
    let stats = s.pass();
    assert_eq!(stats.held_gone, Some(30), "{stats:?}");
    assert_eq!(stats.removed_from_wiki, 0);
    assert!(s.wiki("n0.md").exists());
}

#[test]
fn a_note_still_being_saved_waits() {
    let s = setup();
    s.put_vault("a.md", "x\n");
    let stats = bridge_pass(&s.link, &s.pages, &s.state).unwrap();
    assert_eq!(stats.to_wiki, 0, "{stats:?}");
    assert!(!s.wiki("a.md").exists());
}

#[test]
fn the_id_line_is_taken_out_exactly() {
    assert_eq!(
        without_id_line("---\norigin_id: p\n---\n\nHello\n"),
        "Hello\n"
    );
    assert_eq!(
        without_id_line("---\norigin_id: p\ntags: [a]\n---\nBody\n"),
        "---\ntags: [a]\n---\nBody\n"
    );
    assert_eq!(
        without_id_line("---\r\norigin_id: p\r\ntags: [a]\r\n---\r\nBody\r\n"),
        "---\r\ntags: [a]\r\n---\r\nBody\r\n"
    );
    assert_eq!(without_id_line("no frontmatter"), "no frontmatter");
    assert_eq!(
        without_id_line("---\ntags: [a]\n---\nx"),
        "---\ntags: [a]\n---\nx"
    );
}

#[test]
fn a_vault_inside_the_pages_folder_or_around_it_is_refused() {
    let s = setup();
    let inner = s.pages.join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    assert!(check_link(&inner, &s.pages, "inner2", &[]).is_err());
    let outer = s.pages.parent().unwrap().to_path_buf();
    assert!(check_link(&outer, &s.pages, "x", &[]).is_err());
    check_link(&s.vault, &s.pages, "second-brain", &[]).unwrap();
    assert!(check_link(
        &s.vault,
        &s.pages,
        "second-brain",
        std::slice::from_ref(&s.link)
    )
    .is_err());
}

#[test]
fn the_folder_name_is_the_vaults_own_and_never_a_taken_one() {
    let s = setup();
    assert_eq!(folder_name_for(&s.vault, &s.pages, &[]), "second-brain");
    std::fs::create_dir_all(s.pages.join("second-brain")).unwrap();
    std::fs::write(s.pages.join("second-brain/x.md"), "x").unwrap();
    assert_eq!(folder_name_for(&s.vault, &s.pages, &[]), "second-brain 2");
}

#[test]
fn a_note_page_is_reported_once_so_it_can_be_locked() {
    let s = setup();
    s.put_vault("收件匣.md", "# 收件匣\n");
    assert!(
        s.pass().new_pages.is_empty(),
        "no page yet before the watcher stamps the copy"
    );
    s.stamp("收件匣.md", "page_inbox");
    assert_eq!(s.pass().new_pages, vec!["page_inbox".to_string()]);
    assert!(
        s.pass().new_pages.is_empty(),
        "a page already reported is not reported again"
    );
    assert_eq!(
        page_ids(&s.link, &s.state).unwrap(),
        vec!["page_inbox".to_string()]
    );
}

#[test]
fn no_answer_about_ai_changes_counts_as_no() {
    let mut s = setup();
    assert!(!s.link.ai_may_edit());
    s.link.ai_edits = Some(false);
    assert!(!s.link.ai_may_edit());
    s.link.ai_edits = Some(true);
    assert!(s.link.ai_may_edit());
    let json = serde_json::to_string(&LinkedVault {
        path: "/v".into(),
        folder: "v".into(),
        ai_edits: None,
        linked_at: None,
    })
    .unwrap();
    assert_eq!(
        json, r#"{"path":"/v","folder":"v"}"#,
        "an old config keeps reading the same"
    );
}
