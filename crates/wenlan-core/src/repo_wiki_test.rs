// SPDX-License-Identifier: Apache-2.0
use super::*;
use wenlan_types::repo_wiki::RepoWikiAgent;

fn page(id: &str, title: &str, content: &str) -> Page {
    Page {
        matched_section: None,
        id: id.to_string(),
        title: title.to_string(),
        summary: None,
        content: content.to_string(),
        entity_id: None,
        space: Some("site".to_string()),
        source_memory_ids: vec![],
        version: 1,
        status: "active".to_string(),
        created_at: "2026-10-22T00:00:00+00:00".to_string(),
        last_compiled: "2026-10-22T00:00:00+00:00".to_string(),
        last_modified: "2026-10-22T00:00:00+00:00".to_string(),
        sources_updated_count: 0,
        stale_reason: None,
        pending_rebuild: None,
        refresh_blocked_reason: None,
        review_required: false,
        user_edited: false,
        relevance_score: 0.0,
        last_edited_by: None,
        last_edited_at: None,
        human_edited_at: None,
        last_delta_summary: None,
        changelog: None,
        workspace: None,
        creation_kind: "authored".to_string(),
        review_status: "unconfirmed".to_string(),
        citations: Vec::new(),
        kind: "concept".to_string(),
        truth: None,
    }
}

/// Anchors for `code_bases`, taken from `texts` (path to the file's text as
/// the page's writer read it).
fn anchors_for(
    code_bases: &HashMap<String, Vec<CodeBasisLocator>>,
    texts: &[(&str, &str)],
) -> CodeAnchors {
    let mut out = CodeAnchors::new();
    for (page_id, locators) in code_bases {
        for locator in locators {
            let text = texts
                .iter()
                .find(|(path, _)| *path == locator.path)
                .unwrap()
                .1;
            let span = crate::code_anchor::LineSpan {
                start: locator.line_start,
                end: locator.line_end,
            };
            out.insert(
                anchor_key(page_id, locator),
                crate::code_anchor::anchor(text, span).unwrap(),
            );
        }
    }
    out
}

fn locator(root: &str, path: &str, start: u32, end: u32) -> CodeBasisLocator {
    CodeBasisLocator {
        repo_root: root.to_string(),
        path: path.to_string(),
        line_start: start,
        line_end: end,
    }
}

fn sh(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A bare `origin` with one commit on `main`, and a clone of it.
fn repo_with_origin(root: &Path) -> (PathBuf, PathBuf) {
    let origin = root.join("origin.git");
    let clone = root.join("work");
    std::fs::create_dir_all(&origin).unwrap();
    sh(
        &origin,
        &["init", "--quiet", "--bare", "--initial-branch=main"],
    );
    let seed = root.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    sh(&seed, &["init", "--quiet", "--initial-branch=main"]);
    sh(&seed, &["config", "user.name", "Test"]);
    sh(&seed, &["config", "user.email", "test@example.com"]);
    sh(&seed, &["config", "commit.gpgsign", "false"]);
    std::fs::create_dir_all(seed.join("src")).unwrap();
    std::fs::write(seed.join("src/lib.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
    std::fs::write(seed.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(seed.join("CLAUDE.md"), "# Notes\n\nKeep it small.\n").unwrap();
    sh(&seed, &["add", "."]);
    sh(&seed, &["commit", "--quiet", "-m", "seed"]);
    sh(
        &seed,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    sh(&seed, &["push", "--quiet", "origin", "main"]);
    let output = Command::new("git")
        .args(["clone", "--quiet"])
        .arg(&origin)
        .arg(&clone)
        .output()
        .unwrap();
    assert!(output.status.success());
    sh(&clone, &["config", "user.name", "Test"]);
    sh(&clone, &["config", "user.email", "test@example.com"]);
    sh(&clone, &["config", "commit.gpgsign", "false"]);
    let clone = std::fs::canonicalize(clone).unwrap();
    (origin, clone)
}

#[test]
fn folder_must_stay_inside_the_repository() {
    assert_eq!(normalize_folder("wiki").unwrap(), "wiki");
    assert_eq!(normalize_folder("/docs/wiki/").unwrap(), "docs/wiki");
    assert_eq!(normalize_folder("./docs/./wiki").unwrap(), "docs/wiki");
    for bad in ["", "/", "..", "docs/../..", ".git", "a/.git/b", "."] {
        assert!(normalize_folder(bad).is_err(), "{bad:?} must be refused");
    }
}

#[test]
fn github_slug_reads_https_and_ssh_remotes() {
    for remote in [
        "https://github.com/wenlan-app/wenlan.git",
        "https://github.com/wenlan-app/wenlan",
        "git@github.com:wenlan-app/wenlan.git",
        "ssh://git@github.com/wenlan-app/wenlan.git",
    ] {
        assert_eq!(
            github_slug(remote).as_deref(),
            Some("wenlan-app/wenlan"),
            "{remote}"
        );
    }
    assert_eq!(github_slug("https://gitlab.com/a/b.git"), None);
    assert_eq!(github_slug("/tmp/origin.git"), None);
}

#[test]
fn generated_attributes_are_added_once_beside_other_lines() {
    let first = upsert_generated_attributes("*.png binary", "wiki");
    assert_eq!(
        first,
        "*.png binary\n\
         wiki/.wenlan-code.json linguist-generated=true\n\
         wiki/.wenlan-okf-export.json linguist-generated=true\n"
    );
    assert_eq!(upsert_generated_attributes(&first, "wiki"), first);
    // A line already there, even unmarked by us, is not added twice.
    let partial = "wiki/.wenlan-okf-export.json linguist-generated=true\n";
    assert_eq!(
        upsert_generated_attributes(partial, "wiki")
            .matches("okf-export")
            .count(),
        1
    );
    assert_eq!(
        upsert_generated_attributes("", "docs/wiki"),
        "docs/wiki/.wenlan-code.json linguist-generated=true\n\
         docs/wiki/.wenlan-okf-export.json linguist-generated=true\n"
    );
    assert_eq!(upsert_generated_attributes("x\n", "my wiki"), "x\n");
}

#[test]
fn ux_skill_7_agents_block_carries_the_project_rules_and_drops_them_again() {
    let rules = vec![
        "Run the affected\ntests before a push.".to_string(),
        "  ".to_string(),
        format!("sneaky {AGENTS_END}"),
    ];
    let with = upsert_agents_block_with_rules("# Agents\n\nUse pnpm.\n", "wiki", &rules);
    assert!(with.starts_with("# Agents\n\nUse pnpm.\n\n<!-- WENLAN:START -->"));
    assert!(with.contains(
        "## Project rules\n\n- Run the affected tests before a push.\n\nFor a rule's source or reason, use `read_space_rules` if Wenlan is connected to this project's Space.\n<!-- WENLAN:END -->"
    ));
    assert_eq!(with.matches(AGENTS_END).count(), 1);
    let many = (0..AGENTS_BLOCK_MAX_RULES + 5)
        .map(|n| format!("Rule {n}."))
        .collect::<Vec<_>>();
    let capped = upsert_agents_block_with_rules(&with, "wiki", &many);
    assert_eq!(capped.matches("\n- Rule ").count(), AGENTS_BLOCK_MAX_RULES);
    assert!(!capped.contains("affected tests"));
    let without = upsert_agents_block(&capped, "wiki");
    assert!(!without.contains("## Project rules"));
    assert!(without.contains("Use pnpm."));
}

#[test]
fn agents_block_is_appended_then_replaced_in_place() {
    let first = upsert_agents_block("# Rules\n\nBe kind.\n", "wiki");
    assert!(first.starts_with("# Rules\n\nBe kind.\n\n<!-- WENLAN:START -->"));
    assert!(first.contains("`wiki/index.md`"));
    let edited = first.replace("Be kind.", "Be very kind.") + "\nAfter.\n";
    let second = upsert_agents_block(&edited, "docs/wiki");
    assert!(second.contains("Be very kind."));
    assert!(second.contains("After."));
    assert!(second.contains("`docs/wiki/index.md`"));
    assert!(!second.contains("`wiki/index.md`"));
    assert_eq!(second.matches(AGENTS_START).count(), 1);
    assert_eq!(
        upsert_agents_block("", "wiki"),
        format!("{}\n", agents_block(Some("wiki"), &[], None))
    );
}

#[test]
fn code_footer_links_each_span_back_to_the_repository() {
    let root = "/r";
    let locators = vec![
        locator(root, "src/lib.rs", 10, 20),
        locator(root, "src/lib.rs", 10, 20),
        locator(root, "README.md", 3, 3),
        locator("/elsewhere", "x.rs", 1, 2),
    ];
    let out = with_code_footer("Body.\n", &locators, root, "docs/wiki");
    assert!(out.starts_with("Body.\n\n## Code\n\n"));
    assert!(out.contains("- [`README.md` line 3](../../../README.md#L3)\n"));
    assert!(out.contains("- [`src/lib.rs` lines 10-20](../../../src/lib.rs#L10-L20)\n"));
    assert_eq!(out.matches("src/lib.rs` lines").count(), 1);
    assert!(!out.contains("x.rs"));
    assert_eq!(with_code_footer("Body.", &[], root, "wiki"), "Body.");
}

#[test]
fn only_pages_resting_on_this_repositorys_code_are_selected() {
    let root = "/r";
    let code = page("p1", "Module map", "a");
    let mut with_memory = page("p2", "Lessons", "b");
    with_memory.source_memory_ids = vec!["mem_1".into()];
    let no_basis = page("p3", "Thoughts", "c");
    let other_repo = page("p4", "Other", "d");
    let code_bases = HashMap::from([
        ("p1".to_string(), vec![locator("/r/", "src/lib.rs", 1, 2)]),
        ("p2".to_string(), vec![locator(root, "src/lib.rs", 1, 2)]),
        ("p4".to_string(), vec![locator("/other", "x.rs", 1, 2)]),
    ]);
    let (selected, left_out) = select_pages(
        vec![code, with_memory, no_basis, other_repo],
        &code_bases,
        root,
    );
    assert_eq!(
        selected
            .iter()
            .map(|page| page.id.as_str())
            .collect::<Vec<_>>(),
        ["p1"]
    );
    assert_eq!(left_out, 3);
}

#[test]
fn candidates_name_repositories_with_code_pages_most_first() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clone) = repo_with_origin(dir.path());
    let root = clone.to_string_lossy().into_owned();
    let gone = dir.path().join("gone").to_string_lossy().into_owned();
    let mut with_memory = page("p3", "Lessons", "c");
    with_memory.source_memory_ids = vec!["mem_1".into()];
    let pages = vec![
        page("p1", "Module map", "a"),
        page("p2", "Startup", "b"),
        with_memory,
        page("p4", "Old checkout", "d"),
    ];
    let code_bases = HashMap::from([
        ("p1".to_string(), vec![locator(&root, "src/lib.rs", 1, 2)]),
        (
            "p2".to_string(),
            vec![locator(&format!("{root}/"), "src/main.rs", 1, 1)],
        ),
        ("p3".to_string(), vec![locator(&root, "src/lib.rs", 1, 2)]),
        ("p4".to_string(), vec![locator(&gone, "x.rs", 1, 2)]),
    ]);

    let found = candidates(&pages, &code_bases);

    assert_eq!(
        found.len(),
        1,
        "a missing checkout is not offered: {found:?}"
    );
    assert_eq!(found[0].repo_root, root);
    assert_eq!(found[0].pages, 2, "a page citing memories does not count");
    assert!(found[0]
        .remote_url
        .as_deref()
        .is_some_and(|url| url.ends_with("origin.git")));
    assert!(candidates(&pages[2..3], &code_bases).is_empty());
}

#[test]
fn bind_stores_the_top_level_and_keeps_daily_when_not_named() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clone) = repo_with_origin(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    let daily = RepoWikiDaily {
        enabled: true,
        hour: 9,
        agent: RepoWikiAgent::Claude,
        runs_on: Default::default(),
    };
    let bound = bind_at(
        &store,
        "site",
        &clone.join("src"),
        None,
        Some(daily.clone()),
    )
    .unwrap();
    assert_eq!(bound.repo_root, clone.to_string_lossy());
    assert_eq!(bound.folder, "wenlan");
    assert!(bound.remote_url.is_some());
    let again = bind_at(&store, "site", &clone, Some("docs/wiki"), None).unwrap();
    assert_eq!(again.folder, "docs/wiki");
    assert_eq!(again.daily, Some(daily));
    assert_eq!(load_bindings_at(&store).unwrap().len(), 1);
    assert!(bind_at(&store, "site", dir.path(), None, None).is_err());
    let late = RepoWikiDaily {
        enabled: true,
        hour: 24,
        agent: RepoWikiAgent::Codex,
        runs_on: Default::default(),
    };
    assert!(bind_at(&store, "site", &clone, None, Some(late)).is_err());
    assert!(unbind_at(&store, "site").unwrap());
    assert!(!unbind_at(&store, "site").unwrap());
    assert!(binding_at(&store, "site").unwrap().is_none());
}

#[test]
fn publish_pushes_the_wiki_branch_without_touching_the_working_copy() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_origin(dir.path());
    // Uncommitted work in the user's copy must survive untouched. A page
    // citing it waits: its lines may not match the remote's default branch.
    std::fs::write(clone.join("src/main.rs"), "fn main() { run() }\n").unwrap();
    let store = dir.path().join("data/repo-wiki.json");
    let binding = bind_at(&store, "site", &clone, Some("wiki"), None).unwrap();
    let root = binding.repo_root.clone();
    let code_bases = HashMap::from([
        ("p1".to_string(), vec![locator(&root, "src/lib.rs", 1, 2)]),
        ("p2".to_string(), vec![locator(&root, "src/main.rs", 1, 1)]),
    ]);
    let (pages, left_out) = select_pages(
        vec![
            page("p1", "Module map", "What each part owns."),
            page("p2", "Entry point", "Where it starts."),
        ],
        &code_bases,
        &root,
    );
    // The page about main.rs was written against the uncommitted version.
    let anchors = anchors_for(
        &code_bases,
        &[
            ("src/lib.rs", "fn a() {}\nfn b() {}\nfn c() {}\n"),
            ("src/main.rs", "fn main() { run() }\n"),
        ],
    );
    let concept_paths = ConceptPathMap::new();
    let worktree = dir.path().join("data/repo-wiki/site");
    let publish_once = |pages: Vec<Page>| {
        publish(PublishInput {
            rules: Some(Vec::new()),
            publisher: "me".to_string(),
            binding: &binding,
            pages,
            left_out,
            code_bases: &code_bases,
            anchors: &anchors,
            concept_paths: &concept_paths,
            worktree: worktree.clone(),
            gh: None,
            now: 1,
            prepared: None,
            taken_back: BTreeSet::new(),
            took_back: 0,
        })
        .unwrap()
        .result
    };

    let first = publish_once(pages.clone());
    assert_eq!(first.status, RepoWikiPublishStatus::Pushed);
    assert_eq!(first.pages, 1);
    assert_eq!(first.waiting, 1);
    assert_eq!(first.branch.as_deref(), Some(REPO_WIKI_BRANCH));
    assert_eq!(first.compare_url, None, "a local remote is not on GitHub");

    let files = sh(&origin, &["ls-tree", "-r", "--name-only", REPO_WIKI_BRANCH]);
    assert!(files.lines().any(|f| f == "wiki/index.md"), "{files}");
    assert!(
        files.lines().any(|f| f.starts_with("wiki/pages/")),
        "{files}"
    );
    assert!(files.lines().any(|f| f == "AGENTS.md"), "{files}");
    let page_file = files
        .lines()
        .find(|f| f.starts_with("wiki/pages/") && f.ends_with(".md"))
        .unwrap();
    let body = sh(
        &origin,
        &["show", &format!("{REPO_WIKI_BRANCH}:{page_file}")],
    );
    assert!(
        body.contains("[`src/lib.rs` lines 1-2](../../src/lib.rs#L1-L2)"),
        "{body}"
    );
    // Links inside the bundle resolve from the repository host's view.
    let index = sh(
        &origin,
        &["show", &format!("{REPO_WIKI_BRANCH}:wiki/index.md")],
    );
    assert!(index.contains("](pages/"), "{index}");
    assert!(!index.contains("](/pages/"), "{index}");
    // GitHub shows the folder's README.md: the pages in reading order.
    let readme = sh(
        &origin,
        &["show", &format!("{REPO_WIKI_BRANCH}:wiki/README.md")],
    );
    assert!(readme.starts_with("Kept by [Wenlan]"), "{readme}");
    assert!(readme.contains("\n\n* [Module map](pages/"), "{readme}");
    assert!(!files.contains("entry-point"), "{files}");
    let attributes = sh(
        &origin,
        &["show", &format!("{REPO_WIKI_BRANCH}:.gitattributes")],
    );
    assert!(
        attributes.contains("wiki/.wenlan-code.json linguist-generated=true"),
        "{attributes}"
    );
    let claude = sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:CLAUDE.md")]);
    assert!(claude.starts_with("# Notes\n\nKeep it small.\n\n<!-- WENLAN:START -->"));
    // Beside the pages: the code each relies on, anchored in the default
    // branch, for `wenlan wiki refresh` to check from a plain checkout.
    let manifest: crate::repo_wiki_ci::CodeManifest = serde_json::from_str(&sh(
        &origin,
        &[
            "show",
            &format!("{REPO_WIKI_BRANCH}:wiki/.wenlan-code.json"),
        ],
    ))
    .unwrap();
    assert_eq!(
        manifest.base.as_deref(),
        Some(sh(&origin, &["rev-parse", "main"]).as_str())
    );
    let entry = &manifest.pages[page_file.strip_prefix("wiki/").unwrap()];
    assert_eq!(entry.id, "p1");
    assert_eq!(
        entry
            .code
            .iter()
            .map(|span| (span.path.as_str(), span.start, span.end))
            .collect::<Vec<_>>(),
        [("src/lib.rs", 1, 2)]
    );
    // A refresh rewrites a page byte for byte as the export does.
    assert_eq!(
        crate::repo_wiki_ci::rewrite_page_code(
            &body,
            &crate::repo_wiki_ci::frontmatter_spans(&body),
            "wiki"
        ),
        std::fs::read_to_string(worktree.join(page_file)).unwrap()
    );
    let checked = crate::repo_wiki_ci::check(&worktree, "wiki").unwrap();
    assert_eq!((checked.unchanged, checked.pages.len()), (1, 0));

    // The user's copy: same branch, same uncommitted edit, no wiki folder.
    assert_eq!(sh(&clone, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    assert_eq!(sh(&clone, &["status", "--porcelain"]), "M src/main.rs");
    assert!(!clone.join("wiki").exists());
    assert_eq!(sh(&clone, &["branch", "--list", REPO_WIKI_BRANCH]), "");

    // Nothing new: the open branch already holds this tree.
    let second = publish_once(pages.clone());
    assert_eq!(second.status, RepoWikiPublishStatus::NoChanges);

    // A local edit that shifts the cited lines does not pull a published page
    // back: its lines are still found on the default branch.
    std::fs::write(
        clone.join("src/lib.rs"),
        "// local note\nfn a() {}\nfn b() {}\nfn c() {}\n",
    )
    .unwrap();
    let shifted = publish_once(pages.clone());
    assert_eq!(shifted.status, RepoWikiPublishStatus::NoChanges);
    assert_eq!((shifted.pages, shifted.waiting), (1, 1));
    sh(&clone, &["checkout", "--quiet", "--", "src/lib.rs"]);

    // Merged: the default branch holds it, so there is nothing to offer.
    sh(&clone, &["fetch", "--quiet", "origin"]);
    sh(&clone, &["stash", "--quiet"]);
    sh(
        &clone,
        &[
            "merge",
            "--quiet",
            "--ff-only",
            &format!("origin/{REPO_WIKI_BRANCH}"),
        ],
    );
    sh(&clone, &["push", "--quiet", "origin", "main"]);
    sh(&clone, &["stash", "pop", "--quiet"]);
    let third = publish_once(pages.clone());
    assert_eq!(third.status, RepoWikiPublishStatus::NoChanges);
    assert_eq!(third.branch, None);

    // A changed page goes out again on the same branch.
    let mut changed = pages.clone();
    changed[0].content = "What each part owns, and why.".into();
    let fourth = publish_once(changed);
    assert_eq!(fourth.status, RepoWikiPublishStatus::Pushed);
    let body = sh(
        &origin,
        &["show", &format!("{REPO_WIKI_BRANCH}:{page_file}")],
    );
    assert!(body.contains("and why."), "{body}");

    // Unbinding removes the worktree from the user's repository.
    assert_eq!(sh(&clone, &["worktree", "list"]).lines().count(), 2);
    remove_worktree(&root, &worktree);
    assert!(!worktree.exists());
    assert_eq!(sh(&clone, &["worktree", "list"]).lines().count(), 1);
}

#[test]
fn publish_refuses_a_folder_holding_files_wenlan_did_not_write() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clone) = repo_with_origin(dir.path());
    std::fs::create_dir_all(clone.join("wiki")).unwrap();
    std::fs::write(clone.join("wiki/Home.md"), "Hand-written.\n").unwrap();
    sh(&clone, &["add", "."]);
    sh(&clone, &["commit", "--quiet", "-m", "own wiki"]);
    sh(&clone, &["push", "--quiet", "origin", "main"]);
    let store = dir.path().join("data/repo-wiki.json");
    let binding = bind_at(&store, "site", &clone, Some("wiki"), None).unwrap();
    let code_bases = HashMap::from([(
        "p1".to_string(),
        vec![locator(&binding.repo_root, "src/lib.rs", 1, 2)],
    )]);
    let error = publish(PublishInput {
        rules: Some(Vec::new()),
        publisher: "me".to_string(),
        binding: &binding,
        pages: vec![page("p1", "Module map", "x")],
        left_out: 0,
        code_bases: &code_bases,
        anchors: &anchors_for(
            &code_bases,
            &[("src/lib.rs", "fn a() {}\nfn b() {}\nfn c() {}\n")],
        ),
        concept_paths: &ConceptPathMap::new(),
        worktree: dir.path().join("data/repo-wiki/site"),
        gh: None,
        now: 1,
        prepared: None,
        taken_back: BTreeSet::new(),
        took_back: 0,
    })
    .unwrap_err();
    assert!(
        error.to_string().contains("choose another folder"),
        "{error}"
    );
}

#[test]
fn publish_with_no_selected_pages_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let binding = RepoWikiBinding {
        space: "site".into(),
        repo_root: dir.path().to_string_lossy().into_owned(),
        folder: "wiki".into(),
        remote_url: None,
        daily: None,
        last_publish: None,
        last_daily_run: None,
        doc_fixes: Vec::new(),
        doc_check: None,
        kept_edits: Vec::new(),
        follows: Vec::new(),
        take_over: false,
    };
    let result = publish(PublishInput {
        rules: Some(Vec::new()),
        publisher: "me".to_string(),
        binding: &binding,
        pages: vec![],
        left_out: 4,
        code_bases: &HashMap::new(),
        anchors: &CodeAnchors::new(),
        concept_paths: &ConceptPathMap::new(),
        worktree: dir.path().join("wt"),
        gh: None,
        now: 7,
        prepared: None,
        taken_back: BTreeSet::new(),
        took_back: 0,
    })
    .unwrap()
    .result;
    assert_eq!(result.status, RepoWikiPublishStatus::NothingToPublish);
    assert_eq!(result.left_out, 4);
    assert!(!dir.path().join("wt").exists());
}

#[test]
fn daily_is_due_once_a_day_after_its_hour() {
    use chrono::TimeZone;
    let mut binding = RepoWikiBinding {
        space: "site".into(),
        repo_root: "/r".into(),
        folder: "wiki".into(),
        remote_url: None,
        daily: Some(RepoWikiDaily {
            enabled: true,
            hour: 9,
            agent: RepoWikiAgent::Claude,
            runs_on: Default::default(),
        }),
        last_publish: None,
        last_daily_run: None,
        doc_fixes: Vec::new(),
        doc_check: None,
        kept_edits: Vec::new(),
        follows: Vec::new(),
        take_over: false,
    };
    let before = chrono::Local
        .with_ymd_and_hms(2026, 10, 24, 8, 59, 0)
        .unwrap();
    let after = chrono::Local
        .with_ymd_and_hms(2026, 10, 24, 9, 0, 0)
        .unwrap();
    assert!(!daily_due(&binding, before));
    assert!(daily_due(&binding, after));
    binding.last_daily_run = Some("2026-10-24".into());
    assert!(!daily_due(&binding, after));
    binding.last_daily_run = Some("2026-10-23".into());
    assert!(daily_due(&binding, after));
    // Run on GitHub: this computer leaves it to the workflow.
    binding.daily.as_mut().unwrap().runs_on = RepoWikiRunner::Github;
    assert!(!daily_due(&binding, after));
    binding.daily.as_mut().unwrap().runs_on = RepoWikiRunner::Computer;
    binding.daily.as_mut().unwrap().enabled = false;
    assert!(!daily_due(&binding, after));
    binding.daily = None;
    assert!(!daily_due(&binding, after));
}

#[test]
fn daily_agent_args_run_one_skill_non_interactively() {
    let claude = daily_agent_args(RepoWikiAgent::Claude, "site", "wiki");
    assert_eq!(
        &claude[..3],
        ["-p", "/wenlan wiki space:site", "--allowedTools"]
    );
    assert!(claude.iter().any(|arg| arg == "Bash(git log:*)"));
    assert!(claude
        .iter()
        .any(|arg| arg == "mcp__plugin_wenlan_wenlan__*"));
    assert!(!claude
        .iter()
        .any(|arg| arg == "Bash" || arg == "Write" || arg == "Edit"));
    let codex = daily_agent_args(RepoWikiAgent::Codex, "site", "steep");
    assert_eq!(
        codex,
        [
            "exec",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "$wenlan steep space:site"
        ]
    );
    // The other tools write only through Wenlan: no --force, no write tool,
    // no approval beyond Wenlan's own tools.
    let skill = "Use the wenlan skill to run Wenlan: /wenlan wiki space:site";
    let cursor = daily_agent_args(RepoWikiAgent::Cursor, "site", "wiki");
    assert_eq!(&cursor[..2], ["-p", skill]);
    assert!(!cursor.iter().any(|arg| arg == "--force" || arg == "--yolo"));
    let copilot = daily_agent_args(RepoWikiAgent::Copilot, "site", "wiki");
    assert_eq!(&copilot[..2], ["-p", skill]);
    assert!(copilot.contains(&"--no-ask-user".to_owned()));
    let allowed = copilot
        .iter()
        .find(|arg| arg.starts_with("--allow-tool="))
        .unwrap();
    assert!(
        allowed.contains("wenlan") && !allowed.contains("write"),
        "{allowed}"
    );
}

#[test]
fn only_tools_that_read_agent_skills_need_wenlans_skill_folder() {
    let home = Path::new("/home/someone");
    assert!(wenlan_skill_paths(RepoWikiAgent::Claude, home).is_empty());
    assert!(wenlan_skill_paths(RepoWikiAgent::Codex, home).is_empty());
    assert_eq!(
        wenlan_skill_paths(RepoWikiAgent::Copilot, home),
        [home.join(".copilot/skills/wenlan/SKILL.md")]
    );
}

fn readme_fix(old: &str, new: &str) -> DocFixRequest {
    DocFixRequest {
        path: "README.md".into(),
        old_text: old.into(),
        new_text: new.into(),
        reason: "package.json now requires Node 20.".into(),
        code: vec![RepoDocCode {
            path: "package.json".into(),
            line_start: 5,
            line_end: 5,
        }],
        page: None,
    }
}

/// A fix to README.md from the followed page `page_positioning`.
fn page_fix(old: &str, new: &str, quote: &str) -> DocFixRequest {
    DocFixRequest {
        path: "README.md".into(),
        old_text: old.into(),
        new_text: new.into(),
        reason: "The positioning page now says Node 20.".into(),
        code: Vec::new(),
        page: Some(RepoDocPageLine {
            page_id: "page_positioning".into(),
            version: 3,
            quote: quote.into(),
        }),
    }
}

#[test]
fn docs_follow_a_page_only_when_told_and_take_fixes_from_it() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_readme(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    bind_at(&store, "site", &clone, None, None).unwrap();

    // Not followed yet: a fix from the page is refused.
    let refused = suggest_doc_fix_at(
        &store,
        "site",
        page_fix(
            "Node 18 or later",
            "Node 20 or later",
            "  We support Node 20. ",
        ),
        1,
    )
    .unwrap_err()
    .to_string();
    assert!(refused.contains("does not follow page"), "{refused}");

    // Only the repository's own docs, present in the working copy, can follow.
    let follow = |files: &[&str]| {
        follow_page_at(
            &store,
            "site",
            "page_positioning",
            files.iter().map(|file| (*file).to_owned()).collect(),
            4,
        )
    };
    for (files, says) in [
        (&["CHANGELOG.md"][..], "not one of the repository's docs"),
        (&["wenlan/index.md"][..], "not one of the repository's docs"),
        (&["docs/missing.md"][..], "is not in"),
        (&["../README.md"][..], "leaves the repository"),
        (&[][..], "name 1 to"),
    ] {
        let error = follow(files).unwrap_err().to_string();
        assert!(error.contains(says), "{files:?}: {error}");
    }
    let made = follow(&["README.md", "./README.md"]).unwrap();
    assert_eq!(made.files, ["README.md"]);
    assert_eq!(made.followed_version, None);
    assert!(follow_page_at(
        &store,
        "nope",
        "page_positioning",
        vec!["README.md".into()],
        4
    )
    .unwrap_err()
    .to_string()
    .contains("not connected"));

    // Code and a page together, or an empty quote, are refused.
    let mut both = page_fix(
        "Node 18 or later",
        "Node 20 or later",
        "We support Node 20.",
    );
    both.code = readme_fix("x", "y").code;
    assert!(suggest_doc_fix_at(&store, "site", both, 1)
        .unwrap_err()
        .to_string()
        .contains("not both"));
    assert!(suggest_doc_fix_at(
        &store,
        "site",
        page_fix("Node 18 or later", "Node 20 or later", "   "),
        1
    )
    .unwrap_err()
    .to_string()
    .contains("page.quote"));

    let (fix, open) = suggest_doc_fix_at(
        &store,
        "site",
        page_fix(
            "Node 18 or later",
            "Node 20 or later",
            "  We support Node 20. ",
        ),
        5,
    )
    .unwrap();
    assert_eq!(open, 1);
    let line = fix.page.clone().unwrap();
    assert_eq!(line.quote, "We support Node 20.", "quote is trimmed");
    assert!(fix.code.is_empty());

    // The followed version moves only when an AI tool says it caught up.
    assert_eq!(
        mark_followed_at(&store, "site", "page_positioning", 3)
            .unwrap()
            .followed_version,
        Some(3)
    );
    // Following again changes the files but keeps the version.
    assert_eq!(follow(&["README.md"]).unwrap().followed_version, Some(3));
    assert!(mark_followed_at(&store, "site", "page_other", 1)
        .unwrap_err()
        .to_string()
        .contains("no docs follow"));

    // The pull request names the page's line the fix rests on.
    let worktree = dir.path().join("data/repo-wiki/site");
    let binding = binding_at(&store, "site").unwrap().unwrap();
    let code_bases = HashMap::new();
    let anchors = CodeAnchors::new();
    let concept_paths = ConceptPathMap::new();
    let published = publish(publish_input(
        &binding,
        vec![],
        &code_bases,
        &anchors,
        &concept_paths,
        &worktree,
    ))
    .unwrap();
    assert_eq!(published.result.doc_fixes, 1);
    let readme = sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:README.md")]);
    assert!(readme.contains("Requires Node 20 or later."), "{readme}");

    // Unfollowing keeps the fix already found; the binding forgets the page.
    assert!(unfollow_page_at(&store, "site", "page_positioning").unwrap());
    assert!(!unfollow_page_at(&store, "site", "page_positioning").unwrap());
    let binding = binding_at(&store, "site").unwrap().unwrap();
    assert!(binding.follows.is_empty());
    assert_eq!(binding.doc_fixes.len(), 1);
}

#[test]
fn a_pull_request_names_the_line_of_the_page_a_fix_follows() {
    let binding = RepoWikiBinding {
        space: "site".into(),
        repo_root: "/r".into(),
        folder: "wiki".into(),
        remote_url: None,
        daily: None,
        last_publish: None,
        last_daily_run: None,
        doc_fixes: Vec::new(),
        doc_check: None,
        kept_edits: Vec::new(),
        follows: Vec::new(),
        take_over: false,
    };
    let fix = RepoDocFix {
        id: "fix_1".into(),
        path: "README.md".into(),
        old_text: "a".into(),
        new_text: "b".into(),
        reason: "The page now calls it a studio.".into(),
        code: Vec::new(),
        page: Some(RepoDocPageLine {
            page_id: "page_positioning".into(),
            version: 2,
            quote: "A studio for \"everything\"\n you think".into(),
        }),
        state: RepoDocFixState::Pending,
        found_at: 1,
    };
    let body = pr_body_for_test(&binding, vec![&fix]);
    assert!(
        body.contains("no longer matches the page the doc follows"),
        "{body}"
    );
    assert!(
        body.contains("(the page it follows says: \"A studio for 'everything' you think\")"),
        "{body}"
    );
    assert!(
        !body.contains("page_positioning"),
        "no page id in public text: {body}"
    );
}

#[test]
fn a_quote_counts_when_only_white_space_differs() {
    let page = "# Positioning\n\n- Banner:   A studio for everything\n  you think.\n";
    assert!(page_has_quote(page, "A studio for everything you think."));
    assert!(page_has_quote(page, "  Banner: A studio "));
    assert!(!page_has_quote(page, "A studio for nothing"));
    assert!(!page_has_quote(page, "   "));
}

/// `repo_with_origin` plus a README pushed to `main`.
fn repo_with_readme(root: &Path) -> (PathBuf, PathBuf) {
    let (origin, clone) = repo_with_origin(root);
    std::fs::write(
        clone.join("README.md"),
        "# Site\n\nRequires Node 18 or later.\n\nRun `pnpm dev`.\n",
    )
    .unwrap();
    std::fs::write(clone.join("CHANGELOG.md"), "## 1.0\n\nNode 18.\n").unwrap();
    sh(&clone, &["add", "."]);
    sh(&clone, &["commit", "--quiet", "-m", "readme"]);
    sh(&clone, &["push", "--quiet", "origin", "main"]);
    (origin, clone)
}

fn publish_input<'a>(
    binding: &'a RepoWikiBinding,
    pages: Vec<Page>,
    code_bases: &'a HashMap<String, Vec<CodeBasisLocator>>,
    anchors: &'a CodeAnchors,
    concept_paths: &'a ConceptPathMap,
    worktree: &Path,
) -> PublishInput<'a> {
    PublishInput {
        rules: Some(Vec::new()),
        publisher: "me".to_string(),
        binding,
        pages,
        left_out: 0,
        code_bases,
        anchors,
        concept_paths,
        worktree: worktree.to_path_buf(),
        gh: None,
        now: 1,
        prepared: None,
        taken_back: BTreeSet::new(),
        took_back: 0,
    }
}

#[test]
fn doc_fixes_are_checked_against_the_file_before_they_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clone) = repo_with_readme(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    bind_at(&store, "site", &clone, Some("wiki"), None).unwrap();

    let refused = |request: DocFixRequest| {
        suggest_doc_fix_at(&store, "site", request, 1)
            .unwrap_err()
            .to_string()
    };
    let mut wiki = readme_fix("x", "y");
    wiki.path = "wiki/pages/a.md".into();
    assert!(refused(wiki).contains("not one of the repository's docs"));
    let mut changelog = readme_fix("Node 18.", "Node 20.");
    changelog.path = "CHANGELOG.md".into();
    assert!(refused(changelog).contains("not one of the repository's docs"));
    let mut outside = readme_fix("x", "y");
    outside.path = "../README.md".into();
    assert!(refused(outside).contains("leaves the repository"));
    assert!(refused(readme_fix("Node 16", "Node 20")).contains("not in README.md"));
    assert!(refused(readme_fix("R", "r")).contains("occurs 2 times"));
    assert!(
        suggest_doc_fix_at(&store, "nope", readme_fix("Node 18", "Node 20"), 1)
            .unwrap_err()
            .to_string()
            .contains("not connected")
    );

    let (fix, open) = suggest_doc_fix_at(
        &store,
        "site",
        readme_fix("Node 18 or later", "Node 20 or later"),
        5,
    )
    .unwrap();
    assert_eq!(open, 1);
    assert_eq!(fix.state, RepoDocFixState::Pending);
    assert_eq!(fix.id, doc_fix_id("README.md", "Node 18 or later"));
    // The same text again replaces the fix rather than adding one.
    let (_, open) = suggest_doc_fix_at(
        &store,
        "site",
        readme_fix("Node 18 or later", "Node 20+"),
        6,
    )
    .unwrap();
    assert_eq!(open, 1);
    let binding = binding_at(&store, "site").unwrap().unwrap();
    assert_eq!(binding.doc_fixes[0].new_text, "Node 20+");

    // Declined: not offered again.
    let (_, open) = dismiss_doc_fix_at(&store, "site", &fix.id).unwrap();
    assert_eq!(open, 0);
    assert!(refused(readme_fix("Node 18 or later", "Node 20 or later")).contains("declined"));

    assert_eq!(record_doc_check_at(&store, "site", 7, 9).unwrap(), 0);
    let binding = binding_at(&store, "site").unwrap().unwrap();
    assert_eq!(binding.doc_check, Some(RepoDocCheck { at: 9, docs: 7 }));
}

#[test]
fn wenlans_own_block_takes_no_doc_fix() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clone) = repo_with_origin(dir.path());
    std::fs::write(
        clone.join("AGENTS.md"),
        upsert_agents_block("# Agents\n\nUse pnpm.\n", "wiki"),
    )
    .unwrap();
    let store = dir.path().join("data/repo-wiki.json");
    bind_at(&store, "site", &clone, Some("wiki"), None).unwrap();
    let mut inside = readme_fix("Start at `wiki/index.md`", "Start anywhere");
    inside.path = "AGENTS.md".into();
    assert!(suggest_doc_fix_at(&store, "site", inside, 1)
        .unwrap_err()
        .to_string()
        .contains("Wenlan's own block"));
    let mut outside = readme_fix("Use pnpm.", "Use npm.");
    outside.path = "AGENTS.md".into();
    suggest_doc_fix_at(&store, "site", outside, 1).unwrap();
}

#[test]
fn doc_fixes_go_into_the_wiki_pull_request_and_leave_once_merged() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_readme(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    bind_at(&store, "site", &clone, Some("wiki"), None).unwrap();
    suggest_doc_fix_at(
        &store,
        "site",
        readme_fix("Node 18 or later", "Node 20 or later"),
        1,
    )
    .unwrap();
    let mut other = readme_fix("Run `pnpm dev`.", "Run `pnpm start`.");
    other.reason = "The dev script was renamed.".into();
    let (declined, _) = suggest_doc_fix_at(&store, "site", other, 1).unwrap();
    dismiss_doc_fix_at(&store, "site", &declined.id).unwrap();
    let worktree = dir.path().join("data/repo-wiki/site");
    let code_bases = HashMap::new();
    let anchors = CodeAnchors::new();
    let concept_paths = ConceptPathMap::new();
    let publish_now = || {
        let binding = binding_at(&store, "site").unwrap().unwrap();
        let published = publish(publish_input(
            &binding,
            vec![],
            &code_bases,
            &anchors,
            &concept_paths,
            &worktree,
        ))
        .unwrap();
        settle_doc_fixes_at(&store, "site", &published.applied, &published.gone).unwrap();
        published
    };

    // No wiki pages: the fix alone opens the pull request, and nothing is
    // written into the wiki folder or AGENTS.md.
    let first = publish_now();
    assert_eq!(first.result.status, RepoWikiPublishStatus::Pushed);
    assert_eq!((first.result.pages, first.result.doc_fixes), (0, 1));
    let readme = sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:README.md")]);
    assert!(readme.contains("Requires Node 20 or later."), "{readme}");
    assert!(
        readme.contains("Run `pnpm dev`."),
        "declined fix stays out: {readme}"
    );
    let files = sh(&origin, &["ls-tree", "-r", "--name-only", REPO_WIKI_BRANCH]);
    assert!(
        !files.contains("wiki/") && !files.contains("AGENTS.md"),
        "{files}"
    );
    let binding = binding_at(&store, "site").unwrap().unwrap();
    let states: Vec<_> = binding.doc_fixes.iter().map(|fix| fix.state).collect();
    assert_eq!(states, [RepoDocFixState::InPr, RepoDocFixState::Dismissed]);

    // Merged: both old texts are gone or overtaken, so both fixes leave.
    sh(&clone, &["fetch", "--quiet", "origin"]);
    sh(
        &clone,
        &[
            "merge",
            "--quiet",
            "--ff-only",
            &format!("origin/{REPO_WIKI_BRANCH}"),
        ],
    );
    std::fs::write(
        clone.join("README.md"),
        "# Site\n\nRequires Node 20 or later.\n\nRun `pnpm serve`.\n",
    )
    .unwrap();
    sh(&clone, &["commit", "--quiet", "-am", "serve"]);
    sh(&clone, &["push", "--quiet", "origin", "main"]);
    let second = publish_now();
    assert_eq!(second.gone.len(), 2);
    assert!(binding_at(&store, "site")
        .unwrap()
        .unwrap()
        .doc_fixes
        .is_empty());
}

#[test]
fn a_page_file_edited_in_the_repository_is_found_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_origin(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    let binding = bind_at(&store, "site", &clone, Some("wiki"), None).unwrap();
    let root = binding.repo_root.clone();
    let code_bases = HashMap::from([(
        "page_map".to_string(),
        vec![locator(&root, "src/lib.rs", 1, 2)],
    )]);
    let anchors = anchors_for(
        &code_bases,
        &[("src/lib.rs", "fn a() {}\nfn b() {}\nfn c() {}\n")],
    );
    let concept_paths = ConceptPathMap::new();
    let worktree = dir.path().join("data/repo-wiki/site");
    let pages = vec![page("page_map", "Module map", "What each part owns.")];
    let first = publish(publish_input(
        &binding,
        pages.clone(),
        &code_bases,
        &anchors,
        &concept_paths,
        &worktree,
    ))
    .unwrap();
    assert_eq!(first.result.status, RepoWikiPublishStatus::Pushed);
    let files = sh(&origin, &["ls-tree", "-r", "--name-only", REPO_WIKI_BRANCH]);
    let page_file = files
        .lines()
        .find(|f| f.starts_with("wiki/pages/") && f.ends_with(".md"))
        .unwrap()
        .to_owned();

    // Merged, then someone edits the page on the default branch.
    sh(&clone, &["fetch", "--quiet", "origin"]);
    sh(
        &clone,
        &[
            "merge",
            "--quiet",
            "--ff-only",
            &format!("origin/{REPO_WIKI_BRANCH}"),
        ],
    );
    let text = std::fs::read_to_string(clone.join(&page_file)).unwrap();
    std::fs::write(
        clone.join(&page_file),
        text.replace("What each part owns.", "What each part owns, by hand."),
    )
    .unwrap();
    sh(&clone, &["commit", "--quiet", "-am", "edit the wiki"]);
    sh(&clone, &["push", "--quiet", "origin", "main"]);

    let prepared = prepare(&binding, &worktree).unwrap();
    assert_eq!(prepared.edits.len(), 1);
    let edit = &prepared.edits[0];
    assert_eq!(
        (
            edit.page_id.as_str(),
            edit.path.as_str(),
            edit.origin_version
        ),
        ("page_map", page_file.as_str(), 1)
    );
    assert_eq!(edit.body, "What each part owns, by hand.");

    // Not taken back: the edit stays and nothing is offered.
    let kept = publish(PublishInput {
        rules: Some(Vec::new()),
        publisher: "me".to_string(),
        prepared: Some(prepared),
        ..publish_input(
            &binding,
            pages.clone(),
            &code_bases,
            &anchors,
            &concept_paths,
            &worktree,
        )
    })
    .unwrap();
    assert_eq!(kept.result.status, RepoWikiPublishStatus::NoChanges);
    assert_eq!(kept.result.kept, 1);

    // Still waiting: the next publish finds the same edit and counts it again,
    // so the panel keeps saying a page waits for review.
    let mut seen = binding.clone();
    seen.kept_edits = kept.kept_edits.clone().unwrap();
    let again = publish(PublishInput {
        rules: Some(Vec::new()),
        publisher: "me".to_string(),
        prepared: Some(prepare(&seen, &worktree).unwrap()),
        ..publish_input(
            &seen,
            pages.clone(),
            &code_bases,
            &anchors,
            &concept_paths,
            &worktree,
        )
    })
    .unwrap();
    assert_eq!(again.result.status, RepoWikiPublishStatus::NoChanges);
    assert_eq!(again.result.kept, 1);
    assert_eq!(again.kept_edits, kept.kept_edits);

    // Taken back: the page now says it, and its file is written from it.
    let prepared = prepare(&binding, &worktree).unwrap();
    let mut taken = pages.clone();
    taken[0].content = prepared.edits[0].body.clone();
    taken[0].version = 2;
    let back = publish(PublishInput {
        rules: Some(Vec::new()),
        publisher: "me".to_string(),
        taken_back: BTreeSet::from([page_file.clone()]),
        took_back: 1,
        prepared: Some(prepared),
        ..publish_input(
            &binding,
            taken,
            &code_bases,
            &anchors,
            &concept_paths,
            &worktree,
        )
    })
    .unwrap();
    // The default branch already reads the same: only the page's version
    // stamp would change, which is no reason for a pull request.
    assert_eq!(back.result.status, RepoWikiPublishStatus::NoChanges);
    assert_eq!((back.result.taken_back, back.result.kept), (1, 0));
    let body = std::fs::read_to_string(worktree.join(&page_file)).unwrap();
    assert!(body.contains("origin_version: 2"), "{body}");
    assert!(body.contains("by hand."), "{body}");
    assert!(!files.contains(".wenlan-archive"));
    let files = sh(&origin, &["ls-tree", "-r", "--name-only", REPO_WIKI_BRANCH]);
    assert!(!files.contains(".wenlan-archive"), "{files}");
}

#[test]
fn a_merged_refresh_is_told_apart_from_a_persons_edit() {
    let dir = tempfile::tempdir().unwrap();
    let (_origin, clone) = repo_with_origin(dir.path());
    sh(&clone, &["config", "user.name", "Test"]);
    sh(&clone, &["config", "user.email", "test@example.com"]);
    let store = dir.path().join("data/repo-wiki.json");
    let binding = bind_at(&store, "site", &clone, Some("wiki"), None).unwrap();
    let root = binding.repo_root.clone();
    let code_bases = HashMap::from([
        ("p1".to_string(), vec![locator(&root, "src/lib.rs", 1, 2)]),
        ("p2".to_string(), vec![locator(&root, "src/lib.rs", 3, 3)]),
    ]);
    let anchors = anchors_for(
        &code_bases,
        &[("src/lib.rs", "fn a() {}\nfn b() {}\nfn c() {}\n")],
    );
    let concept_paths = ConceptPathMap::new();
    let worktree = dir.path().join("data/repo-wiki/site");
    let pages = vec![
        page("p1", "Module map", "What each part owns."),
        page("p2", "Third", "`c` does nothing."),
    ];
    let first = publish(publish_input(
        &binding,
        pages,
        &code_bases,
        &anchors,
        &concept_paths,
        &worktree,
    ))
    .unwrap();
    assert_eq!(first.result.status, RepoWikiPublishStatus::Pushed);
    sh(&clone, &["fetch", "--quiet", "origin"]);
    sh(
        &clone,
        &[
            "merge",
            "--quiet",
            "--ff-only",
            &format!("origin/{REPO_WIKI_BRANCH}"),
        ],
    );
    let file_of = |id: &str| {
        sh(&clone, &["ls-files", "wiki/pages"])
            .lines()
            .find(|file| {
                std::fs::read_to_string(clone.join(file))
                    .unwrap()
                    .contains(&format!("origin_id: {id}\n"))
            })
            .unwrap()
            .to_owned()
    };
    let (map, third) = (file_of("p1"), file_of("p2"));
    let replace = |file: &str, from: &str, to: &str| {
        let text = std::fs::read_to_string(clone.join(file)).unwrap();
        assert!(text.contains(from), "{text}");
        std::fs::write(clone.join(file), text.replace(from, to)).unwrap();
    };

    // `wenlan wiki refresh` rewrites p1 for changed code; someone edits p2.
    replace(&map, "What each part owns.", "What each part owns now.");
    crate::repo_wiki_ci::reanchor(
        &clone,
        "wiki",
        &[map.strip_prefix("wiki/").unwrap().to_owned()],
        None,
    )
    .unwrap();
    replace(&third, "`c` does nothing.", "`c` still does nothing.");
    sh(&clone, &["add", "-A"]);
    sh(&clone, &["commit", "--quiet", "-m", "refresh and an edit"]);
    sh(&clone, &["push", "--quiet", "origin", "main"]);

    let mut edits = prepare(&binding, &worktree).unwrap().edits;
    edits.sort_by(|a, b| a.page_id.cmp(&b.page_id));
    assert_eq!(edits.len(), 2, "{edits:?}");
    assert_eq!(
        (edits[0].page_id.as_str(), edits[0].by_refresh),
        ("p1", true)
    );
    assert_eq!(edits[0].spans, vec![("src/lib.rs".to_owned(), 1, 2)]);
    assert_eq!(
        (edits[1].page_id.as_str(), edits[1].by_refresh),
        ("p2", false)
    );
    assert_eq!(edits[1].body, "`c` still does nothing.");
}

#[test]
fn edits_people_push_to_the_wiki_branch_survive_the_next_publish() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_readme(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    let binding = bind_at(&store, "site", &clone, Some("wiki"), None).unwrap();
    let root = binding.repo_root.clone();
    suggest_doc_fix_at(
        &store,
        "site",
        readme_fix("Node 18 or later", "Node 20 or later"),
        1,
    )
    .unwrap();
    let binding = binding_at(&store, "site").unwrap().unwrap();
    let code_bases = HashMap::from([(
        "page_map".to_string(),
        vec![locator(&root, "src/lib.rs", 1, 2)],
    )]);
    let anchors = anchors_for(
        &code_bases,
        &[("src/lib.rs", "fn a() {}\nfn b() {}\nfn c() {}\n")],
    );
    let concept_paths = ConceptPathMap::new();
    let worktree = dir.path().join("data/repo-wiki/site");
    let pages = vec![page("page_map", "Module map", "What each part owns.")];
    publish(publish_input(
        &binding,
        pages.clone(),
        &code_bases,
        &anchors,
        &concept_paths,
        &worktree,
    ))
    .unwrap();

    // A reviewer rewords the fix and the page on the pull request's branch.
    sh(&clone, &["fetch", "--quiet", "origin"]);
    sh(
        &clone,
        &[
            "checkout",
            "--quiet",
            "-b",
            "review",
            &format!("origin/{REPO_WIKI_BRANCH}"),
        ],
    );
    let readme = std::fs::read_to_string(clone.join("README.md")).unwrap();
    std::fs::write(
        clone.join("README.md"),
        readme.replace("Node 20 or later", "Node 20 (LTS)"),
    )
    .unwrap();
    let files = sh(&clone, &["ls-files", "wiki/pages"]);
    let page_file = files.lines().next().unwrap().to_owned();
    let text = std::fs::read_to_string(clone.join(&page_file)).unwrap();
    std::fs::write(
        clone.join(&page_file),
        text.replace("What each part owns.", "What each part owns, reviewed."),
    )
    .unwrap();
    sh(&clone, &["commit", "--quiet", "-am", "review"]);
    sh(
        &clone,
        &[
            "push",
            "--quiet",
            "origin",
            &format!("HEAD:{REPO_WIKI_BRANCH}"),
        ],
    );
    sh(&clone, &["checkout", "--quiet", "main"]);

    // Wenlan's next publish, with a changed page: both edits stay.
    let prepared = prepare(&binding, &worktree).unwrap();
    assert_eq!(prepared.edits.len(), 1);
    assert_eq!(prepared.edits[0].body, "What each part owns, reviewed.");
    let mut changed = pages.clone();
    changed[0].title = "Module map".into();
    let next = publish(PublishInput {
        rules: Some(Vec::new()),
        publisher: "me".to_string(),
        prepared: Some(prepared),
        ..publish_input(
            &binding,
            changed,
            &code_bases,
            &anchors,
            &concept_paths,
            &worktree,
        )
    })
    .unwrap();
    assert_eq!(next.result.kept, 1);
    let readme = sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:README.md")]);
    assert!(readme.contains("Node 20 (LTS)"), "{readme}");
    let body = sh(
        &origin,
        &["show", &format!("{REPO_WIKI_BRANCH}:{page_file}")],
    );
    assert!(body.contains("reviewed."), "{body}");
}

#[test]
fn a_page_file_reads_back_as_page_content() {
    let pages = vec![
        page("page_a", "Module map", "x"),
        page("page_b", "How to run it", "y"),
    ];
    let text = "---\ntitle: \"Module map\"\norigin_id: page_a\norigin_version: 3\n---\n\n\
                See [How to run it](how-to-run-it.md) and [the runner](how-to-run-it.md), \
                not [this](https://example.com).\n\n## Code\n\n- [`src/lib.rs` lines 1-2](../../src/lib.rs#L1-L2)\n";
    let (id, version, body) = parse_wiki_page_file(text).unwrap();
    assert_eq!((id.as_str(), version), ("page_a", 3));
    assert!(!body.contains("## Code"), "{body}");
    assert_eq!(
        crate::export::okf::wikilinks_from_repository_body(&body, &pages),
        "See [[How to run it]] and [[How to run it|the runner]], not [this](https://example.com)."
    );
    // A `## Code` heading a person wrote with prose under it stays.
    let own =
        "---\norigin_id: page_a\norigin_version: 1\n---\n\nIntro.\n\n## Code\n\nWe like Rust.\n";
    assert!(parse_wiki_page_file(own)
        .unwrap()
        .2
        .ends_with("We like Rust."));
    assert!(parse_wiki_page_file("no frontmatter").is_none());
}

#[test]
fn publish_refuses_an_origin_that_changed_since_bind() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_origin(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    let binding = bind_at(&store, "site", &clone, Some("wiki"), None).unwrap();
    let root = binding.repo_root.clone();
    let code_bases = HashMap::from([("p1".to_string(), vec![locator(&root, "src/lib.rs", 1, 2)])]);
    let (pages, _) = select_pages(
        vec![page("p1", "Module map", "What each part owns.")],
        &code_bases,
        &root,
    );
    let anchors = anchors_for(
        &code_bases,
        &[("src/lib.rs", "fn a() {}\nfn b() {}\nfn c() {}\n")],
    );
    let concept_paths = ConceptPathMap::new();
    let worktree = dir.path().join("data/repo-wiki/site");
    // Someone points `origin` at another repository after agreeing to this one.
    let other = dir.path().join("other.git");
    sh(
        dir.path(),
        &[
            "clone",
            "--quiet",
            "--bare",
            &origin.to_string_lossy(),
            &other.to_string_lossy(),
        ],
    );
    sh(
        &clone,
        &["remote", "set-url", "origin", &other.to_string_lossy()],
    );
    let Err(error) = publish(publish_input(
        &binding,
        pages,
        &code_bases,
        &anchors,
        &concept_paths,
        &worktree,
    )) else {
        panic!("a changed origin is refused");
    };
    let error = error.to_string();
    assert!(error.contains("changed since it was bound"), "{error}");
    let branches = sh(&other, &["branch", "--list", REPO_WIKI_BRANCH]);
    assert!(branches.is_empty(), "nothing was pushed: {branches}");
}

#[test]
fn same_remote_matches_a_repository_over_https_and_ssh() {
    assert!(same_remote(
        "https://github.com/pallets/itsdangerous.git",
        "git@github.com:Pallets/itsdangerous"
    ));
    assert!(!same_remote(
        "https://github.com/pallets/itsdangerous",
        "https://github.com/lucian/itsdangerous"
    ));
    assert!(same_remote("/srv/site.git", "/srv/site.git"));
    assert!(!same_remote("/srv/site.git", "/srv/other.git"));
}

#[test]
fn publish_target_names_the_repository_and_its_owner() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clone) = repo_with_origin(dir.path());

    let local = publish_target(&clone, None);
    assert!(local.slug.is_none(), "a local remote is not on GitHub");
    assert_eq!(local.name(), local.remote_url.as_deref());
    assert!(local.confirmed_by(local.remote_url.as_deref().unwrap()));

    sh(
        &clone,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:pallets/itsdangerous.git",
        ],
    );
    let theirs = publish_target(&clone, None);
    assert_eq!(theirs.slug.as_deref(), Some("pallets/itsdangerous"));
    assert_eq!(theirs.owner(), Some("pallets"));
    assert!(!theirs.owned(), "an unknown account is not the owner");
    assert!(theirs.confirmed_by("pallets/itsdangerous"));
    assert!(theirs.confirmed_by("github.com/pallets/itsdangerous"));
    assert!(theirs.confirmed_by("https://github.com/Pallets/itsdangerous.git"));
    assert!(!theirs.confirmed_by("lucian/itsdangerous"));

    let signed_in = |login: &str| PublishTarget {
        signed_in_as: Some(login.to_owned()),
        ..theirs.clone()
    };
    assert!(!signed_in("lucian").owned());
    assert!(signed_in("Pallets").owned());
    assert!(
        !signed_in("lucian").ours(),
        "a project only cloned is not ours"
    );
    assert!(signed_in("Pallets").ours());
    // A teammate's account may push to the organization's repository.
    let teammate = PublishTarget {
        can_push: true,
        ..signed_in("lucian")
    };
    assert!(!teammate.owned());
    assert!(teammate.ours());
}

#[test]
fn a_repository_that_already_holds_pages_under_wiki_keeps_that_folder() {
    let dir = tempfile::tempdir().unwrap();
    let top = dir.path();
    assert_eq!(default_folder(top), "wenlan");
    // A hand-written `wiki/` folder is not Wenlan's.
    std::fs::create_dir_all(top.join("wiki")).unwrap();
    assert_eq!(default_folder(top), "wenlan");
    std::fs::write(top.join("wiki").join(crate::repo_wiki_ci::CODE_FILE), "{}").unwrap();
    assert_eq!(default_folder(top), "wiki");
    // Once pages are under `wenlan/`, that is the folder.
    std::fs::create_dir_all(top.join("wenlan")).unwrap();
    std::fs::write(
        top.join("wenlan").join(crate::repo_wiki_ci::CODE_FILE),
        "{}",
    )
    .unwrap();
    assert_eq!(default_folder(top), "wenlan");
}

#[test]
fn ux_skill_7_project_rules_reach_agents_md_without_any_wiki_page() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_origin(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    let binding = bind_at(&store, "site", &clone, None, None).unwrap();
    let published = publish(PublishInput {
        rules: Some(vec!["Run the affected tests before a push.".to_string()]),
        publisher: "me".to_string(),
        binding: &binding,
        pages: vec![],
        left_out: 0,
        code_bases: &HashMap::new(),
        anchors: &CodeAnchors::new(),
        concept_paths: &ConceptPathMap::new(),
        worktree: dir.path().join("data/repo-wiki/site"),
        gh: None,
        now: 1,
        prepared: None,
        taken_back: BTreeSet::new(),
        took_back: 0,
    })
    .unwrap();
    assert_eq!(published.result.status, RepoWikiPublishStatus::Pushed);
    assert_eq!(published.result.pages, 0);
    let agents = sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:AGENTS.md")]);
    assert!(
        agents.contains(
            "<!-- WENLAN:START -->\n<!-- wenlan-publisher: me -->\n## Project rules\n\n- Run the affected tests before a push.\n\nFor a rule's source or reason, use `read_space_rules` if Wenlan is connected to this project's Space.\n<!-- WENLAN:END -->"
        ),
        "{agents}"
    );
    // No folder of pages, and nothing that points at one.
    assert!(!agents.contains("## Wiki"), "{agents}");
    let files = sh(&origin, &["ls-tree", "-r", "--name-only", REPO_WIKI_BRANCH]);
    assert!(!files.contains("wenlan/"), "{files}");
    assert!(!files.contains(".gitattributes"), "{files}");
}

/// A rule taken away in Wenlan leaves the repository's `AGENTS.md` at the
/// next wiki pull request, the last one too: the block is taken out rather
/// than "nothing to publish" leaving the old rules on the default branch.
#[test]
fn ux_skill_7_a_removed_rule_leaves_agents_md_even_the_last_one() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_origin(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    bind_at(&store, "site", &clone, None, None).unwrap();
    let publish_rules = |rules: &[&str]| {
        let binding = binding_at(&store, "site").unwrap().unwrap();
        let published = publish(PublishInput {
            rules: Some(rules.iter().map(|rule| rule.to_string()).collect()),
            publisher: "me".to_string(),
            binding: &binding,
            pages: vec![],
            left_out: 0,
            code_bases: &HashMap::new(),
            anchors: &CodeAnchors::new(),
            concept_paths: &ConceptPathMap::new(),
            worktree: dir.path().join("data/repo-wiki/site"),
            gh: None,
            now: 1,
            prepared: None,
            taken_back: BTreeSet::new(),
            took_back: 0,
        })
        .unwrap();
        record_publish_at(&store, "site", &published).unwrap();
        published.result.status
    };
    let merge = || {
        sh(&clone, &["fetch", "--quiet", "origin"]);
        sh(
            &clone,
            &[
                "merge",
                "--quiet",
                "--ff-only",
                &format!("origin/{REPO_WIKI_BRANCH}"),
            ],
        );
        sh(&clone, &["push", "--quiet", "origin", "main"]);
    };
    let on_branch = |file: &str| sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:{file}")]);

    assert_eq!(
        publish_rules(&["Rule A.", "Rule B."]),
        RepoWikiPublishStatus::Pushed
    );
    merge();

    // One of two taken away: only the other one is left.
    assert_eq!(publish_rules(&["Rule B."]), RepoWikiPublishStatus::Pushed);
    let agents = on_branch("AGENTS.md");
    assert!(!agents.contains("Rule A."), "{agents}");
    assert!(agents.contains("- Rule B."), "{agents}");
    merge();

    // The last one taken away: the pull request takes the block out.
    assert_eq!(publish_rules(&[]), RepoWikiPublishStatus::Pushed);
    let files = sh(&origin, &["ls-tree", "-r", "--name-only", REPO_WIKI_BRANCH]);
    assert!(!files.lines().any(|f| f == "AGENTS.md"), "{files}");
    let claude = on_branch("CLAUDE.md");
    assert_eq!(claude, "# Notes\n\nKeep it small.");
    merge();

    // Nothing of Wenlan's left in the repository: nothing to publish again.
    assert_eq!(publish_rules(&[]), RepoWikiPublishStatus::NothingToPublish);
}

/// Publish `rules` (and a page resting on `src/lib.rs`, when `with_page`)
/// for the Space "site", recording the publish as the server does.
fn publish_site(
    dir: &Path,
    store: &Path,
    rules: Option<&[&str]>,
    with_page: bool,
) -> RepoWikiPublishStatus {
    publish_site_with(dir, store, rules, with_page, None).status
}

fn publish_site_with(
    dir: &Path,
    store: &Path,
    rules: Option<&[&str]>,
    with_page: bool,
    gh: Option<PathBuf>,
) -> RepoWikiPublishResult {
    let binding = binding_at(store, "site").unwrap().unwrap();
    let code_bases = if with_page {
        HashMap::from([(
            "p1".to_string(),
            vec![locator(&binding.repo_root, "src/lib.rs", 1, 2)],
        )])
    } else {
        HashMap::new()
    };
    let anchors = anchors_for(
        &code_bases,
        &[("src/lib.rs", "fn a() {}\nfn b() {}\nfn c() {}\n")],
    );
    let published = publish(PublishInput {
        rules: rules.map(|rules| rules.iter().map(|rule| rule.to_string()).collect()),
        publisher: "me".to_string(),
        binding: &binding,
        pages: if with_page {
            vec![page("p1", "Page one", "Body.")]
        } else {
            vec![]
        },
        left_out: 0,
        code_bases: &code_bases,
        anchors: &anchors,
        concept_paths: &ConceptPathMap::new(),
        worktree: dir.join("data/repo-wiki/site"),
        gh,
        now: 1,
        prepared: None,
        taken_back: BTreeSet::new(),
        took_back: 0,
    })
    .unwrap();
    record_publish_at(store, "site", &published).unwrap();
    published.result
}

fn merge_wiki_branch(clone: &Path) {
    sh(clone, &["fetch", "--quiet", "origin"]);
    sh(
        clone,
        &[
            "merge",
            "--quiet",
            "--ff-only",
            &format!("origin/{REPO_WIKI_BRANCH}"),
        ],
    );
    sh(clone, &["push", "--quiet", "origin", "main"]);
}

/// Rules that could not be read are not "no rules": the repository keeps
/// the ones it has, with and without pages.
#[test]
fn ux_skill_7_rules_that_could_not_be_read_stay_in_agents_md() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_origin(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    bind_at(&store, "site", &clone, None, None).unwrap();
    assert_eq!(
        publish_site(dir.path(), &store, Some(&["Rule A.", "Rule B."]), false),
        RepoWikiPublishStatus::Pushed
    );
    merge_wiki_branch(&clone);

    // No pages: nothing to publish, and the rules stay where they are.
    assert_eq!(
        publish_site(dir.path(), &store, None, false),
        RepoWikiPublishStatus::NothingToPublish
    );
    let main = sh(&origin, &["show", "main:AGENTS.md"]);
    assert!(main.contains("- Rule A.\n- Rule B.\n"), "{main}");

    // With a page: the block gains the wiki pointer and keeps the rules.
    assert_eq!(
        publish_site(dir.path(), &store, None, true),
        RepoWikiPublishStatus::Pushed
    );
    let agents = sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:AGENTS.md")]);
    assert!(agents.contains("## Wiki"), "{agents}");
    assert!(
        agents.contains("## Project rules\n\n- Rule A.\n- Rule B.\n\nFor a rule's source or reason, use `read_space_rules` if Wenlan is connected to this project's Space.\n<!-- WENLAN:END -->"),
        "{agents}"
    );
}

/// The last rule taken away while the wiki pull request that added it is
/// still open: the pull request no longer carries it.
#[test]
fn ux_skill_7_a_removed_last_rule_leaves_the_open_pull_request() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_origin(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    bind_at(&store, "site", &clone, None, None).unwrap();
    assert_eq!(
        publish_site(dir.path(), &store, Some(&["Rule A."]), false),
        RepoWikiPublishStatus::Pushed
    );
    let files = sh(&origin, &["ls-tree", "-r", "--name-only", REPO_WIKI_BRANCH]);
    assert!(files.lines().any(|f| f == "AGENTS.md"), "{files}");

    assert_eq!(
        publish_site(dir.path(), &store, Some(&[]), false),
        RepoWikiPublishStatus::Pushed
    );
    let files = sh(&origin, &["ls-tree", "-r", "--name-only", REPO_WIKI_BRANCH]);
    assert!(!files.lines().any(|f| f == "AGENTS.md"), "{files}");
    let claude = sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:CLAUDE.md")]);
    assert_eq!(claude, "# Notes\n\nKeep it small.");
    let diff = sh(&origin, &["diff", "--stat", "main", REPO_WIKI_BRANCH]);
    assert!(diff.is_empty(), "{diff}");

    // Settled: nothing more to send.
    assert_eq!(
        publish_site(dir.path(), &store, Some(&[]), false),
        RepoWikiPublishStatus::NothingToPublish
    );
}

/// Make `clone`'s `origin` look like `github.com/o/r` while fetches and
/// pushes still reach the bare `origin` on disk.
fn github_like(root: &Path, clone: &Path, origin: &Path) {
    let ssh = root.join("fake-ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nfor a; do cmd=$a; done\n\
             cmd=$(printf %s \"$cmd\" | sed -e \"s|'/*o/r.git'|'{}'|\" -e 's|^git-|git |')\n\
             exec sh -c \"$cmd\"\n",
            origin.display()
        ),
    )
    .unwrap();
    make_executable(&ssh);
    sh(clone, &["config", "core.sshCommand", ssh.to_str().unwrap()]);
    sh(
        clone,
        &[
            "remote",
            "set-url",
            "origin",
            "ssh://git@github.com/o/r.git",
        ],
    );
    // A host may rewrite GitHub's ssh addresses to https; this longer match
    // wins and keeps the address as it is.
    sh(
        clone,
        &[
            "config",
            "url.ssh://git@github.com/o/r.git.insteadOf",
            "ssh://git@github.com/o/r.git",
        ],
    );
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_: &Path) {}

/// A stand-in `gh`: it logs each call, lists the URL in `state` as the open
/// pull request, `pr create` opens one and `pr close` closes it.
fn fake_gh(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let gh = root.join("fake-gh");
    let log = root.join("gh.log");
    let state = root.join("gh.state");
    std::fs::write(&state, "").unwrap();
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\nprintf '%s\\n' --- \"$@\" >> '{log}'\n\
             case \"$1 $2\" in\n\
             'pr list') cat '{state}' ;;\n\
             'pr create') echo https://github.com/o/r/pull/7; \
             echo https://github.com/o/r/pull/7 > '{state}' ;;\n\
             'pr close') : > '{state}' ;;\n\
             esac\n",
            log = log.display(),
            state = state.display()
        ),
    )
    .unwrap();
    make_executable(&gh);
    (gh, log, state)
}

fn remote_sha(origin: &Path, branch: &str) -> String {
    sh(origin, &["rev-parse", branch])
}

#[cfg(unix)]
#[test]
fn ux_skill_7_a_removed_last_rule_closes_the_open_pull_request() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_origin(dir.path());
    github_like(dir.path(), &clone, &origin);
    let (gh, log, _) = fake_gh(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    bind_at(&store, "site", &clone, None, None).unwrap();
    let opened = publish_site_with(
        dir.path(),
        &store,
        Some(&["Rule A."]),
        false,
        Some(gh.clone()),
    );
    assert_eq!(opened.status, RepoWikiPublishStatus::PrOpened);
    let before = remote_sha(&origin, REPO_WIKI_BRANCH);

    let closed = publish_site_with(dir.path(), &store, Some(&[]), false, Some(gh));
    assert_eq!(closed.status, RepoWikiPublishStatus::NoChanges);
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(
        calls.contains(
            "---\npr\nclose\nhttps://github.com/o/r/pull/7\n--repo\no/r\n--comment\n\
             Wenlan: every rule was removed, nothing left to change.\n--delete-branch\n"
        ),
        "{calls}"
    );
    assert_eq!(calls.matches("\ncreate\n").count(), 1, "{calls}");
    // Closed, not pushed to.
    assert_eq!(remote_sha(&origin, REPO_WIKI_BRANCH), before);
}

#[cfg(unix)]
#[test]
fn ux_skill_7_the_pull_request_taking_rules_out_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_origin(dir.path());
    github_like(dir.path(), &clone, &origin);
    let (gh, log, state) = fake_gh(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    bind_at(&store, "site", &clone, None, None).unwrap();
    let opened = publish_site_with(
        dir.path(),
        &store,
        Some(&["Rule A."]),
        false,
        Some(gh.clone()),
    );
    assert_eq!(opened.status, RepoWikiPublishStatus::PrOpened);
    merge_wiki_branch(&clone);
    std::fs::write(&state, "").unwrap();

    let removal = publish_site_with(dir.path(), &store, Some(&[]), false, Some(gh.clone()));
    assert_eq!(removal.status, RepoWikiPublishStatus::PrOpened);
    let calls = std::fs::read_to_string(&log).unwrap();
    let body = calls.rsplit("--body\n").next().unwrap();
    assert!(
        body.contains("Every project rule was removed in Wenlan, so this takes them out"),
        "{body}"
    );

    // Asked again while that pull request is open: nothing new to push.
    std::fs::write(&state, "https://github.com/o/r/pull/7\n").unwrap();
    assert_eq!(
        publish_site_with(dir.path(), &store, Some(&[]), false, Some(gh)).status,
        RepoWikiPublishStatus::NoChanges
    );
}

/// A wiki branch left behind after its pull request was closed is not an
/// open pull request: nothing is pushed to it and none is opened.
#[cfg(unix)]
#[test]
fn ux_skill_7_a_stale_wiki_branch_gets_no_new_pull_request() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_origin(dir.path());
    github_like(dir.path(), &clone, &origin);
    let (gh, log, state) = fake_gh(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    bind_at(&store, "site", &clone, None, None).unwrap();
    let opened = publish_site_with(
        dir.path(),
        &store,
        Some(&["Rule A."]),
        false,
        Some(gh.clone()),
    );
    assert_eq!(opened.status, RepoWikiPublishStatus::PrOpened);
    // The person closed it and kept the branch.
    std::fs::write(&state, "").unwrap();
    let before = remote_sha(&origin, REPO_WIKI_BRANCH);

    let again = publish_site_with(dir.path(), &store, Some(&[]), false, Some(gh));
    assert_eq!(again.status, RepoWikiPublishStatus::NothingToPublish);
    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(calls.matches("\ncreate\n").count(), 1, "{calls}");
    assert!(!calls.contains("\nclose\n"), "{calls}");
    assert_eq!(remote_sha(&origin, REPO_WIKI_BRANCH), before);
}

#[test]
fn ux_skill_7_taking_rules_out_keeps_the_persons_own_text() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_origin(dir.path());
    std::fs::write(clone.join("AGENTS.md"), "# Team\n\nOur own line.\n").unwrap();
    sh(&clone, &["add", "AGENTS.md"]);
    sh(&clone, &["commit", "--quiet", "-m", "agents"]);
    sh(&clone, &["push", "--quiet", "origin", "main"]);
    let store = dir.path().join("data/repo-wiki.json");
    bind_at(&store, "site", &clone, None, None).unwrap();
    assert_eq!(
        publish_site(dir.path(), &store, Some(&["Rule A."]), false),
        RepoWikiPublishStatus::Pushed
    );
    merge_wiki_branch(&clone);
    assert_eq!(
        publish_site(dir.path(), &store, Some(&[]), false),
        RepoWikiPublishStatus::Pushed
    );
    let agents = sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:AGENTS.md")]);
    assert_eq!(agents, "# Team\n\nOur own line.");
}

/// With nothing new, a block another Wenlan wrote, or one from before
/// publishers were named that this Wenlan never pushed, is left alone.
#[test]
fn ux_skill_7_nothing_new_leaves_another_wenlans_rules_alone() {
    for (block, pushed_before) in [
        ("<!-- WENLAN:START -->\n<!-- wenlan-publisher: someone-else -->\n## Rules for this project\n\n- Theirs.\n<!-- WENLAN:END -->\n", true),
        ("<!-- WENLAN:START -->\n## Rules for this project\n\n- Old.\n<!-- WENLAN:END -->\n", false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (origin, clone) = repo_with_origin(dir.path());
        let store = dir.path().join("data/repo-wiki.json");
        bind_at(&store, "site", &clone, None, None).unwrap();
        if pushed_before {
            // This Wenlan pushed once; its branch is gone since, and another
            // Wenlan keeps the repository now.
            assert_eq!(
                publish_site(dir.path(), &store, Some(&["Mine."]), false),
                RepoWikiPublishStatus::Pushed
            );
            sh(&origin, &["branch", "-D", REPO_WIKI_BRANCH]);
        }
        std::fs::write(clone.join("AGENTS.md"), block).unwrap();
        sh(&clone, &["add", "AGENTS.md"]);
        sh(&clone, &["commit", "--quiet", "-m", "agents"]);
        sh(&clone, &["push", "--quiet", "origin", "main"]);
        for _ in 0..2 {
            assert_eq!(
                publish_site(dir.path(), &store, Some(&[]), false),
                RepoWikiPublishStatus::NothingToPublish
            );
        }
        let branches = sh(&origin, &["branch", "--list"]);
        assert!(!branches.contains(REPO_WIKI_BRANCH), "{branches}");
    }
}

/// Unread rules in an open pull request that is not merged yet stay in it.
#[test]
fn ux_skill_7_unread_rules_stay_in_the_open_pull_request() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_origin(dir.path());
    let store = dir.path().join("data/repo-wiki.json");
    bind_at(&store, "site", &clone, None, None).unwrap();
    assert_eq!(
        publish_site(dir.path(), &store, Some(&["Rule A."]), false),
        RepoWikiPublishStatus::Pushed
    );
    assert_eq!(
        publish_site(dir.path(), &store, None, true),
        RepoWikiPublishStatus::Pushed
    );
    let agents = sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:AGENTS.md")]);
    assert!(agents.contains("## Wiki"), "{agents}");
    assert!(
        agents.contains("- Rule A.\n\nFor a rule's source or reason, use `read_space_rules` if Wenlan is connected to this project's Space.\n<!-- WENLAN:END -->"),
        "{agents}"
    );
}

/// A checkout that turns line endings into CRLF (Git for Windows) neither
/// hides the rules nor loses them.
#[test]
fn ux_skill_7_rules_survive_a_crlf_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_origin(dir.path());
    std::fs::write(clone.join("AGENTS.md"), "# Team\n\nOur own line.\n").unwrap();
    sh(&clone, &["add", "AGENTS.md"]);
    sh(&clone, &["commit", "--quiet", "-m", "agents"]);
    sh(&clone, &["push", "--quiet", "origin", "main"]);
    sh(&clone, &["config", "core.autocrlf", "true"]);
    let store = dir.path().join("data/repo-wiki.json");
    bind_at(&store, "site", &clone, None, None).unwrap();
    assert_eq!(
        publish_site(dir.path(), &store, Some(&["Rule A.", "Rule B."]), false),
        RepoWikiPublishStatus::Pushed
    );
    merge_wiki_branch(&clone);
    let checkout = std::fs::read_to_string(dir.path().join("data/repo-wiki/site/AGENTS.md"));
    assert!(checkout.unwrap().contains("\r\n"), "the checkout is CRLF");

    // Unread, with a page: both rules stay.
    assert_eq!(
        publish_site(dir.path(), &store, None, true),
        RepoWikiPublishStatus::Pushed
    );
    let agents = sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:AGENTS.md")]);
    assert!(
        agents.contains("- Rule A.\n- Rule B.\n\nFor a rule's source or reason, use `read_space_rules` if Wenlan is connected to this project's Space.\n<!-- WENLAN:END -->"),
        "{agents}"
    );
    assert!(!agents.contains('\r'), "{agents:?}");
    merge_wiki_branch(&clone);

    // Every rule removed: they leave, the pointer and the person's text stay.
    assert_eq!(
        publish_site(dir.path(), &store, Some(&[]), false),
        RepoWikiPublishStatus::Pushed
    );
    let agents = sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:AGENTS.md")]);
    assert!(
        agents.starts_with("# Team\n\nOur own line.\n\n"),
        "{agents}"
    );
    assert!(agents.contains("## Wiki"), "{agents}");
    assert!(!agents.contains("Rule A."), "{agents}");
}

#[test]
fn the_block_is_read_and_written_in_the_files_own_line_endings() {
    let crlf = "# Team\r\n\r\n<!-- WENLAN:START -->\r\n## Rules for this project\r\n\r\n- One.\r\n- Two.\r\n<!-- WENLAN:END -->\r\n\r\nAfter.\r\n";
    assert!(block_holds_rules(crlf));
    assert_eq!(block_rule_lines(crlf), vec!["One.", "Two."]);
    assert_eq!(remove_block(crlf), "# Team\r\n\r\nAfter.\r\n");
    let upserted = upsert_agents_rules_block("# Team\r\n", &["Three.".to_string()]);
    assert_eq!(
        upserted,
        "# Team\r\n\r\n<!-- WENLAN:START -->\r\n## Project rules\r\n\r\n- Three.\r\n\r\nFor a rule's source or reason, use `read_space_rules` if Wenlan is connected to this project's Space.\r\n<!-- WENLAN:END -->\r\n"
    );
    // A heading without the blank line after it still gives every rule.
    let tight =
        "<!-- WENLAN:START -->\n## Rules for this project\n- One.\n- Two.\n<!-- WENLAN:END -->";
    assert_eq!(block_rule_lines(tight), vec!["One.", "Two."]);
}

#[test]
fn ux_skill_7_rules_hint_sits_under_the_rules_only_when_there_are_rules() {
    let with = upsert_agents_block_with_rules("", "wiki", &["Use pnpm.".to_string()]);
    assert!(
        with.contains(&format!("- Use pnpm.\n\n{RULES_HINT}\n<!-- WENLAN:END -->")),
        "{with}"
    );
    assert_eq!(with.matches(RULES_HINT).count(), 1);
    // No rules: no heading and no hint, the wiki pointer alone.
    let without = upsert_agents_block("", "wiki");
    assert!(!without.contains(RULES_HINT), "{without}");
    assert!(!without.contains(RULES_HEADING), "{without}");
    assert!(!upsert_agents_rules_block("", &[]).contains(RULES_HINT));
}

#[test]
fn ux_skill_7_rules_hint_is_not_read_back_as_a_rule() {
    let rules = vec!["One.".to_string(), "Two.".to_string()];
    for text in [
        upsert_agents_rules_block("# Team\n", &rules),
        upsert_agents_rules_block("# Team\r\n", &rules),
        upsert_agents_block_with_rules("# Team\r\n", "wiki", &rules),
    ] {
        assert!(block_holds_rules(&text), "{text:?}");
        assert_eq!(block_rule_lines(&text), rules, "{text:?}");
        let removed = remove_block(&text);
        assert!(!removed.contains(RULES_HINT), "{removed:?}");
        assert_eq!(removed.trim_end(), "# Team");
    }
}

#[test]
fn ux_skill_7_pull_request_description_names_no_space() {
    let dir = tempfile::tempdir().unwrap();
    let binding = RepoWikiBinding {
        space: "Secret Client Project".into(),
        repo_root: dir.path().to_string_lossy().into_owned(),
        folder: "wiki".into(),
        remote_url: None,
        daily: None,
        last_publish: None,
        last_daily_run: None,
        doc_fixes: Vec::new(),
        doc_check: None,
        kept_edits: Vec::new(),
        follows: Vec::new(),
        take_over: false,
    };
    for (pages, rules, rules_removed) in [(2, 3, false), (0, 1, false), (0, 0, true)] {
        let body = pr_body(PrBody {
            binding: &binding,
            pages,
            left_out: 1,
            waiting: 1,
            kept: 0,
            rules,
            rules_removed,
            agents_linked: false,
            project_paths: &[],
            project_adoptions: &[],
            team_rules: false,
            fixes: Vec::new(),
            slug: None,
            base: "main",
        });
        assert!(!body.contains("Secret Client Project"), "{body}");
        assert!(
            body.starts_with("Written by [Wenlan](https://wenlan.app)."),
            "{body}"
        );
        if rules > 0 {
            assert!(body.contains("from notes approved in Wenlan"), "{body}");
            assert!(body.contains("under \"Project rules\""), "{body}");
            assert!(
                body.contains("Check their wording before merging"),
                "{body}"
            );
        }
        if pages > 0 {
            assert!(body.contains("in `wiki/`"), "{body}");
        }
    }
}

#[test]
fn the_rules_block_stands_alone_or_after_the_wiki_pointer() {
    let rules = vec!["Use pnpm.".to_string()];
    let alone = upsert_agents_rules_block("# Agents\n", &rules);
    assert_eq!(
        alone,
        "# Agents\n\n<!-- WENLAN:START -->\n## Project rules\n\n- Use pnpm.\n\nFor a rule's source or reason, use `read_space_rules` if Wenlan is connected to this project's Space.\n<!-- WENLAN:END -->\n"
    );
    // Pages arrive later: the same block gains the pointer above the rules.
    let both = upsert_agents_block_with_rules(&alone, "wenlan", &rules);
    assert!(both.contains("`wenlan/index.md`"));
    assert!(both.contains("wins.\n\n## Project rules\n\n- Use pnpm.\n\nFor a rule's source or reason, use `read_space_rules` if Wenlan is connected to this project's Space.\n<!-- WENLAN:END -->"));
    assert_eq!(both.matches(AGENTS_START).count(), 1);
}

/// Two people, each with their own Wenlan, tied to one repository.
#[test]
fn a_second_wenlan_does_not_replace_what_another_keeps_in_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone_a) = repo_with_origin(dir.path());
    let clone_b = dir.path().join("work-b");
    let output = Command::new("git")
        .args(["clone", "--quiet"])
        .arg(&origin)
        .arg(&clone_b)
        .output()
        .unwrap();
    assert!(output.status.success());
    sh(&clone_b, &["config", "user.name", "B"]);
    sh(&clone_b, &["config", "user.email", "b@example.com"]);
    sh(&clone_b, &["config", "commit.gpgsign", "false"]);
    let clone_b = std::fs::canonicalize(clone_b).unwrap();

    let lib = "fn a() {}\nfn b() {}\nfn c() {}\n";
    let run = |who: &str, clone: &Path, id: &str, title: &str, rule: &str| {
        let store = dir.path().join(format!("data-{who}/repo-wiki.json"));
        let binding = match binding_at(&store, "site").unwrap() {
            Some(binding) => binding,
            None => bind_at(&store, "site", clone, None, None).unwrap(),
        };
        let code_bases = HashMap::from([(
            id.to_string(),
            vec![locator(&binding.repo_root, "src/lib.rs", 1, 2)],
        )]);
        let published = publish(PublishInput {
            rules: Some(vec![rule.to_string()]),
            publisher: publisher_id_at(&store).unwrap(),
            binding: &binding,
            pages: vec![page(id, title, "Body.")],
            left_out: 0,
            code_bases: &code_bases,
            anchors: &anchors_for(&code_bases, &[("src/lib.rs", lib)]),
            concept_paths: &ConceptPathMap::new(),
            worktree: dir.path().join(format!("data-{who}/repo-wiki/site")),
            gh: None,
            now: 1,
            prepared: None,
            taken_back: BTreeSet::new(),
            took_back: 0,
        })?;
        record_publish_at(&store, "site", &published).unwrap();
        Ok::<_, WenlanError>(published.result.status)
    };
    let holds = |tree: &str| {
        let files = sh(&origin, &["ls-tree", "-r", "--name-only", tree]);
        let agents = sh(&origin, &["show", &format!("{tree}:AGENTS.md")]);
        (
            files.contains("wenlan/pages/page-from-a.md"),
            files.contains("wenlan/pages/page-from-b.md"),
            agents.contains("- Rule from A."),
            agents.contains("- Rule from B."),
        )
    };
    let only_a = (true, false, true, false);

    // A's pull request is open: B's publish stops, and the branch is A's.
    assert_eq!(
        run("a", &clone_a, "pa", "Page from A", "Rule from A.").unwrap(),
        RepoWikiPublishStatus::Pushed
    );
    let refused = run("b", &clone_b, "pb", "Page from B", "Rule from B.").unwrap_err();
    assert!(matches!(refused, WenlanError::Conflict(_)), "{refused}");
    let message = refused.to_string();
    assert!(
        message.contains("already kept by another Wenlan"),
        "{message}"
    );
    assert!(
        message.contains("`wenlan repo bind --take-over`"),
        "{message}"
    );
    assert_eq!(holds(REPO_WIKI_BRANCH), only_a);
    // A keeps publishing its own.
    assert!(run("a", &clone_a, "pa", "Page from A", "Rule from A.").is_ok());

    // A's pull request is merged: B still stops, and nothing of A's goes.
    sh(&clone_a, &["fetch", "--quiet", "origin"]);
    sh(
        &clone_a,
        &[
            "merge",
            "--quiet",
            "--ff-only",
            &format!("origin/{REPO_WIKI_BRANCH}"),
        ],
    );
    sh(&clone_a, &["push", "--quiet", "origin", "main"]);
    sh(&origin, &["branch", "-D", REPO_WIKI_BRANCH]);
    assert!(run("b", &clone_b, "pb", "Page from B", "Rule from B.").is_err());
    assert_eq!(holds("main"), only_a);
    // A's next publish works though the merged branch is gone from the remote.
    assert!(run("a", &clone_a, "pa", "Page from A", "Rule from A.").is_ok());
    // B sees it in the working copy too, before any publish, once pulled.
    sh(&clone_b, &["pull", "--quiet", "origin", "main"]);
    let b_store = dir.path().join("data-b/repo-wiki.json");
    let b_id = publisher_id_at(&b_store).unwrap();
    assert!(working_copy_kept_by_another(&clone_b, &b_id));
    let a_id = publisher_id_at(&dir.path().join("data-a/repo-wiki.json")).unwrap();
    assert!(!working_copy_kept_by_another(&clone_a, &a_id));

    // B takes the repository over on purpose: one publish, then the mark is gone.
    set_take_over_at(&b_store, "site", true).unwrap();
    assert_eq!(
        run("b", &clone_b, "pb", "Page from B", "Rule from B.").unwrap(),
        RepoWikiPublishStatus::Pushed
    );
    assert_eq!(holds(REPO_WIKI_BRANCH), (false, true, false, true));
    assert!(!binding_at(&b_store, "site").unwrap().unwrap().take_over);
    // Now it is A who is told.
    assert!(run("a", &clone_a, "pa", "Page from A", "Rule from A.").is_err());
}

#[test]
fn a_block_from_before_publishers_were_named_is_ours_only_after_we_published() {
    assert_eq!(keeper(None, "me"), Keeper::Nobody);
    assert_eq!(keeper(Some("# Agents\n"), "me"), Keeper::Nobody);
    let old = upsert_agents_block("", "wiki");
    assert_eq!(keeper(Some(&old), "me"), Keeper::Unnamed);
    let mine = upsert_block("", &agents_block(None, &[], Some("me")));
    assert_eq!(block_publisher(&mine).as_deref(), Some("me"));
    assert_eq!(keeper(Some(&mine), "me"), Keeper::Ours);
    assert_eq!(keeper(Some(&mine), "you"), Keeper::Another);
}

/// UX-SKILL-7: what `CLAUDE.md` holds next to Wenlan's block in `AGENTS.md`.
#[test]
fn ux_skill_7_claude_md_reads_agents_md_and_never_holds_the_block_alone() {
    let block = agents_block(None, &["Use pnpm.".to_string()], Some("me"));
    // Missing: the import alone.
    assert_eq!(
        claude_file_for(None, &block).as_deref(),
        Some("@AGENTS.md\n")
    );
    // Empty or block-only: the import, in the file's own line ending.
    assert_eq!(
        claude_file_for(Some(""), &block).as_deref(),
        Some("@AGENTS.md\n")
    );
    let only = upsert_block("", &block).replace('\n', "\r\n");
    assert_eq!(
        claude_file_for(Some(&only), &block).as_deref(),
        Some("@AGENTS.md\r\n")
    );
    // Already imports AGENTS.md: left alone, and an old copy is taken out.
    assert_eq!(claude_file_for(Some("# Mine\n@AGENTS.md\n"), &block), None);
    let with_copy = upsert_block("# Mine\n@AGENTS.md\n", &block);
    assert_eq!(
        claude_file_for(Some(&with_copy), &block).as_deref(),
        Some("# Mine\n@AGENTS.md\n")
    );
    // The person's own text and no import: the block stays beside it.
    let hand = claude_file_for(Some("# Mine\n"), &block).unwrap();
    assert!(hand.starts_with("# Mine\n"), "{hand}");
    assert!(hand.contains("## Project rules\n\n- Use pnpm."), "{hand}");
}

/// UX-SKILL-7: a published repository with no `CLAUDE.md` gets one that
/// holds only `@AGENTS.md`, and the rules go into `AGENTS.md`.
#[test]
fn ux_skill_7_publish_adds_an_import_only_claude_md() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, clone) = repo_with_origin(dir.path());
    sh(&clone, &["rm", "--quiet", "CLAUDE.md"]);
    sh(&clone, &["commit", "--quiet", "-m", "no CLAUDE.md"]);
    sh(&clone, &["push", "--quiet", "origin", "main"]);
    let store = dir.path().join("data/repo-wiki.json");
    bind_at(&store, "site", &clone, None, None).unwrap();
    assert_eq!(
        publish_site(dir.path(), &store, Some(&["Rule A."]), false),
        RepoWikiPublishStatus::Pushed
    );
    let claude = sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:CLAUDE.md")]);
    assert_eq!(claude.trim_end(), "@AGENTS.md");
    let agents = sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:AGENTS.md")]);
    assert!(
        agents.contains("## Project rules\n\n- Rule A.\n"),
        "{agents}"
    );
}

/// UX-SKILL-7: a `CLAUDE.md` linked to `AGENTS.md` already reads the block,
/// so it is left alone and the rules in `AGENTS.md` survive.
#[cfg(unix)]
#[test]
fn ux_skill_7_linked_claude_md_keeps_the_agents_md_block() {
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("AGENTS.md", dir.path().join("CLAUDE.md")).unwrap();
    let touched = maintain_agent_files(dir.path(), None, &["Use pnpm.".to_string()], "me").unwrap();
    assert_eq!(touched, vec!["AGENTS.md"]);
    let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(agents.contains("- Use pnpm."), "{agents}");
    assert!(std::fs::symlink_metadata(dir.path().join("CLAUDE.md"))
        .unwrap()
        .file_type()
        .is_symlink());
}

/// UX-SKILL-7: the project heading names no Space (the repository may be
/// public), and the headings of earlier blocks are still read back.
#[test]
fn ux_skill_7_project_heading_names_no_space_and_old_headings_still_read() {
    let block = agents_block(None, &["Use pnpm.".to_string()], Some("me"));
    assert!(
        block.contains("## Project rules\n\n- Use pnpm.\n"),
        "{block}"
    );
    for old in ["## Rules for this project", "## Project rules for Clinic"] {
        let text = block.replace("## Project rules\n", &format!("{old}\n"));
        assert!(block_holds_rules(&text), "{old}");
        assert_eq!(block_rule_lines(&text), ["Use pnpm."], "{old}");
    }
}

/// UX-SKILL-7: a linked `AGENTS.md` is left alone, with `CLAUDE.md`, so the
/// rules are never lost through a link to `CLAUDE.md` and no file outside
/// the repository is written.
#[cfg(unix)]
#[test]
fn ux_skill_7_linked_agents_md_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    // Linked to an empty CLAUDE.md in the repository.
    std::fs::write(repo.join("CLAUDE.md"), "").unwrap();
    std::os::unix::fs::symlink("CLAUDE.md", repo.join("AGENTS.md")).unwrap();
    let rules = ["Use pnpm.".to_string()];
    assert!(maintain_agent_files(&repo, None, &rules, "me")
        .unwrap()
        .is_empty());
    assert_eq!(std::fs::read_to_string(repo.join("CLAUDE.md")).unwrap(), "");
    // Linked to a file outside the repository, also when taking rules out.
    let outside = dir.path().join("outside.md");
    let kept = format!("# Mine\n\n{}\n", agents_block(None, &rules, Some("me")));
    std::fs::write(&outside, &kept).unwrap();
    std::fs::remove_file(repo.join("AGENTS.md")).unwrap();
    std::os::unix::fs::symlink(&outside, repo.join("AGENTS.md")).unwrap();
    for rules in [&rules[..], &[]] {
        assert!(maintain_agent_files(&repo, None, rules, "me")
            .unwrap()
            .is_empty());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), kept);
    }
    assert!(std::fs::symlink_metadata(repo.join("AGENTS.md"))
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn ux_skill_7_pull_request_description_says_a_linked_agents_md_was_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let binding = RepoWikiBinding {
        space: "Space".into(),
        repo_root: dir.path().to_string_lossy().into_owned(),
        folder: "wiki".into(),
        remote_url: None,
        daily: None,
        last_publish: None,
        last_daily_run: None,
        doc_fixes: Vec::new(),
        doc_check: None,
        kept_edits: Vec::new(),
        follows: Vec::new(),
        take_over: false,
    };
    let body = pr_body(PrBody {
        binding: &binding,
        pages: 2,
        left_out: 0,
        waiting: 0,
        kept: 0,
        rules: 0,
        rules_removed: false,
        agents_linked: true,
        project_paths: &[],
        project_adoptions: &[],
        team_rules: false,
        fixes: Vec::new(),
        slug: None,
        base: "main",
    });
    assert!(
        body.contains("`AGENTS.md` is a link, so Wenlan left it and `CLAUDE.md` unchanged."),
        "{body}"
    );
    assert!(!body.contains("points coding agents at it"), "{body}");
    assert!(!body.contains("project rule"), "{body}");
}
