// SPDX-License-Identifier: Apache-2.0

use super::*;

fn markdown(content: &str) -> String {
    render(&parse_outline(content))
}

#[test]
fn outline_becomes_paragraphs_and_a_shallower_list() {
    let page = "- # Trip plan\n- We fly to Taipei in March.\n\t- Book the hotel\n\t\t- Near the station\n- Pack light.\n";
    assert_eq!(
        markdown(page),
        "# Trip plan\n\nWe fly to Taipei in March.\n\n- Book the hotel\n  - Near the station\n\nPack light.\n"
    );
}

#[test]
fn two_space_indents_and_continuation_lines() {
    let page = "- First block\n  second line of it\n  - Child\n    child continues\n";
    assert_eq!(
        markdown(page),
        "First block\nsecond line of it\n\n- Child\n  child continues\n"
    );
}

#[test]
fn page_and_block_properties_are_read_or_dropped() {
    let page = "title:: Reading list\ntags:: [[books]], learning\nalias:: Books\n\n- Dune\n  id:: 6531f1a2-0000-4000-8000-000000000001\n  collapsed:: true\n";
    let outline = parse_outline(page);
    assert_eq!(
        outline.properties,
        vec![
            ("title".to_string(), "Reading list".to_string()),
            ("tags".to_string(), "[[books]], learning".to_string()),
            ("alias".to_string(), "Books".to_string()),
        ]
    );
    assert_eq!(render(&outline), "Dune\n");
}

#[test]
fn page_properties_in_a_first_block() {
    let outline = parse_outline("- tags:: work\n  type:: decision\n- We picked Rust.\n");
    assert_eq!(
        outline.properties,
        vec![
            ("tags".to_string(), "work".to_string()),
            ("type".to_string(), "decision".to_string()),
        ]
    );
    assert_eq!(render(&outline), "We picked Rust.\n");
}

#[test]
fn block_refs_and_macros_become_text() {
    let page = "- The launch moved to March.\n  id:: 6531F1A2-0000-4000-8000-000000000001\n- See ((6531f1a2-0000-4000-8000-000000000001)) and ((00000000-0000-4000-8000-000000000000)).\n- {{embed [[Roadmap]]}}\n- {{query (todo now)}}\n";
    assert_eq!(
        markdown(page),
        "The launch moved to March.\n\nSee The launch moved to March. and .\n\n[[Roadmap]]\n"
    );
}

#[test]
fn code_fences_keep_their_dashes() {
    let page = "- Example:\n  ```\n  - not a block\n  key:: not a property\n  ```\n";
    assert_eq!(
        markdown(page),
        "Example:\n```\n- not a block\nkey:: not a property\n```\n"
    );
}

#[test]
fn titles_follow_logseq_file_names() {
    assert_eq!(
        title_from_path(Path::new("/g/journals/2026_10_21.md")),
        "2026-10-21"
    );
    assert_eq!(
        title_from_path(Path::new("/g/pages/projects___atlas.md")),
        "projects/atlas"
    );
    assert_eq!(title_from_path(Path::new("/g/pages/what%3F.md")), "what?");
    assert_eq!(
        title_from_path(Path::new("/g/pages/2026_10_21.md")),
        "2026_10_21"
    );
    assert_eq!(title_from_path(Path::new("/g/pages/100%.md")), "100%");
}

#[test]
fn link_only_pages_are_skipped_but_linked_prose_is_kept() {
    assert!(is_link_index("- [[Projects]]\n- [[Reading]] #books\n"));
    assert!(!is_link_index("Met [[Alice]] about [[Atlas]].\n"));
    assert!(!is_link_index(""));
}

#[test]
fn page_to_documents_uses_title_tags_and_agent() {
    let content = "title:: Atlas notes\ntags:: work, [[planning]]\n\n- # Atlas\n- Atlas ships in March, after the hotel booking and the venue are settled with the team.\n";
    let docs = page_to_documents("logseq-g", Path::new("/g/pages/atlas.md"), content, 7);
    assert_eq!(docs.len(), 1);
    let doc = &docs[0];
    assert_eq!(doc.source_id, "logseq-g::/g/pages/atlas.md::0");
    assert_eq!(doc.title, "Atlas notes");
    assert_eq!(doc.source_agent.as_deref(), Some(LOGSEQ_SOURCE_AGENT));
    assert_eq!(doc.space.as_deref(), Some("work"));
    assert_eq!(
        doc.metadata.get("tags").map(String::as_str),
        Some("work,planning")
    );
    assert_eq!(doc.last_modified, 7);
    assert!(doc.content.contains("Atlas ships in March"));
    assert!(!doc.content.contains("tags::"));
    assert!(!doc.content.contains("- # Atlas"));
}

#[test]
fn empty_journal_and_projected_pages_store_nothing() {
    let journal = Path::new("/g/journals/2026_10_21.md");
    assert!(page_to_documents("g", journal, "-\n", 1).is_empty());
    assert!(page_to_documents("g", journal, "- \n- \n", 1).is_empty());
    let projected = "---\norigin_id: page_1\n---\n\n- Wenlan wrote this.\n";
    assert!(page_to_documents("g", Path::new("/g/pages/p.md"), projected, 1).is_empty());
}

#[test]
fn scan_reads_pages_and_journals_only() {
    let root = tempfile::tempdir().unwrap();
    for (rel, text) in [
        ("logseq/config.edn", "{}"),
        ("logseq/bak/pages/atlas/2026.md", "- old copy"),
        ("pages/atlas.md", "- Atlas"),
        ("pages/.hidden.md", "- hidden"),
        ("journals/2026_10_21.md", "- Today"),
        ("assets/image.md", "- not a page"),
        ("notes.md", "- outside"),
    ] {
        let path = root.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    assert!(is_logseq_graph(root.path()));
    assert!(has_any_page(root.path()));
    let mut found: Vec<String> = scan_graph(root.path())
        .into_iter()
        .map(|p| {
            p.strip_prefix(root.path())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    found.sort();
    assert_eq!(found, vec!["journals/2026_10_21.md", "pages/atlas.md"]);

    let empty = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(empty.path().join("logseq")).unwrap();
    std::fs::create_dir_all(empty.path().join("pages")).unwrap();
    assert!(is_logseq_graph(empty.path()));
    assert!(!has_any_page(empty.path()));
    assert!(!is_logseq_graph(root.path().join("pages").as_path()));
}
