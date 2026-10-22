// SPDX-License-Identifier: Apache-2.0
//! A Space bound to a git repository: its code pages are written into a
//! folder of that repository and offered as a pull request.
//!
//! The binding lives in `<data root>/repo-wiki.json`, beside `config.json`.
//! A publish never touches the user's working copy: it checks out the remote
//! default branch in a worktree under `<data root>/repo-wiki/`, writes the
//! pages there with the OKF exporter, commits on [`REPO_WIKI_BRANCH`], pushes
//! with the user's own git credentials and opens (or updates) the pull
//! request with the user's `gh`, when there is one.
//!
//! Only pages that rest on this repository's code alone are published: a page
//! with memory sources or `[n]` citations, or with no code basis in this
//! repository, stays in Wenlan, because a repository is often public.

use crate::code_anchor::{self, CodeAnchor, LineSpan, Relocation};
use crate::error::WenlanError;
use crate::export::okf::{export_okf_for_repository, ConceptPathMap};
use crate::pages::Page;
use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use wenlan_types::repo_wiki::{
    RepoWikiAgent, RepoWikiBinding, RepoWikiCandidate, RepoWikiDaily, RepoWikiPublishResult,
    RepoWikiPublishStatus, DEFAULT_REPO_WIKI_FOLDER, REPO_WIKI_BRANCH,
};
use wenlan_types::CodeBasisLocator;

const STORE_FILE: &str = "repo-wiki.json";
const WORKTREES_DIR: &str = "repo-wiki";
const AGENTS_START: &str = "<!-- WENLAN:START -->";
const AGENTS_END: &str = "<!-- WENLAN:END -->";

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Store {
    #[serde(default)]
    bindings: Vec<RepoWikiBinding>,
}

/// Where the bindings are kept.
pub fn store_path() -> PathBuf {
    crate::config::data_root().join(STORE_FILE)
}

/// Where publishes check out the repository, one folder per Space.
pub fn worktrees_root() -> PathBuf {
    crate::config::data_root().join(WORKTREES_DIR)
}

fn read_store(path: &Path) -> Result<Store, WenlanError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(serde_json::from_str(&text)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
        Err(error) => Err(error.into()),
    }
}

/// Every binding in the store at `path`.
pub fn load_bindings_at(path: &Path) -> Result<Vec<RepoWikiBinding>, WenlanError> {
    Ok(read_store(path)?.bindings)
}

/// The binding of `space` in the store at `path`, if it has one.
pub fn binding_at(path: &Path, space: &str) -> Result<Option<RepoWikiBinding>, WenlanError> {
    Ok(load_bindings_at(path)?
        .into_iter()
        .find(|binding| binding.space == space))
}

/// Change the store at `path` under an exclusive lock and replace it
/// atomically. `change` sees every binding and returns what the caller wants
/// back; an error leaves the file as it was.
pub fn update_store_at<T>(
    path: &Path,
    change: impl FnOnce(&mut Vec<RepoWikiBinding>) -> Result<T, WenlanError>,
) -> Result<T, WenlanError> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| WenlanError::Validation(format!("no folder for {}", path.display())))?;
    std::fs::create_dir_all(parent)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    let mut store = read_store(path)?;
    let out = change(&mut store.bindings)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(serde_json::to_string_pretty(&store)?.as_bytes())?;
    tmp.flush()?;
    tmp.persist(path)
        .map_err(|error| WenlanError::Io(error.error))?;
    Ok(out)
}

/// A folder inside the repository: relative, `/` separated, no `..`, not the
/// repository root and not inside `.git`.
pub fn normalize_folder(folder: &str) -> Result<String, WenlanError> {
    let trimmed = folder.trim().trim_matches('/');
    let invalid = |why: &str| {
        WenlanError::Validation(format!(
            "wiki folder {folder:?} {why}; name a folder inside the repository, such as \"wiki\""
        ))
    };
    if trimmed.is_empty() {
        return Err(invalid("is empty"));
    }
    let mut parts = Vec::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().ok_or_else(|| invalid("is not valid UTF-8"))?;
                if part == ".git" {
                    return Err(invalid("is inside .git"));
                }
                parts.push(part.to_owned());
            }
            Component::CurDir => {}
            _ => return Err(invalid("leaves the repository")),
        }
    }
    if parts.is_empty() {
        return Err(invalid("is the repository root"));
    }
    Ok(parts.join("/"))
}

fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        // A publish runs from the daemon, where nobody can answer a prompt.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never");
    command
}

fn run(mut command: Command, what: &str) -> Result<String, WenlanError> {
    let output = command
        .output()
        .map_err(|error| WenlanError::Generic(format!("{what}: could not run: {error}")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        Err(WenlanError::Generic(if detail.is_empty() {
            format!("{what} failed ({})", output.status)
        } else {
            format!("{what} failed: {detail}")
        }))
    }
}

/// The top level of the git working copy holding `path`, canonicalized.
pub fn repository_top_level(path: &Path) -> Result<PathBuf, WenlanError> {
    let mut command = git(path);
    command.args(["rev-parse", "--show-toplevel"]);
    let top = run(command, "git rev-parse").map_err(|_| {
        WenlanError::Validation(format!("{} is not inside a git repository", path.display()))
    })?;
    Ok(std::fs::canonicalize(&top).unwrap_or_else(|_| PathBuf::from(top)))
}

fn origin_url(repo: &Path) -> Option<String> {
    let mut command = git(repo);
    command.args(["remote", "get-url", "origin"]);
    run(command, "git remote get-url origin")
        .ok()
        .filter(|url| !url.is_empty())
}

/// Bind `space` to the repository holding `repo_path` in the store at `path`,
/// or update its folder and daily setting. `daily: None` keeps the current
/// daily setting.
pub fn bind_at(
    path: &Path,
    space: &str,
    repo_path: &Path,
    folder: Option<&str>,
    daily: Option<RepoWikiDaily>,
) -> Result<RepoWikiBinding, WenlanError> {
    if space.trim().is_empty() {
        return Err(WenlanError::Validation("a Space is required".into()));
    }
    if let Some(daily) = &daily {
        if daily.hour > 23 {
            return Err(WenlanError::Validation(format!(
                "daily hour {} is not between 0 and 23",
                daily.hour
            )));
        }
    }
    let top = repository_top_level(repo_path)?;
    let remote_url = origin_url(&top);
    let repo_root = top.to_string_lossy().into_owned();
    update_store_at(path, |bindings| {
        let existing = bindings.iter().position(|binding| binding.space == space);
        let folder = match folder {
            Some(folder) => normalize_folder(folder)?,
            None => existing
                .map(|index| bindings[index].folder.clone())
                .unwrap_or_else(|| DEFAULT_REPO_WIKI_FOLDER.to_owned()),
        };
        let binding = match existing {
            Some(index) => {
                let current = &mut bindings[index];
                if current.repo_root != repo_root {
                    current.last_publish = None;
                    current.last_daily_run = None;
                }
                current.repo_root = repo_root.clone();
                current.folder = folder;
                current.remote_url = remote_url.clone();
                if daily.is_some() {
                    current.daily = daily.clone();
                }
                current.clone()
            }
            None => {
                let binding = RepoWikiBinding {
                    space: space.to_owned(),
                    repo_root: repo_root.clone(),
                    folder,
                    remote_url: remote_url.clone(),
                    daily: daily.clone(),
                    last_publish: None,
                    last_daily_run: None,
                };
                bindings.push(binding.clone());
                binding
            }
        };
        Ok(binding)
    })
}

/// Remove the binding of `space`. True when there was one.
pub fn unbind_at(path: &Path, space: &str) -> Result<bool, WenlanError> {
    update_store_at(path, |bindings| {
        let before = bindings.len();
        bindings.retain(|binding| binding.space != space);
        Ok(bindings.len() != before)
    })
}

/// Record a publish result on the binding of `space`.
pub fn record_publish_at(
    path: &Path,
    space: &str,
    result: &RepoWikiPublishResult,
) -> Result<(), WenlanError> {
    update_store_at(path, |bindings| {
        if let Some(binding) = bindings.iter_mut().find(|binding| binding.space == space) {
            binding.last_publish = Some(result.clone());
        }
        Ok(())
    })
}

/// Pages of a Space that may go into the repository at `repo_root`: each has
/// at least one code basis in that repository and no memory sources or `[n]`
/// citations. Returns them with the count of pages left out.
pub fn select_pages(
    pages: Vec<Page>,
    code_bases: &HashMap<String, Vec<CodeBasisLocator>>,
    repo_root: &str,
) -> (Vec<Page>, usize) {
    let total = pages.len();
    let selected: Vec<Page> = pages
        .into_iter()
        .filter(|page| goes_into(page, code_bases, repo_root))
        .collect();
    let left_out = total - selected.len();
    (selected, left_out)
}

fn goes_into(
    page: &Page,
    code_bases: &HashMap<String, Vec<CodeBasisLocator>>,
    repo_root: &str,
) -> bool {
    page.source_memory_ids.is_empty()
        && page.citations.is_empty()
        && code_bases.get(&page.id).is_some_and(|locators| {
            locators
                .iter()
                .any(|locator| same_root(&locator.repo_root, repo_root))
        })
}

/// Repositories the pages of a Space could go into: every working copy a
/// page cites code from, with how many pages would go in, most first. A root
/// that is no longer a git working copy is left out. Runs git.
pub fn candidates(
    pages: &[Page],
    code_bases: &HashMap<String, Vec<CodeBasisLocator>>,
) -> Vec<RepoWikiCandidate> {
    let roots: BTreeSet<&str> = pages
        .iter()
        .filter_map(|page| code_bases.get(&page.id))
        .flatten()
        .map(|locator| locator.repo_root.trim_end_matches('/'))
        .collect();
    let mut found: Vec<RepoWikiCandidate> = roots
        .into_iter()
        .filter_map(|root| {
            let count = pages
                .iter()
                .filter(|page| goes_into(page, code_bases, root))
                .count();
            if count == 0 {
                return None;
            }
            let top = repository_top_level(Path::new(root)).ok()?;
            Some(RepoWikiCandidate {
                repo_root: root.to_owned(),
                pages: count,
                remote_url: origin_url(&top),
            })
        })
        .collect();
    found.sort_by(|a, b| {
        b.pages
            .cmp(&a.pages)
            .then_with(|| a.repo_root.cmp(&b.repo_root))
    });
    found
}

fn same_root(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// `../` from a page file (`<folder>/pages/<slug>.md`) back to the
/// repository root.
fn up_to_repo_root(folder: &str) -> String {
    "../".repeat(folder.split('/').count() + 1)
}

/// The page's body with a closing `## Code` list linking every line range it
/// relies on in this repository, so a reader on GitHub can jump to it.
pub fn with_code_footer(
    content: &str,
    locators: &[CodeBasisLocator],
    repo_root: &str,
    folder: &str,
) -> String {
    let spans: BTreeSet<(&str, u32, u32)> = locators
        .iter()
        .filter(|locator| same_root(&locator.repo_root, repo_root))
        .map(|locator| (locator.path.as_str(), locator.line_start, locator.line_end))
        .collect();
    if spans.is_empty() {
        return content.to_owned();
    }
    let up = up_to_repo_root(folder);
    let mut out = content.trim_end().to_owned();
    out.push_str("\n\n## Code\n\n");
    for (path, start, end) in spans {
        let (label, anchor) = if start == end {
            (format!("line {start}"), format!("L{start}"))
        } else {
            (format!("lines {start}-{end}"), format!("L{start}-L{end}"))
        };
        out.push_str(&format!("- [`{path}` {label}]({up}{path}#{anchor})\n"));
    }
    out
}

/// The text between [`AGENTS_START`] and [`AGENTS_END`].
fn agents_block(folder: &str) -> String {
    format!(
        "{AGENTS_START}\n## Wiki\n\nThis repository has a wiki written by Wenlan in `{folder}/`. Start at \
         `{folder}/index.md`. Each page ends with the code lines it relies on. Read the page \
         for an unfamiliar part before changing it, and check the cited lines: when a page \
         and the code disagree, the code wins.\n{AGENTS_END}"
    )
}

/// `text` with Wenlan's block replaced, or appended when it has none. Text
/// outside the markers is never changed.
pub fn upsert_agents_block(text: &str, folder: &str) -> String {
    let block = agents_block(folder);
    if let (Some(start), Some(end)) = (text.find(AGENTS_START), text.find(AGENTS_END)) {
        if start < end {
            let end = end + AGENTS_END.len();
            return format!("{}{block}{}", &text[..start], &text[end..]);
        }
    }
    if text.trim().is_empty() {
        return format!("{block}\n");
    }
    format!("{}\n\n{block}\n", text.trim_end())
}

/// Keep Wenlan's block in `AGENTS.md` (created when missing) and in an
/// existing `CLAUDE.md` that does not already import `AGENTS.md`.
fn maintain_agent_files(worktree: &Path, folder: &str) -> Result<Vec<&'static str>, WenlanError> {
    let mut touched = vec!["AGENTS.md"];
    let agents = worktree.join("AGENTS.md");
    let current = std::fs::read_to_string(&agents).unwrap_or_default();
    std::fs::write(&agents, upsert_agents_block(&current, folder))?;
    let claude = worktree.join("CLAUDE.md");
    if let Ok(current) = std::fs::read_to_string(&claude) {
        if !current.contains("@AGENTS.md") {
            std::fs::write(&claude, upsert_agents_block(&current, folder))?;
            touched.push("CLAUDE.md");
        }
    }
    Ok(touched)
}

/// `owner/name` of a github.com remote URL.
pub fn github_slug(remote: &str) -> Option<String> {
    let rest = remote
        .strip_prefix("https://github.com/")
        .or_else(|| remote.strip_prefix("http://github.com/"))
        .or_else(|| remote.strip_prefix("git@github.com:"))
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = rest.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

/// `gh`, looked up on `PATH` and where Homebrew and the installer put it (a
/// daemon started at login has a short `PATH`). `WENLAN_GH_BIN` overrides.
pub fn find_gh() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("WENLAN_GH_BIN") {
        let path = PathBuf::from(explicit);
        return path.is_file().then_some(path);
    }
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    candidates.extend(
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"]
            .iter()
            .map(PathBuf::from),
    );
    let exe = if cfg!(windows) { "gh.exe" } else { "gh" };
    candidates
        .into_iter()
        .map(|dir| dir.join(exe))
        .find(|path| path.is_file())
}

/// What a publish needs besides the binding.
/// Code basis anchors by [`anchor_key`].
pub type CodeAnchors = HashMap<(String, String, u32, u32), CodeAnchor>;

/// The key of one code basis of `page_id` in [`CodeAnchors`].
pub fn anchor_key(page_id: &str, locator: &CodeBasisLocator) -> (String, String, u32, u32) {
    (
        page_id.to_owned(),
        locator.path.clone(),
        locator.line_start,
        locator.line_end,
    )
}

pub struct PublishInput<'a> {
    pub binding: &'a RepoWikiBinding,
    /// Selected pages ([`select_pages`]).
    pub pages: Vec<Page>,
    pub left_out: usize,
    pub code_bases: &'a HashMap<String, Vec<CodeBasisLocator>>,
    /// The anchor of each code basis ([`anchor_key`]), to find its lines in
    /// the remote's default branch.
    pub anchors: &'a CodeAnchors,
    pub concept_paths: &'a ConceptPathMap,
    /// Folder for this Space's worktree (under [`worktrees_root`]).
    pub worktree: PathBuf,
    /// `gh` to open the pull request with; `None` pushes the branch only.
    pub gh: Option<PathBuf>,
    /// Unix seconds, stamped on the result.
    pub now: i64,
}

fn remote_default_branch(repo: &Path) -> String {
    let mut command = git(repo);
    command.args(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]);
    if let Ok(name) = run(command, "git symbolic-ref") {
        if let Some(branch) = name.strip_prefix("origin/") {
            return branch.to_owned();
        }
    }
    let mut command = git(repo);
    command.args(["ls-remote", "--symref", "origin", "HEAD"]);
    if let Ok(out) = run(command, "git ls-remote") {
        for line in out.lines() {
            if let Some(rest) = line.strip_prefix("ref: refs/heads/") {
                if let Some(branch) = rest.split_whitespace().next() {
                    return branch.to_owned();
                }
            }
        }
    }
    "main".to_owned()
}

/// Check out `origin/<base>` in `worktree`, reusing it when it is already a
/// worktree of `repo`.
fn prepare_worktree(repo: &Path, worktree: &Path, base: &str) -> Result<(), WenlanError> {
    let target = format!("origin/{base}");
    let reusable = worktree.join(".git").exists() && {
        let mut command = git(worktree);
        command.args(["rev-parse", "--git-common-dir"]);
        let mut repo_common = git(repo);
        repo_common.args(["rev-parse", "--git-common-dir"]);
        match (run(command, "rev-parse"), run(repo_common, "rev-parse")) {
            (Ok(theirs), Ok(ours)) => {
                let theirs = std::fs::canonicalize(worktree.join(&theirs))
                    .or_else(|_| std::fs::canonicalize(&theirs));
                let ours = std::fs::canonicalize(repo.join(&ours))
                    .or_else(|_| std::fs::canonicalize(&ours));
                matches!((theirs, ours), (Ok(a), Ok(b)) if a == b)
            }
            _ => false,
        }
    };
    if reusable {
        let mut command = git(worktree);
        command.args(["checkout", "--force", "--detach", &target]);
        run(command, "git checkout")?;
        let mut command = git(worktree);
        command.args(["clean", "-fdq"]);
        run(command, "git clean")?;
        return Ok(());
    }
    if worktree.exists() {
        // Ours (under the data root), but no longer a usable worktree.
        std::fs::remove_dir_all(worktree)?;
    }
    if let Some(parent) = worktree.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut command = git(repo);
    command.args(["worktree", "prune"]);
    run(command, "git worktree prune")?;
    let mut command = git(repo);
    command
        .args(["worktree", "add", "--force", "--detach"])
        .arg(worktree)
        .arg(&target);
    run(command, "git worktree add")?;
    Ok(())
}

/// The text of `path` on `origin/<base>` in `repo`, if it is there.
fn base_file(repo: &Path, base: &str, path: &str) -> Option<String> {
    let mut command = git(repo);
    command.args(["show", &format!("origin/{base}:{path}")]);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// Remove this Space's worktree from `repo_root`'s worktree list and from
/// disk. Best effort: an unbind should not fail over a worktree already gone.
pub fn remove_worktree(repo_root: &str, worktree: &Path) {
    let repo = Path::new(repo_root);
    if repo.exists() {
        let mut command = git(repo);
        command
            .args(["worktree", "remove", "--force"])
            .arg(worktree);
        let _ = run(command, "git worktree remove");
    }
    if worktree.exists() {
        let _ = std::fs::remove_dir_all(worktree);
    }
    if repo.exists() {
        let mut command = git(repo);
        command.args(["worktree", "prune"]);
        let _ = run(command, "git worktree prune");
    }
}

fn pr_body(binding: &RepoWikiBinding, pages: usize, left_out: usize, waiting: usize) -> String {
    let mut body = format!(
        "Written by [Wenlan](https://wenlan.app) from the `{}` Space: {pages} page{} in `{}/`, \
         each ending with the code lines it relies on. `AGENTS.md` points coding agents at it.",
        binding.space,
        if pages == 1 { "" } else { "s" },
        binding.folder
    );
    if left_out > 0 {
        body.push_str(&format!(
            "\n\n{left_out} page{} of the Space rest on personal notes or chats rather than this \
             code and stay out of the repository.",
            if left_out == 1 { "" } else { "s" }
        ));
    }
    if waiting > 0 {
        body.push_str(&format!(
            "\n\n{waiting} more page{} cite code that is not on the default branch yet and \
             join once it is pushed.",
            if waiting == 1 { "" } else { "s" }
        ));
    }
    body
}

/// Write the pages into the repository and offer them as a pull request.
/// Blocking: runs git and gh. The user's working copy is only read (its
/// remote and objects); everything is written in `input.worktree`.
pub fn publish(input: PublishInput<'_>) -> Result<RepoWikiPublishResult, WenlanError> {
    let binding = input.binding;
    let waiting = std::cell::Cell::new(0);
    let result = |status, pages, branch: Option<&str>, pr_url, compare_url| RepoWikiPublishResult {
        status,
        pages,
        left_out: input.left_out,
        waiting: waiting.get(),
        branch: branch.map(str::to_owned),
        pr_url,
        compare_url,
        at: input.now,
    };
    if input.pages.is_empty() {
        return Ok(result(
            RepoWikiPublishStatus::NothingToPublish,
            0,
            None,
            None,
            None,
        ));
    }
    let repo = PathBuf::from(&binding.repo_root);
    let folder = normalize_folder(&binding.folder)?;
    let remote = origin_url(&repo).ok_or_else(|| {
        WenlanError::Validation(format!(
            "{} has no `origin` remote to open a pull request on",
            repo.display()
        ))
    })?;

    let mut command = git(&repo);
    command.args(["fetch", "--quiet", "origin"]);
    run(command, "git fetch origin")?;
    let base = remote_default_branch(&repo);
    prepare_worktree(&repo, &input.worktree, &base)?;

    // A page's lines were read from the working copy; the pull request is
    // built on the remote's default branch. Each code basis is looked up there
    // by its anchor: found, the published page cites the lines it has there;
    // not found (code not pushed yet, or changed since), the page waits.
    let mut base_files: HashMap<String, Option<String>> = HashMap::new();
    let mut published_bases: HashMap<String, Vec<CodeBasisLocator>> = HashMap::new();
    let mut ready = Vec::new();
    let mut held = 0;
    for page in input.pages {
        let mut spans = Vec::new();
        let mut found = true;
        for locator in input
            .code_bases
            .get(&page.id)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            if !same_root(&locator.repo_root, &binding.repo_root) {
                continue;
            }
            let text = base_files
                .entry(locator.path.clone())
                .or_insert_with(|| base_file(&repo, &base, &locator.path));
            match (
                text.as_deref(),
                input.anchors.get(&anchor_key(&page.id, locator)),
            ) {
                (Some(text), Some(anchor)) => match code_anchor::relocate(
                    text,
                    Some(LineSpan {
                        start: locator.line_start,
                        end: locator.line_end,
                    }),
                    anchor,
                ) {
                    Relocation::Unchanged { span } | Relocation::Moved { span } => {
                        spans.push(CodeBasisLocator {
                            line_start: span.start,
                            line_end: span.end,
                            ..locator.clone()
                        });
                    }
                    Relocation::Changed { .. } | Relocation::Unresolved => found = false,
                },
                _ => found = false,
            }
        }
        if found {
            published_bases.insert(page.id.clone(), spans);
            ready.push(page);
        } else {
            held += 1;
        }
    }
    waiting.set(held);
    if ready.is_empty() {
        return Ok(result(
            RepoWikiPublishStatus::NothingToPublish,
            0,
            None,
            None,
            None,
        ));
    }

    let pages: Vec<Page> = ready
        .into_iter()
        .map(|mut page| {
            let locators = published_bases
                .get(&page.id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            page.content = with_code_footer(&page.content, locators, &binding.repo_root, &folder);
            page
        })
        .collect();
    let target = input.worktree.join(&folder);
    let target_existed = target.exists();
    let stats = export_okf_for_repository(&pages, &target, input.concept_paths, &published_bases)
        .map_err(|error| match error {
        WenlanError::Conflict(message) if target_existed => WenlanError::Conflict(format!(
            "{message}. `{folder}/` in the repository holds files Wenlan did not write; \
                 choose another folder"
        )),
        other => other,
    })?;
    if stats.failed > 0 {
        return Err(WenlanError::Generic(format!(
            "{} page file(s) could not be written into {folder}/",
            stats.failed
        )));
    }
    let touched = maintain_agent_files(&input.worktree, &folder)?;

    let mut command = git(&input.worktree);
    command.args(["add", "--all", "--", &folder]).args(&touched);
    run(command, "git add")?;
    let mut command = git(&input.worktree);
    command.args(["diff", "--cached", "--quiet"]);
    if command
        .status()
        .map_err(|error| WenlanError::Generic(format!("git diff: {error}")))?
        .success()
    {
        return Ok(result(
            RepoWikiPublishStatus::NoChanges,
            pages.len(),
            None,
            None,
            None,
        ));
    }
    // The commit stays on a detached HEAD: no local branch is added to the
    // user's repository, only the remote branch the push below names.
    let mut command = git(&input.worktree);
    command.args([
        "commit",
        "--quiet",
        "-m",
        "docs(wiki): update the wiki from Wenlan",
    ]);
    run(command, "git commit")?;
    // The branch is rebuilt on the latest default branch every time. The
    // lease refuses to overwrite commits someone else pushed to it since the
    // fetch above.
    // The open pull request already holds exactly this tree: pushing a fresh
    // commit of it would only churn the pull request.
    let mut command = git(&input.worktree);
    command.args([
        "diff",
        "--quiet",
        &format!("origin/{REPO_WIKI_BRANCH}"),
        "HEAD",
    ]);
    if command
        .status()
        .map_err(|error| WenlanError::Generic(format!("git diff: {error}")))?
        .success()
    {
        return Ok(result(
            RepoWikiPublishStatus::NoChanges,
            pages.len(),
            Some(REPO_WIKI_BRANCH),
            None,
            None,
        ));
    }
    let mut command = git(&input.worktree);
    command.args([
        "push",
        "--quiet",
        &format!("--force-with-lease=refs/heads/{REPO_WIKI_BRANCH}"),
        "origin",
        &format!("HEAD:refs/heads/{REPO_WIKI_BRANCH}"),
    ]);
    run(command, "git push")?;

    let slug = github_slug(&remote);
    let compare_url = slug.as_ref().map(|slug| {
        format!("https://github.com/{slug}/compare/{base}...{REPO_WIKI_BRANCH}?expand=1")
    });
    let pushed = |compare_url| {
        Ok(result(
            RepoWikiPublishStatus::Pushed,
            pages.len(),
            Some(REPO_WIKI_BRANCH),
            None,
            compare_url,
        ))
    };
    let (Some(gh), Some(slug)) = (input.gh.as_ref(), slug.as_ref()) else {
        return pushed(compare_url);
    };
    let mut command = Command::new(gh);
    command.current_dir(&input.worktree).args([
        "pr",
        "list",
        "--repo",
        slug,
        "--head",
        REPO_WIKI_BRANCH,
        "--state",
        "open",
        "--json",
        "url",
        "--jq",
        ".[0].url // \"\"",
    ]);
    let open = match run(command, "gh pr list") {
        Ok(url) => url,
        Err(error) => {
            log::warn!("[repo_wiki] {error}");
            return pushed(compare_url);
        }
    };
    if !open.is_empty() {
        return Ok(result(
            RepoWikiPublishStatus::PrUpdated,
            pages.len(),
            Some(REPO_WIKI_BRANCH),
            Some(open),
            None,
        ));
    }
    let mut command = Command::new(gh);
    command.current_dir(&input.worktree).args([
        "pr",
        "create",
        "--repo",
        slug,
        "--head",
        REPO_WIKI_BRANCH,
        "--base",
        &base,
        "--title",
        "docs(wiki): update the wiki from Wenlan",
        "--body",
        &pr_body(binding, pages.len(), input.left_out, waiting.get()),
    ]);
    match run(command, "gh pr create") {
        Ok(out) => {
            let url = out
                .lines()
                .rev()
                .find(|line| line.starts_with("https://"))
                .map(str::to_owned);
            Ok(result(
                RepoWikiPublishStatus::PrOpened,
                pages.len(),
                Some(REPO_WIKI_BRANCH),
                url,
                None,
            ))
        }
        Err(error) => {
            log::warn!("[repo_wiki] {error}");
            pushed(compare_url)
        }
    }
}

/// Find an executable named `name` on `PATH` or in the usual install
/// folders a daemon started at login does not have on its `PATH`.
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let home = dirs::home_dir();
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = &home {
        candidates.push(home.join(".local/bin"));
        if name == "claude" {
            candidates.push(home.join(".claude/local"));
        }
    }
    candidates.extend(
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"]
            .iter()
            .map(PathBuf::from),
    );
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    candidates
        .into_iter()
        .map(|dir| dir.join(&exe))
        .find(|path| path.is_file())
}

/// Read-only tools a daily Claude Code run may use without asking: reading
/// the repository, read-only shell commands, and Wenlan's own tools.
const CLAUDE_DAILY_TOOLS: &[&str] = &[
    "Skill",
    "Read",
    "Glob",
    "Grep",
    "Bash(git rev-parse:*)",
    "Bash(git ls-files:*)",
    "Bash(git check-ignore:*)",
    "Bash(git log:*)",
    "Bash(git show:*)",
    "Bash(ls:*)",
    "Bash(wc:*)",
    "Bash(grep:*)",
    "Bash(sed -n:*)",
    "Bash(cat:*)",
    "mcp__plugin_wenlan_wenlan__*",
];

/// Program arguments for one daily step (`verb` is `steep` or `wiki`) of
/// `agent` on `space`. The run is non-interactive: Claude Code gets only the
/// read-only tools above; Codex runs in its read-only sandbox.
pub fn daily_agent_args(agent: RepoWikiAgent, space: &str, verb: &str) -> Vec<String> {
    match agent {
        RepoWikiAgent::Claude => {
            let mut args = vec![
                "-p".to_owned(),
                format!("/wenlan {verb} space:{space}"),
                "--allowedTools".to_owned(),
            ];
            args.extend(CLAUDE_DAILY_TOOLS.iter().map(|tool| (*tool).to_owned()));
            args
        }
        RepoWikiAgent::Codex => vec![
            "exec".into(),
            "--skip-git-repo-check".into(),
            "--sandbox".into(),
            "read-only".into(),
            format!("$wenlan {verb} space:{space}"),
        ],
    }
}

/// The executable for `agent`.
pub fn find_agent(agent: RepoWikiAgent) -> Option<PathBuf> {
    find_tool(match agent {
        RepoWikiAgent::Claude => "claude",
        RepoWikiAgent::Codex => "codex",
    })
}

/// Whether a daily run of `binding` is due at local `now`: enabled, past its
/// hour, and not yet started today.
pub fn daily_due(binding: &RepoWikiBinding, now: chrono::DateTime<chrono::Local>) -> bool {
    use chrono::Timelike;
    let Some(daily) = &binding.daily else {
        return false;
    };
    daily.enabled
        && now.hour() >= u32::from(daily.hour)
        && binding.last_daily_run.as_deref() != Some(now.format("%Y-%m-%d").to_string().as_str())
}

/// Mark the daily run of `space` as started on `date` (`YYYY-MM-DD`).
pub fn mark_daily_started_at(path: &Path, space: &str, date: &str) -> Result<(), WenlanError> {
    update_store_at(path, |bindings| {
        if let Some(binding) = bindings.iter_mut().find(|binding| binding.space == space) {
            binding.last_daily_run = Some(date.to_owned());
        }
        Ok(())
    })
}

/// A folder name for `space` that is safe on every platform.
pub fn worktree_for(space: &str) -> PathBuf {
    let safe: String = space
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    worktrees_root().join(safe)
}

#[cfg(test)]
#[path = "repo_wiki_test.rs"]
mod tests;
