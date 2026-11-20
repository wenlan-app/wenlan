// SPDX-License-Identifier: Apache-2.0
use super::*;

const COPILOT: &str = ".github/copilot-instructions.md";
const SPACE: &str = "private fixture Space";
const PUBLISHER: &str = "fixture-keeper";

struct Scratch {
    _root: tempfile::TempDir,
    repo: PathBuf,
    store: PathBuf,
    binding: RepoWikiBinding,
}

impl Scratch {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("state/repo-wiki.json");
        let repo = root.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git_run(&repo, &["init", "--quiet", "-b", "main"]);
        git_run(&repo, &["config", "user.email", "fixture@example.invalid"]);
        git_run(&repo, &["config", "user.name", "Wenlan fixture"]);
        git_run(&repo, &["config", "core.autocrlf", "false"]);
        git_run(
            &repo,
            &["config", "core.hooksPath", ".git/fixture-no-hooks"],
        );
        for (path, text) in files {
            let path = repo.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        git_run(&repo, &["add", "--all"]);
        git_run(
            &repo,
            &["commit", "--quiet", "--allow-empty", "-m", "initial files"],
        );
        let binding = super::super::bind_at(&store, SPACE, &repo, None, None).unwrap();
        Self {
            _root: root,
            repo,
            store,
            binding,
        }
    }

    fn project(&self) -> Project {
        Project::open(&self.store, &self.binding).unwrap()
    }

    fn head(&self) -> String {
        git_run(&self.repo, &["rev-parse", "HEAD"])
    }

    fn prepare_draft(&self, at: &str) -> PathBuf {
        let checkout = self
            ._root
            .path()
            .join(format!("draft-{}", uuid::Uuid::new_v4()));
        git_run(
            &self.repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                checkout.to_str().unwrap(),
                at,
            ],
        );
        checkout
    }

    fn stage_and_commit(
        &self,
        project: &mut Project,
        base: &str,
        draft: &Path,
        plan: &Plan,
    ) -> String {
        plan.stage(draft).unwrap();
        git_run(
            draft,
            &["commit", "--quiet", "-m", "proposed project rules"],
        );
        let commit = git_run(draft, &["rev-parse", "HEAD"]);
        project
            .remember_delivery(&self.repo, base, &commit, plan, true)
            .unwrap();
        commit
    }

    fn inspect(&self, project: &mut Project, base: &str, staged: &StagedRules) -> Result<Observed> {
        project.inspect(&self.repo, base, None, &[], staged.keys().cloned())
    }

    fn body(&self, commit: &str, path: &str) -> String {
        let blob = blob_at(&self.repo, commit, path).unwrap().unwrap();
        blob_text(&self.repo, path, Some(&blob)).unwrap()
    }
}

fn git_run(repo: &Path, args: &[&str]) -> String {
    let mut command = git(repo);
    command.args(args);
    run(command, "temporary Git test repository").unwrap()
}

fn rules(bullets: &[&str]) -> (Vec<String>, StagedRules) {
    let master = bullets
        .iter()
        .map(|line| (*line).to_owned())
        .collect::<Vec<_>>();
    let body = format!(
        "{RULES_HEADING}\n\n{}\n",
        master
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let staged = [ROOT_AGENTS, ROOT_CLAUDE, COPILOT]
        .into_iter()
        .map(|path| (path.to_owned(), body.clone()))
        .collect();
    (master, staged)
}

#[test]
fn squash_landed_bytes_identify_a_teammate_edit_and_only_approved_content_fans_out() {
    let hand = "# Hand-written instructions\r\n\r\n- Use the existing runtime.  \r\n";
    let claude = "# Claude reference\n\nPlease keep the trailing whitespace.  ";
    let scratch = Scratch::new(&[(ROOT_AGENTS, hand), (ROOT_CLAUDE, claude)]);
    let (master, generated) = rules(&["Use the existing runtime.", "Run unit tests."]);
    let base = scratch.head();
    let proposal = {
        let mut project = scratch.project();
        let draft = scratch.prepare_draft(&base);
        let plan = scratch
            .inspect(&mut project, &base, &generated)
            .unwrap()
            .plan(&generated, Some("wiki"), PUBLISHER, &master, false, false)
            .unwrap();
        assert_eq!(plan.adopted, [ROOT_AGENTS, ROOT_CLAUDE]);
        let commit = scratch.stage_and_commit(&mut project, &base, &draft, &plan);
        assert_eq!(project.state.offered.len(), 1);
        assert!(project.state.landed.is_none());
        assert_eq!(fs::read_to_string(draft.join(ROOT_AGENTS)).unwrap(), hand);
        assert_eq!(fs::read_to_string(draft.join(ROOT_CLAUDE)).unwrap(), claude);
        assert_eq!(
            fs::read_to_string(scratch.repo.join(ROOT_AGENTS)).unwrap(),
            hand
        );
        assert!(!draft.join(COPILOT).exists());
        let agents = scratch.body(&commit, ROOT_AGENTS);
        assert!(agents.starts_with(hand));
        assert_eq!(agents.matches("Use the existing runtime.").count(), 1);
        assert!(agents.contains(super::super::RULES_HINT));
        assert!(!agents.contains(SPACE));
        assert!(scratch.body(&commit, ROOT_CLAUDE).starts_with(claude));
        commit
    };
    git_run(&scratch.repo, &["merge", "--squash", &proposal]);
    git_run(
        &scratch.repo,
        &["commit", "--quiet", "-m", "squash wiki PR locally"],
    );
    let landed = scratch.head();
    assert!(!super::super::is_ancestor(
        &scratch.repo,
        &proposal,
        &landed
    ));

    let agents = scratch.repo.join(ROOT_AGENTS);
    let changed = fs::read_to_string(&agents)
        .unwrap()
        .replace("Run unit tests.", "Run smoke tests.");
    fs::write(&agents, changed.as_bytes()).unwrap();
    git_run(&scratch.repo, &["add", ROOT_AGENTS]);
    git_run(
        &scratch.repo,
        &["commit", "--quiet", "-m", "teammate edits one tool's rules"],
    );
    let base = scratch.head();
    {
        let mut project = scratch.project();
        let error = scratch
            .inspect(&mut project, &base, &generated)
            .err()
            .unwrap();
        assert!(error.to_string().contains("awaiting review"));
        assert_eq!(project.state.landed.unwrap().commit, landed);
        assert!(project.state.offered.is_empty());
    }
    let card = proposals(&scratch.store, SPACE)
        .unwrap()
        .into_iter()
        .find(|card| card.removed_rules == ["Run unit tests."])
        .unwrap();
    assert_eq!(card.path, ROOT_AGENTS);
    assert_eq!(card.revision, base);
    assert_eq!(card.added_rules, ["Run smoke tests."]);
    assert!(card.diff.contains("+- Run smoke tests."));
    assert!(decide(&scratch.store, SPACE, &card.id, Some(&master)).is_err());
    assert!(scratch.body(&base, COPILOT).contains("Run unit tests."));
    assert!(!scratch.body(&base, COPILOT).contains("Run smoke tests."));

    let (approved, generated) = rules(&["Use the existing runtime.", "Run smoke tests."]);
    decide(&scratch.store, SPACE, &card.id, Some(&approved)).unwrap();
    assert!(decide(&scratch.store, SPACE, &card.id, Some(&approved)).is_err());
    let mut project = scratch.project();
    let draft = scratch.prepare_draft(&base);
    let plan = scratch
        .inspect(&mut project, &base, &generated)
        .unwrap()
        .plan(&generated, Some("wiki"), PUBLISHER, &approved, false, false)
        .unwrap();
    let next = scratch.stage_and_commit(&mut project, &base, &draft, &plan);
    for path in [ROOT_AGENTS, ROOT_CLAUDE, COPILOT] {
        let body = scratch.body(&next, path);
        assert_eq!(body.matches("Run smoke tests.").count(), 1, "{path}");
        assert_eq!(
            body.matches("Use the existing runtime.").count(),
            1,
            "{path}"
        );
        assert!(!body.contains("Run unit tests."), "{path}");
    }
    assert_eq!(fs::read_to_string(&agents).unwrap(), changed);
    assert_eq!(scratch.head(), base);
}

#[test]
fn an_open_offer_does_not_become_a_landed_baseline_or_create_removal_cards() {
    let scratch = Scratch::new(&[]);
    let (master, generated) = rules(&["Run unit tests."]);
    let base = scratch.head();
    let mut project = scratch.project();
    let draft = scratch.prepare_draft(&base);
    let plan = scratch
        .inspect(&mut project, &base, &generated)
        .unwrap()
        .plan(&generated, None, PUBLISHER, &master, false, false)
        .unwrap();
    let branch = scratch.stage_and_commit(&mut project, &base, &draft, &plan);
    project
        .inspect(
            &scratch.repo,
            &base,
            Some(&branch),
            &[],
            generated.keys().cloned(),
        )
        .unwrap();
    assert!(project.state.landed.is_none());
    assert!(project.state.reviews.is_empty());
    assert_eq!(project.state.offered.len(), 1);
}

#[test]
fn a_late_index_edit_is_never_replaced() {
    let scratch = Scratch::new(&[(ROOT_AGENTS, "Original hand text.\n")]);
    let (master, generated) = rules(&["Run unit tests."]);
    let base = scratch.head();
    let draft = scratch.prepare_draft(&base);
    let mut project = scratch.project();
    let plan = scratch
        .inspect(&mut project, &base, &generated)
        .unwrap()
        .plan(&generated, None, PUBLISHER, &master, false, false)
        .unwrap();
    fs::write(draft.join(ROOT_AGENTS), "Late edit.\n").unwrap();
    git_run(&draft, &["add", ROOT_AGENTS]);
    let error = plan.stage(&draft).err().unwrap();
    assert!(error.to_string().contains("changed while"));
    assert_eq!(git_run(&draft, &["show", ":AGENTS.md"]), "Late edit.");
    assert_eq!(
        git_run(&draft, &["ls-files", "--", ROOT_CLAUDE, COPILOT]),
        ""
    );
}

#[cfg(unix)]
#[test]
fn symlinked_rule_files_and_ancestors_are_blocked_in_git_and_on_disk() {
    use std::os::unix::fs::symlink;

    let scratch = Scratch::new(&[("other.md", "Outside the rules path.\n")]);
    symlink("other.md", scratch.repo.join(ROOT_AGENTS)).unwrap();
    symlink("other.md", scratch.repo.join(".github")).unwrap();
    git_run(&scratch.repo, &["add", ROOT_AGENTS, ".github"]);
    git_run(
        &scratch.repo,
        &["commit", "--quiet", "-m", "link rule paths"],
    );
    let base = scratch.head();
    assert!(blob_at(&scratch.repo, &base, ROOT_AGENTS)
        .unwrap_err()
        .to_string()
        .contains("symlink"));
    assert!(blob_at(&scratch.repo, &base, COPILOT)
        .unwrap_err()
        .to_string()
        .contains("symlink"));
    assert!(check_worktree_path(&scratch.repo, ROOT_AGENTS).is_err());
    assert!(check_worktree_path(&scratch.repo, COPILOT).is_err());
    assert!(scratch.project().check_links(&scratch.repo).is_err());
    assert_eq!(
        fs::read_to_string(scratch.repo.join("other.md")).unwrap(),
        "Outside the rules path.\n"
    );
}

#[test]
fn project_proposals_omit_credential_text_and_settings_paths_are_refused() {
    let fixture = "SERVICE_TOKEN=fixture-value";
    let scratch = Scratch::new(&[(ROOT_AGENTS, fixture)]);
    let base = scratch.head();
    let (_, generated) = rules(&["Run unit tests."]);
    {
        let mut project = scratch.project();
        scratch.inspect(&mut project, &base, &generated).unwrap();
        assert!(!fs::read_to_string(&project.path).unwrap().contains(fixture));
    }
    let cards = proposals(&scratch.store, SPACE).unwrap();
    assert_eq!(cards.len(), 1);
    assert!(cards[0].blocked.as_ref().unwrap().contains("credentials"));
    assert!(cards[0].diff.is_empty());
    assert!(cards[0].added_rules.is_empty());
    assert!(approved_texts(&[fixture.to_owned()]).is_err());
    assert!(validate_path(".claude/settings.json").is_err());
    assert!(validate_path(".git/AGENTS.md").is_err());
    assert!(validate_path("../AGENTS.md").is_err());
}

#[test]
fn claude_import_does_not_repeat_project_instructions() {
    let hand = "# Some instructions\n@AGENTS.md\n";
    let scratch = Scratch::new(&[(ROOT_CLAUDE, hand)]);
    let base = scratch.head();
    let (master, generated) = rules(&["Run unit tests."]);
    let mut project = scratch.project();
    let plan = scratch
        .inspect(&mut project, &base, &generated)
        .unwrap()
        .plan(&generated, None, PUBLISHER, &master, false, false)
        .unwrap();
    let draft = scratch.prepare_draft(&base);
    let commit = scratch.stage_and_commit(&mut project, &base, &draft, &plan);
    assert_eq!(scratch.body(&commit, ROOT_CLAUDE), hand);
    assert!(scratch
        .body(&commit, ROOT_AGENTS)
        .contains("Run unit tests."));
}

#[test]
fn a_local_unpushed_candidate_needs_delivery_evidence_before_matching_a_squash() {
    let scratch = Scratch::new(&[]);
    let (master, generated) = rules(&["Run unit tests."]);
    let base = scratch.head();
    let mut project = scratch.project();
    let draft = scratch.prepare_draft(&base);
    let plan = scratch
        .inspect(&mut project, &base, &generated)
        .unwrap()
        .plan(&generated, None, PUBLISHER, &master, false, false)
        .unwrap();
    plan.stage(&draft).unwrap();
    git_run(
        &draft,
        &["commit", "--quiet", "-m", "unpublished candidate"],
    );
    let candidate = git_run(&draft, &["rev-parse", "HEAD"]);
    project
        .remember_delivery(&scratch.repo, &base, &candidate, &plan, false)
        .unwrap();
    git_run(&scratch.repo, &["merge", "--squash", &candidate]);
    git_run(
        &scratch.repo,
        &[
            "commit",
            "--quiet",
            "-m",
            "same bytes without a known offer",
        ],
    );
    let landed = scratch.head();
    scratch.inspect(&mut project, &landed, &generated).unwrap();
    assert!(project.state.landed.is_none());
    project.confirm_delivery(&candidate).unwrap();
    scratch.inspect(&mut project, &landed, &generated).unwrap();
    assert_eq!(project.state.landed.unwrap().commit, landed);
}

#[test]
fn inline_markers_are_hand_text_and_a_foreign_block_is_kept() {
    let comment = format!("Explain `{AGENTS_START}` and `{AGENTS_END}` here.\n");
    let foreign = agents_block(None, &["A teammate's rule.".into()], Some("another-keeper"));
    let scratch = Scratch::new(&[(ROOT_AGENTS, &comment), (COPILOT, &foreign)]);
    assert!(parts(&comment).unwrap().block.is_empty());
    let base = scratch.head();
    let (master, generated) = rules(&["Run unit tests."]);
    let mut project = scratch.project();
    let error = scratch
        .inspect(&mut project, &base, &generated)
        .unwrap()
        .plan(&generated, None, PUBLISHER, &master, false, false)
        .err()
        .unwrap();
    assert!(error.to_string().contains("kept by another Wenlan"));
    assert_eq!(scratch.body(&base, COPILOT), foreign);
}
