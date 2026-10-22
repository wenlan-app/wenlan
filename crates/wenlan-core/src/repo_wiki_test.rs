// SPDX-License-Identifier: Apache-2.0
use super::*;
use wenlan_types::repo_wiki::RepoWikiAgent;

fn page(id: &str, title: &str, content: &str) -> Page {
    Page {
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
        format!("{}\n", agents_block("wiki"))
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
    assert_eq!(bound.folder, "wiki");
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
    let binding = bind_at(&store, "site", &clone, None, None).unwrap();
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
            binding: &binding,
            pages,
            left_out,
            code_bases: &code_bases,
            anchors: &anchors,
            concept_paths: &concept_paths,
            worktree: worktree.clone(),
            gh: None,
            now: 1,
        })
        .unwrap()
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
    assert!(!files.contains("entry-point"), "{files}");
    let claude = sh(&origin, &["show", &format!("{REPO_WIKI_BRANCH}:CLAUDE.md")]);
    assert!(claude.starts_with("# Notes\n\nKeep it small.\n\n<!-- WENLAN:START -->"));

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
    let binding = bind_at(&store, "site", &clone, None, None).unwrap();
    let code_bases = HashMap::from([(
        "p1".to_string(),
        vec![locator(&binding.repo_root, "src/lib.rs", 1, 2)],
    )]);
    let error = publish(PublishInput {
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
    };
    let result = publish(PublishInput {
        binding: &binding,
        pages: vec![],
        left_out: 4,
        code_bases: &HashMap::new(),
        anchors: &CodeAnchors::new(),
        concept_paths: &ConceptPathMap::new(),
        worktree: dir.path().join("wt"),
        gh: None,
        now: 7,
    })
    .unwrap();
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
        }),
        last_publish: None,
        last_daily_run: None,
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
}
