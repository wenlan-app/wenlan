// SPDX-License-Identifier: Apache-2.0
use super::*;

const CODE: &str = "fn a() {}\n\nfn b() {\n    1\n}\n\nfn c() {}\n";

const PAGE: &str = "---\ntitle: \"B\"\ntype: page\norigin_id: page_b\nsources:\n  - {resource: \"repo://src/lib.rs#L3-L5\"}\n---\n\n# B\n\n`b` returns one.\n\n## Code\n\n- [`src/lib.rs` lines 3-5](../../src/lib.rs#L3-L5)\n";

fn repo_with(code: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join("wiki/pages")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), code).unwrap();
    std::fs::write(dir.path().join("wiki/pages/b.md"), PAGE).unwrap();
    let manifest = build_manifest(
        Some("abc".to_owned()),
        [(
            "pages/b.md".to_owned(),
            "page_b".to_owned(),
            vec![("src/lib.rs".to_owned(), 3, 5)],
        )],
        |path| (path == "src/lib.rs").then_some(code),
    );
    write_manifest(&dir.path().join("wiki"), &manifest).unwrap();
    dir
}

#[test]
fn unchanged_code_needs_nothing() {
    let repo = repo_with(CODE);
    let found = check(repo.path(), "wiki").unwrap();
    assert_eq!(found.unchanged, 1);
    assert!(found.pages.is_empty());
    assert_eq!(found.base.as_deref(), Some("abc"));
}

#[test]
fn moved_code_gets_new_line_numbers_without_a_model() {
    let repo = repo_with(CODE);
    std::fs::write(
        repo.path().join("src/lib.rs"),
        format!("// new header\n// two\n{CODE}"),
    )
    .unwrap();
    let found = check(repo.path(), "wiki").unwrap();
    assert_eq!(found.moved().count(), 1);
    assert_eq!(found.to_rewrite().count(), 0);

    assert_eq!(apply_moves(repo.path(), "wiki", &found).unwrap(), 1);
    let page = std::fs::read_to_string(repo.path().join("wiki/pages/b.md")).unwrap();
    assert!(page.contains("repo://src/lib.rs#L5-L7"), "{page}");
    assert!(
        page.contains("- [`src/lib.rs` lines 5-7](../../src/lib.rs#L5-L7)"),
        "{page}"
    );
    assert!(!page.contains("L3-L5"), "{page}");
    assert!(page.contains("`b` returns one."));
    assert!(check(repo.path(), "wiki").unwrap().pages.is_empty());
}

#[test]
fn changed_code_asks_for_a_rewrite_then_reanchors_what_the_tool_wrote() {
    let repo = repo_with(CODE);
    std::fs::write(
        repo.path().join("src/lib.rs"),
        "fn a() {}\n\nfn b() {\n    2\n}\n\nfn c() {}\n",
    )
    .unwrap();
    let found = check(repo.path(), "wiki").unwrap();
    let pages: Vec<_> = found.to_rewrite().collect();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].file, "pages/b.md");
    let prompt = rewrite_prompt("wiki", &found);
    assert!(prompt.contains("`wiki/pages/b.md`"), "{prompt}");
    assert!(prompt.contains("git diff abc HEAD"), "{prompt}");

    // The tool fixes the prose and now cites only lines 3-4.
    let edited = PAGE
        .replace("`b` returns one.", "`b` returns two.")
        .replace("#L3-L5\"", "#L3-L4\"");
    std::fs::write(repo.path().join("wiki/pages/b.md"), edited).unwrap();
    let done = reanchor(
        repo.path(),
        "wiki",
        &["pages/b.md".to_owned()],
        Some("def".to_owned()),
    )
    .unwrap();
    assert_eq!(
        done,
        vec![Reanchored::Updated {
            file: "pages/b.md".to_owned()
        }]
    );
    let page = std::fs::read_to_string(repo.path().join("wiki/pages/b.md")).unwrap();
    assert!(page.contains("`b` returns two."));
    assert!(
        page.contains("- [`src/lib.rs` lines 3-4](../../src/lib.rs#L3-L4)\n"),
        "{page}"
    );
    assert_eq!(page.matches("## Code").count(), 1, "{page}");
    let after = check(repo.path(), "wiki").unwrap();
    assert!(after.pages.is_empty(), "{after:?}");
    assert_eq!(after.base.as_deref(), Some("def"));
}

#[test]
fn a_page_naming_lines_that_do_not_exist_is_left_for_a_person() {
    let repo = repo_with(CODE);
    let edited = PAGE.replace("#L3-L5\"", "#L3-L50\"");
    std::fs::write(repo.path().join("wiki/pages/b.md"), edited).unwrap();
    let done = reanchor(repo.path(), "wiki", &["pages/b.md".to_owned()], None).unwrap();
    assert!(matches!(&done[0], Reanchored::Broken { reason, .. } if reason.contains("lines 3-50")));
}

#[test]
fn deleted_code_is_missing() {
    let repo = repo_with(CODE);
    std::fs::remove_file(repo.path().join("src/lib.rs")).unwrap();
    let found = check(repo.path(), "wiki").unwrap();
    assert_eq!(found.pages[0].spans[0].state, SpanState::Missing);
    assert!(found.pages[0].needs_rewrite());
}

#[test]
fn rewriting_code_keeps_memory_sources_and_other_frontmatter() {
    let text = "---\ntitle: \"T\"\nsources:\n  - {id: \"mem_1\", resource: \"wenlan://memory/mem_1\"}\n  - {resource: \"repo://a.rs#L1-L2\"}\nstatus: stable\n---\n\nBody.\n";
    let out = rewrite_page_code(text, &[("b.rs".to_owned(), 4, 4)], "wiki");
    assert_eq!(
        out,
        "---\ntitle: \"T\"\nsources:\n  - {id: \"mem_1\", resource: \"wenlan://memory/mem_1\"}\n  - {resource: \"repo://b.rs#L4-L4\"}\nstatus: stable\n---\n\nBody.\n\n## Code\n\n- [`b.rs` line 4](../../b.rs#L4)\n\n"
    );
    assert_eq!(frontmatter_spans(&out), vec![("b.rs".to_owned(), 4, 4)]);
}

#[test]
fn spans_outside_the_repository_are_never_read() {
    assert!(read_code(Path::new("/tmp"), "../etc/passwd").is_none());
    assert!(read_code(Path::new("/tmp"), "/etc/passwd").is_none());
}

#[test]
fn the_workflow_runs_refresh_daily_with_the_chosen_tool() {
    use wenlan_types::repo_wiki::RepoWikiAgent;
    let claude = workflow("docs/wiki", RepoWikiAgent::Claude, 1);
    assert!(claude.contains("    - cron: \"17 1 * * *\"\n"), "{claude}");
    assert!(claude.contains("CLAUDE_CODE_OAUTH_TOKEN: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}"));
    assert!(claude.contains("GH_TOKEN: ${{ github.token }}"));
    assert!(claude
        .contains("run: wenlan wiki refresh --folder docs/wiki --agent claude --pull-request\n"));
    let codex = workflow("wiki", RepoWikiAgent::Codex, 23);
    assert!(codex.contains("CODEX_API_KEY: ${{ secrets.OPENAI_API_KEY }}"));
    assert!(codex.contains("npm install --global @openai/codex"));
    assert!(!codex.contains("ANTHROPIC"));
    if std::env::var_os("PRINT_WORKFLOW").is_some() {
        println!("{claude}");
    }
}
