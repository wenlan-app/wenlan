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
    assert!(!files.contains("entry-point"), "{files}");
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
    }
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
    bind_at(&store, "site", &clone, None, None).unwrap();

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
    bind_at(&store, "site", &clone, None, None).unwrap();
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
    bind_at(&store, "site", &clone, None, None).unwrap();
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
    let binding = bind_at(&store, "site", &clone, None, None).unwrap();
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
    let binding = bind_at(&store, "site", &clone, None, None).unwrap();
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
    let binding = bind_at(&store, "site", &clone, None, None).unwrap();
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
