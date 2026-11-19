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
    RepoDocCheck, RepoDocCode, RepoDocFix, RepoDocFixState, RepoDocFollow, RepoDocPageLine,
    RepoWikiAgent, RepoWikiBinding, RepoWikiCandidate, RepoWikiDaily, RepoWikiPublishResult,
    RepoWikiPublishStatus, RepoWikiRunner, DEFAULT_REPO_WIKI_FOLDER, EARLIER_REPO_WIKI_FOLDER,
    REPO_WIKI_BRANCH,
};
use wenlan_types::CodeBasisLocator;

const STORE_FILE: &str = "repo-wiki.json";
const WORKTREES_DIR: &str = "repo-wiki";
const AGENTS_START: &str = "<!-- WENLAN:START -->";
const AGENTS_END: &str = "<!-- WENLAN:END -->";
/// Subject of every commit Wenlan makes on [`REPO_WIKI_BRANCH`]; a commit
/// with another subject there is someone else's.
const COMMIT_SUBJECT: &str = "docs(wiki): update the wiki from Wenlan";
/// The export's record of what it wrote (see `export::okf`).
const BUNDLE_MARKER: &str = ".wenlan-okf-export.json";
/// Where the export moves a bundle file someone edited (see `export::okf`).
const BUNDLE_ARCHIVE: &str = ".wenlan-archive";
/// Most fixes waiting for or in the pull request at once.
const MAX_OPEN_DOC_FIXES: usize = 50;
/// Most docs one page can lead.
const MAX_FOLLOW_FILES: usize = 20;
/// Most pages the docs of one Space can follow.
const MAX_FOLLOWS: usize = 20;
/// Longest quote of a followed page a fix may rest on.
const MAX_PAGE_QUOTE: usize = 500;

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Store {
    #[serde(default)]
    bindings: Vec<RepoWikiBinding>,
}

/// Where the bindings are kept.
pub fn store_path() -> PathBuf {
    crate::config::data_root().join(STORE_FILE)
}

/// File beside the store holding this Wenlan's publisher id.
const PUBLISHER_FILE: &str = "repo-wiki-publisher";
/// Line inside Wenlan's block in `AGENTS.md` naming the Wenlan that keeps it.
const PUBLISHER_MARK: &str = "<!-- wenlan-publisher: ";
/// Trailer of the wiki commit naming the Wenlan that made it.
const PUBLISHER_TRAILER: &str = "Wenlan-Publisher: ";

/// This Wenlan's publisher id: a random id made once and kept beside the
/// store at `store`. A repository's pages and rules are kept by one Wenlan;
/// the id is how another one tells they are not its own.
pub fn publisher_id_at(store: &Path) -> Result<String, WenlanError> {
    let file = store.with_file_name(PUBLISHER_FILE);
    if let Ok(id) = std::fs::read_to_string(&file) {
        let id = id.trim();
        if !id.is_empty() {
            return Ok(id.to_owned());
        }
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    std::fs::write(&file, &id)?;
    Ok(id)
}

/// The publisher named in Wenlan's block of an `AGENTS.md`, if it names one.
pub fn block_publisher(agents_md: &str) -> Option<String> {
    let start = agents_md.find(AGENTS_START)?;
    let end = agents_md[start..].find(AGENTS_END)? + start;
    let block = &agents_md[start..end];
    let from = block.find(PUBLISHER_MARK)? + PUBLISHER_MARK.len();
    let id = block[from..].split("-->").next()?.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

/// Whose pages and rules a text of `AGENTS.md` shows, seen from `ours`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keeper {
    /// No Wenlan block.
    Nobody,
    Ours,
    /// A block written before publishers were named.
    Unnamed,
    Another,
}

fn keeper(agents_md: Option<&str>, ours: &str) -> Keeper {
    match agents_md {
        Some(text) if text.contains(AGENTS_START) => match block_publisher(text) {
            Some(id) if id == ours => Keeper::Ours,
            Some(_) => Keeper::Another,
            None => Keeper::Unnamed,
        },
        _ => Keeper::Nobody,
    }
}

/// What a second Wenlan is told when the repository is another one's.
fn kept_by_another(folder: &str) -> WenlanError {
    WenlanError::Conflict(format!(
        "this repository's pages and rules are already kept by another Wenlan (a teammate's, \
         or another computer of yours), and publishing from here would replace theirs. Nothing \
         was sent. Read them in the repository: `AGENTS.md`, and `{folder}/` when it is there. \
         To publish from this computer instead, run `wenlan repo bind --take-over`"
    ))
}

/// Whether the working copy at `top` shows that another Wenlan keeps this
/// repository: its `AGENTS.md` block names a publisher that is not `ours`.
/// Reads the files on disk, no network.
pub fn working_copy_kept_by_another(top: &Path, ours: &str) -> bool {
    let text = std::fs::read_to_string(top.join("AGENTS.md")).ok();
    // A block from before publishers were named counts too: this is asked
    // before a first bind, so it cannot be this Wenlan's own.
    matches!(
        keeper(text.as_deref(), ours),
        Keeper::Another | Keeper::Unnamed
    )
}

/// The error for binding a repository another Wenlan keeps (see
/// [`working_copy_kept_by_another`]).
pub fn kept_by_another_error(folder: &str) -> WenlanError {
    kept_by_another(folder)
}

/// Set or clear the "next publish may take the repository over" mark.
pub fn set_take_over_at(
    path: &Path,
    space: &str,
    take_over: bool,
) -> Result<Option<RepoWikiBinding>, WenlanError> {
    let mut out = None;
    update_store_at(path, |bindings| {
        if let Some(binding) = bindings.iter_mut().find(|binding| binding.space == space) {
            binding.take_over = take_over;
            out = Some(binding.clone());
        }
        Ok(())
    })?;
    Ok(out)
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

pub(crate) fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        // A publish runs from the daemon, where nobody can answer a prompt.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never");
    command
}

pub(crate) fn run(mut command: Command, what: &str) -> Result<String, WenlanError> {
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

/// How long a git command that reaches the remote may take.
const NETWORK_LIMIT: std::time::Duration = std::time::Duration::from_secs(120);

/// [`run`], stopping the command when it takes longer than `limit`: a stalled
/// remote must not hold the daily run. For commands with little output.
fn run_within(
    mut command: Command,
    what: &str,
    limit: std::time::Duration,
) -> Result<String, WenlanError> {
    use std::process::Stdio;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| WenlanError::Generic(format!("{what}: could not run: {error}")))?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WenlanError::Generic(format!(
                    "{what} was stopped after {} seconds",
                    limit.as_secs()
                )));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(error) => {
                return Err(WenlanError::Generic(format!("{what}: {error}")));
            }
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| WenlanError::Generic(format!("{what}: {error}")))?;
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

pub fn origin_url(repo: &Path) -> Option<String> {
    let mut command = git(repo);
    command.args(["remote", "get-url", "origin"]);
    run(command, "git remote get-url origin")
        .ok()
        .filter(|url| !url.is_empty())
}

/// The folder to use in the repository at `top` when none is named:
/// [`DEFAULT_REPO_WIKI_FOLDER`], unless the repository has no pages there yet
/// and still holds the ones an earlier publish wrote under
/// [`EARLIER_REPO_WIKI_FOLDER`].
pub fn default_folder(top: &Path) -> &'static str {
    let published = |folder: &str| {
        top.join(folder)
            .join(crate::repo_wiki_ci::CODE_FILE)
            .is_file()
    };
    if !published(DEFAULT_REPO_WIKI_FOLDER) && published(EARLIER_REPO_WIKI_FOLDER) {
        EARLIER_REPO_WIKI_FOLDER
    } else {
        DEFAULT_REPO_WIKI_FOLDER
    }
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
                .unwrap_or_else(|| default_folder(&top).to_owned()),
        };
        let binding = match existing {
            Some(index) => {
                let current = &mut bindings[index];
                if current.repo_root != repo_root {
                    current.last_publish = None;
                    current.last_daily_run = None;
                    current.doc_fixes.clear();
                    current.doc_check = None;
                    current.kept_edits.clear();
                    current.follows.clear();
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
                    doc_fixes: Vec::new(),
                    doc_check: None,
                    kept_edits: Vec::new(),
                    follows: Vec::new(),
                    take_over: false,
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
    published: &Published,
) -> Result<(), WenlanError> {
    update_store_at(path, |bindings| {
        if let Some(binding) = bindings.iter_mut().find(|binding| binding.space == space) {
            binding.last_publish = Some(published.result.clone());
            if matches!(
                published.result.status,
                RepoWikiPublishStatus::Pushed
                    | RepoWikiPublishStatus::PrOpened
                    | RepoWikiPublishStatus::PrUpdated
            ) {
                binding.take_over = false;
            }
            if let Some(kept) = &published.kept_edits {
                binding.kept_edits = kept.clone();
            }
        }
        Ok(())
    })
}

/// A file inside the repository: relative, `/` separated, no `..`, not
/// inside `.git`.
fn normalize_repo_path(path: &str) -> Result<String, WenlanError> {
    let invalid = |why: &str| {
        WenlanError::Validation(format!(
            "path {path:?} {why}; name a file relative to the repository root, such as \
             \"README.md\" or \"docs/setup.md\""
        ))
    };
    let mut parts = Vec::new();
    for component in Path::new(path.trim()).components() {
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
        return Err(invalid("is empty"));
    }
    Ok(parts.join("/"))
}

/// Whether `path` (relative to the repository root) is one of the
/// repository's own docs Wenlan may suggest a fix to: a Markdown file outside
/// the wiki `folder`. A changelog is left to the release tooling that writes
/// it.
pub fn is_repo_doc(path: &str, folder: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    (lower.ends_with(".md") || lower.ends_with(".mdx"))
        && !name.starts_with("changelog")
        && path != folder
        && !path.starts_with(&format!("{folder}/"))
        && !path
            .split('/')
            .any(|part| part == "node_modules" || part == BUNDLE_ARCHIVE)
}

/// The id of a fix: the same file and old text give the same id.
pub fn doc_fix_id(path: &str, old_text: &str) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(format!("{path}\0{old_text}").as_bytes());
    format!("fix_{}", &format!("{digest:x}")[..12])
}

fn occurrences(text: &str, needle: &str) -> usize {
    text.match_indices(needle).count()
}

/// Whether `needle` overlaps Wenlan's own block in `text`.
fn inside_agents_block(text: &str, needle: &str) -> bool {
    let (Some(start), Some(end)) = (text.find(AGENTS_START), text.find(AGENTS_END)) else {
        return false;
    };
    let end = end + AGENTS_END.len();
    text.match_indices(needle)
        .any(|(at, found)| at < end && at + found.len() > start)
}

fn open_fixes(binding: &RepoWikiBinding) -> usize {
    binding
        .doc_fixes
        .iter()
        .filter(|fix| fix.state != RepoDocFixState::Dismissed)
        .count()
}

/// What a doc fix says, as an AI tool hands it over.
pub struct DocFixRequest {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
    pub reason: String,
    pub code: Vec<RepoDocCode>,
    /// For a fix from a followed page. The caller has checked that the quote
    /// is in that version of the page and that the version is current.
    pub page: Option<RepoDocPageLine>,
}

/// Record a fix to one of the repository's docs for the next publish of
/// `space`. The old text must occur exactly once in the file as it is in the
/// working copy, outside Wenlan's own block. A fix the user declined before is
/// refused. Returns the fix and how many are open.
pub fn suggest_doc_fix_at(
    store: &Path,
    space: &str,
    request: DocFixRequest,
    now: i64,
) -> Result<(RepoDocFix, usize), WenlanError> {
    let binding = binding_at(store, space)?.ok_or_else(|| {
        WenlanError::NotFound(format!(
            "Space {space:?} is not connected to a repository; connect it first"
        ))
    })?;
    let path = normalize_repo_path(&request.path)?;
    if !is_repo_doc(&path, &binding.folder) {
        return Err(WenlanError::Validation(format!(
            "{path} is not one of the repository's docs: fixes go to Markdown files outside \
             `{}/` (the wiki is kept with write_page) and not to a changelog",
            binding.folder
        )));
    }
    let old_text = request.old_text;
    let new_text = request.new_text;
    let reason = request.reason.trim().to_owned();
    if old_text.trim().is_empty() || old_text.chars().count() > 1000 {
        return Err(WenlanError::Validation(
            "old_text must be the exact text to replace, at most 1000 characters".into(),
        ));
    }
    if new_text == old_text || new_text.chars().count() > 1000 {
        return Err(WenlanError::Validation(
            "new_text must differ from old_text, at most 1000 characters".into(),
        ));
    }
    if reason.is_empty() || reason.chars().count() > 500 {
        return Err(WenlanError::Validation(if request.page.is_some() {
            "reason must say in one sentence what the followed page says now".into()
        } else {
            "reason must say in one sentence what the code says instead".into()
        }));
    }
    let page = match request.page {
        Some(page) => {
            if !request.code.is_empty() {
                return Err(WenlanError::Validation(
                    "a fix rests on the code or on a followed page, not both".into(),
                ));
            }
            let quote = page.quote.trim();
            if quote.is_empty() || quote.chars().count() > MAX_PAGE_QUOTE {
                return Err(WenlanError::Validation(format!(
                    "page.quote must be the page's line the fix rests on, at most \
                     {MAX_PAGE_QUOTE} characters"
                )));
            }
            let follows_file = binding
                .follows
                .iter()
                .any(|follow| follow.page_id == page.page_id && follow.files.contains(&path));
            if !follows_file {
                return Err(WenlanError::Validation(format!(
                    "{path} does not follow page {}; only the files a page leads take fixes \
                     from it",
                    page.page_id
                )));
            }
            Some(RepoDocPageLine {
                quote: quote.to_owned(),
                ..page
            })
        }
        None => None,
    };
    if request.code.len() > 20 {
        return Err(WenlanError::Validation(
            "at most 20 code entries per fix".into(),
        ));
    }
    let mut code = Vec::with_capacity(request.code.len());
    for entry in request.code {
        if entry.line_start == 0 || entry.line_end < entry.line_start {
            return Err(WenlanError::Validation(format!(
                "code {}: lines start at 1 and line_end is not before line_start",
                entry.path
            )));
        }
        code.push(RepoDocCode {
            path: normalize_repo_path(&entry.path)?,
            ..entry
        });
    }
    let text =
        std::fs::read_to_string(Path::new(&binding.repo_root).join(&path)).map_err(|_| {
            WenlanError::Validation(format!("{path} cannot be read in {}", binding.repo_root))
        })?;
    match occurrences(&text, &old_text) {
        1 => {}
        0 => {
            return Err(WenlanError::Validation(format!(
                "old_text is not in {path}; copy it from the file exactly"
            )))
        }
        n => {
            return Err(WenlanError::Validation(format!(
            "old_text occurs {n} times in {path}; include more of the sentence so it occurs once"
        )))
        }
    }
    if inside_agents_block(&text, &old_text) {
        return Err(WenlanError::Validation(format!(
            "old_text is inside Wenlan's own block of {path}, which Wenlan keeps itself"
        )));
    }
    let id = doc_fix_id(&path, &old_text);
    update_store_at(store, |bindings| {
        let binding = bindings
            .iter_mut()
            .find(|binding| binding.space == space)
            .ok_or_else(|| WenlanError::NotFound(format!("Space {space:?} is not connected")))?;
        if let Some(existing) = binding.doc_fixes.iter().find(|fix| fix.id == id) {
            if existing.state == RepoDocFixState::Dismissed {
                return Err(WenlanError::Conflict(format!(
                    "the user declined a fix to this text of {path}; leave it as it is"
                )));
            }
        } else if open_fixes(binding) >= MAX_OPEN_DOC_FIXES {
            return Err(WenlanError::Conflict(format!(
                "{MAX_OPEN_DOC_FIXES} doc fixes are already waiting; publish or review them first"
            )));
        }
        let fix = RepoDocFix {
            id: id.clone(),
            path: path.clone(),
            old_text: old_text.clone(),
            new_text: new_text.clone(),
            reason: reason.clone(),
            code: code.clone(),
            page: page.clone(),
            state: RepoDocFixState::Pending,
            found_at: now,
        };
        match binding
            .doc_fixes
            .iter_mut()
            .find(|existing| existing.id == id)
        {
            Some(existing) => *existing = fix.clone(),
            None => binding.doc_fixes.push(fix.clone()),
        }
        Ok((fix, open_fixes(binding)))
    })
}

/// The user declines the fix `id` of `space`: it leaves the pull request on
/// the next publish and is not suggested again while the old text is there.
pub fn dismiss_doc_fix_at(
    store: &Path,
    space: &str,
    id: &str,
) -> Result<(RepoDocFix, usize), WenlanError> {
    update_store_at(store, |bindings| {
        let binding = bindings
            .iter_mut()
            .find(|binding| binding.space == space)
            .ok_or_else(|| WenlanError::NotFound(format!("Space {space:?} is not connected")))?;
        let fix = binding
            .doc_fixes
            .iter_mut()
            .find(|fix| fix.id == id)
            .ok_or_else(|| WenlanError::NotFound(format!("no doc fix {id}")))?;
        fix.state = RepoDocFixState::Dismissed;
        let fix = fix.clone();
        Ok((fix, open_fixes(binding)))
    })
}

/// Record that an AI tool read `docs` of the repository's docs against the
/// code. Returns how many fixes are open.
pub fn record_doc_check_at(
    store: &Path,
    space: &str,
    docs: u32,
    now: i64,
) -> Result<usize, WenlanError> {
    update_store_at(store, |bindings| {
        let binding = bindings
            .iter_mut()
            .find(|binding| binding.space == space)
            .ok_or_else(|| WenlanError::NotFound(format!("Space {space:?} is not connected")))?;
        binding.doc_check = Some(RepoDocCheck { at: now, docs });
        Ok(open_fixes(binding))
    })
}

/// Make `files` of the repository follow the page `page_id` of `space`.
/// Each must be one of the repository's docs (see [`is_repo_doc`]) and exist
/// in the working copy. Following a page again replaces its files and keeps
/// the version they last followed.
pub fn follow_page_at(
    store: &Path,
    space: &str,
    page_id: &str,
    files: Vec<String>,
    now: i64,
) -> Result<RepoDocFollow, WenlanError> {
    let binding = binding_at(store, space)?.ok_or_else(|| {
        WenlanError::NotFound(format!(
            "Space {space:?} is not connected to a repository; connect it first"
        ))
    })?;
    let page_id = page_id.trim();
    if page_id.is_empty() {
        return Err(WenlanError::Validation("page_id is required".into()));
    }
    if files.is_empty() || files.len() > MAX_FOLLOW_FILES {
        return Err(WenlanError::Validation(format!(
            "name 1 to {MAX_FOLLOW_FILES} docs to follow the page"
        )));
    }
    let mut normalized: Vec<String> = Vec::with_capacity(files.len());
    for file in files {
        let path = normalize_repo_path(&file)?;
        if !is_repo_doc(&path, &binding.folder) {
            return Err(WenlanError::Validation(format!(
                "{path} is not one of the repository's docs: a page can lead Markdown files \
                 outside `{}/`, not a changelog",
                binding.folder
            )));
        }
        if !Path::new(&binding.repo_root).join(&path).is_file() {
            return Err(WenlanError::Validation(format!(
                "{path} is not in {}",
                binding.repo_root
            )));
        }
        if !normalized.contains(&path) {
            normalized.push(path);
        }
    }
    update_store_at(store, |bindings| {
        let binding = bindings
            .iter_mut()
            .find(|binding| binding.space == space)
            .ok_or_else(|| WenlanError::NotFound(format!("Space {space:?} is not connected")))?;
        if let Some(existing) = binding
            .follows
            .iter_mut()
            .find(|follow| follow.page_id == page_id)
        {
            existing.files = normalized.clone();
            return Ok(existing.clone());
        }
        if binding.follows.len() >= MAX_FOLLOWS {
            return Err(WenlanError::Conflict(format!(
                "the docs already follow {MAX_FOLLOWS} pages; stop following one first"
            )));
        }
        let follow = RepoDocFollow {
            page_id: page_id.to_owned(),
            files: normalized.clone(),
            followed_version: None,
            added_at: now,
        };
        binding.follows.push(follow.clone());
        Ok(follow)
    })
}

/// The docs stop following `page_id`. Fixes already found from it stay.
/// True when they followed it.
pub fn unfollow_page_at(store: &Path, space: &str, page_id: &str) -> Result<bool, WenlanError> {
    update_store_at(store, |bindings| {
        let binding = bindings
            .iter_mut()
            .find(|binding| binding.space == space)
            .ok_or_else(|| WenlanError::NotFound(format!("Space {space:?} is not connected")))?;
        let before = binding.follows.len();
        binding.follows.retain(|follow| follow.page_id != page_id);
        Ok(binding.follows.len() != before)
    })
}

/// An AI tool brought the docs that follow `page_id` up to `version` of it.
/// The caller has checked that `version` is the page's current one.
pub fn mark_followed_at(
    store: &Path,
    space: &str,
    page_id: &str,
    version: i64,
) -> Result<RepoDocFollow, WenlanError> {
    update_store_at(store, |bindings| {
        let binding = bindings
            .iter_mut()
            .find(|binding| binding.space == space)
            .ok_or_else(|| WenlanError::NotFound(format!("Space {space:?} is not connected")))?;
        let follow = binding
            .follows
            .iter_mut()
            .find(|follow| follow.page_id == page_id)
            .ok_or_else(|| {
                WenlanError::NotFound(format!("no docs follow page {page_id} in {space:?}"))
            })?;
        follow.followed_version = Some(version);
        Ok(follow.clone())
    })
}

/// Whether `quote` is on the page: the same text once surrounding white
/// space is set aside on each line, so a quote copied from a Markdown line
/// without its indent or list marker spacing still counts.
pub fn page_has_quote(page_text: &str, quote: &str) -> bool {
    let quote = quote.trim();
    !quote.is_empty() && (page_text.contains(quote) || squeeze(page_text).contains(&squeeze(quote)))
}

fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// After a publish: fixes it put in the pull request are `in_pr`, fixes whose
/// old text is gone from the default branch (merged, or the sentence changed)
/// are dropped, declined ones included.
pub fn settle_doc_fixes_at(
    store: &Path,
    space: &str,
    applied: &[String],
    gone: &[String],
) -> Result<(), WenlanError> {
    update_store_at(store, |bindings| {
        if let Some(binding) = bindings.iter_mut().find(|binding| binding.space == space) {
            binding.doc_fixes.retain(|fix| !gone.contains(&fix.id));
            for fix in &mut binding.doc_fixes {
                if applied.contains(&fix.id) && fix.state == RepoDocFixState::Pending {
                    fix.state = RepoDocFixState::InPr;
                }
            }
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
    code_footer(content, spans, folder)
}

/// `content` with the closing `## Code` list for `spans` (path, first line,
/// last line), sorted and deduplicated by the caller.
pub(crate) fn code_footer<'a>(
    content: &str,
    spans: impl IntoIterator<Item = (&'a str, u32, u32)>,
    folder: &str,
) -> String {
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

/// How many of the Space's rules ride in the always-read block.
pub const AGENTS_BLOCK_MAX_RULES: usize = 30;

/// The text between [`AGENTS_START`] and [`AGENTS_END`]: where the wiki is
/// (when the repository holds one, in `folder`), then the Space's own project
/// rules, one per line, without their source pages (the repository may be
/// public; the page titles are the person's).
fn agents_block(folder: Option<&str>, rules: &[String], publisher: Option<&str>) -> String {
    let mut block = format!("{AGENTS_START}\n");
    if let Some(publisher) = publisher {
        block.push_str(&format!("{PUBLISHER_MARK}{publisher} -->\n"));
    }
    if let Some(folder) = folder {
        block.push_str(&format!(
            "## Wiki\n\nThis repository has a wiki written by Wenlan in `{folder}/`. Start at \
             `{folder}/index.md`. Each page ends with the code lines it relies on. Read the page \
             for an unfamiliar part before changing it, and check the cited lines: when a page \
             and the code disagree, the code wins.\n"
        ));
    }
    let rules = block_rules(rules);
    if !rules.is_empty() {
        if folder.is_some() {
            block.push('\n');
        }
        block.push_str(&format!("{RULES_HEADING}\n\n"));
        for rule in rules {
            block.push_str(&format!("- {rule}\n"));
        }
    }
    block.push_str(AGENTS_END);
    block
}

/// The rules as they go into the block: one line each, at most
/// [`AGENTS_BLOCK_MAX_RULES`].
fn block_rules(rules: &[String]) -> Vec<String> {
    rules
        .iter()
        .map(|rule| rule.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|rule| !rule.is_empty() && !rule.contains(AGENTS_END))
        .take(AGENTS_BLOCK_MAX_RULES)
        .collect()
}

const RULES_HEADING: &str = "## Rules for this project";

/// The line ending a file uses: CRLF when it has any, LF otherwise.
fn line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// The lines of Wenlan's block in `text`, without their line endings.
fn block_lines(text: &str) -> Option<Vec<&str>> {
    let (start, end) = (text.find(AGENTS_START)?, text.find(AGENTS_END)?);
    (start < end).then(|| text[start..end].lines().map(str::trim_end).collect())
}

/// Whether Wenlan's block in `text` carries project rules.
fn block_holds_rules(text: &str) -> bool {
    block_lines(text).is_some_and(|lines| lines.contains(&RULES_HEADING))
}

/// The rules Wenlan's block in `text` carries: the bullet lines under its
/// rules heading, as written there.
fn block_rule_lines(text: &str) -> Vec<String> {
    let Some(lines) = block_lines(text) else {
        return Vec::new();
    };
    lines
        .iter()
        .skip_while(|line| **line != RULES_HEADING)
        .skip(1)
        .take_while(|line| !line.starts_with("## "))
        .filter_map(|line| line.strip_prefix("- "))
        .map(str::to_owned)
        .collect()
}

/// `text` without Wenlan's block, the person's text around it kept with its
/// own line endings.
fn remove_block(text: &str) -> String {
    let (Some(start), Some(end)) = (text.find(AGENTS_START), text.find(AGENTS_END)) else {
        return text.to_owned();
    };
    if start > end {
        return text.to_owned();
    }
    let eol = line_ending(text);
    let mut out = text[..start].trim_end().to_owned();
    let rest = text[end + AGENTS_END.len()..].trim_start_matches(['\r', '\n']);
    if !rest.is_empty() {
        if !out.is_empty() {
            out.push_str(eol);
            out.push_str(eol);
        }
        out.push_str(rest);
    } else if !out.is_empty() {
        out.push_str(eol);
    }
    out
}

/// `text` with Wenlan's block replaced, or appended when it has none. Text
/// outside the markers is never changed.
pub fn upsert_agents_block(text: &str, folder: &str) -> String {
    upsert_agents_block_with_rules(text, folder, &[])
}

/// [`upsert_agents_block`], with the Space's project rules inside the block.
pub fn upsert_agents_block_with_rules(text: &str, folder: &str, rules: &[String]) -> String {
    upsert_block(text, &agents_block(Some(folder), rules, None))
}

/// Wenlan's block with the project rules alone, for a repository that holds
/// no wiki folder: nothing in it points at one.
pub fn upsert_agents_rules_block(text: &str, rules: &[String]) -> String {
    upsert_block(text, &agents_block(None, rules, None))
}

fn upsert_block(text: &str, block: &str) -> String {
    // The block takes the file's own line endings.
    let eol = line_ending(text);
    let block = block.replace('\n', eol);
    if let (Some(start), Some(end)) = (text.find(AGENTS_START), text.find(AGENTS_END)) {
        if start < end {
            let end = end + AGENTS_END.len();
            return format!("{}{block}{}", &text[..start], &text[end..]);
        }
    }
    if text.trim().is_empty() {
        return format!("{block}{eol}");
    }
    format!("{}{eol}{eol}{block}{eol}", text.trim_end())
}

/// Keep Wenlan's block in `AGENTS.md` (created when missing) and in an
/// existing `CLAUDE.md` that does not already import `AGENTS.md`. `folder`
/// is the wiki folder when the repository holds one; without it the block
/// carries the rules alone. With no folder and no rules left, the block is
/// taken out, and an `AGENTS.md` that held nothing else goes with it.
fn maintain_agent_files(
    worktree: &Path,
    folder: Option<&str>,
    rules: &[String],
    publisher: &str,
) -> Result<Vec<&'static str>, WenlanError> {
    if folder.is_none() && block_rules(rules).is_empty() {
        let mut touched = Vec::new();
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let path = worktree.join(name);
            let Ok(current) = std::fs::read_to_string(&path) else {
                continue;
            };
            if !current.contains(AGENTS_START) {
                continue;
            }
            let left = remove_block(&current);
            if left.trim().is_empty() && name == "AGENTS.md" {
                std::fs::remove_file(&path)?;
            } else {
                std::fs::write(&path, left)?;
            }
            touched.push(name);
        }
        return Ok(touched);
    }
    let block = agents_block(folder, rules, Some(publisher));
    let mut touched = vec!["AGENTS.md"];
    let agents = worktree.join("AGENTS.md");
    let current = std::fs::read_to_string(&agents).unwrap_or_default();
    std::fs::write(&agents, upsert_block(&current, &block))?;
    let claude = worktree.join("CLAUDE.md");
    if let Ok(current) = std::fs::read_to_string(&claude) {
        if !current.contains("@AGENTS.md") {
            std::fs::write(&claude, upsert_block(&current, &block))?;
            touched.push("CLAUDE.md");
        }
    }
    Ok(touched)
}

/// `text` with the files the export rewrites on every publish marked
/// `linguist-generated`, so GitHub folds them in the pull request diff. Adds
/// only the lines that are missing and leaves every other line as it was. A
/// folder that would need quoting in a pattern is left unmarked.
fn upsert_generated_attributes(text: &str, folder: &str) -> String {
    if folder
        .chars()
        .any(|c| c.is_whitespace() || "*?[]\\\"#!".contains(c))
    {
        return text.to_owned();
    }
    let mut out = text.to_owned();
    for file in [crate::repo_wiki_ci::CODE_FILE, BUNDLE_MARKER] {
        let line = format!("{folder}/{file} linguist-generated=true");
        if text.lines().any(|existing| existing.trim() == line) {
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn maintain_gitattributes(worktree: &Path, folder: &str) -> Result<(), WenlanError> {
    let path = worktree.join(".gitattributes");
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = upsert_generated_attributes(&current, folder);
    if updated != current {
        std::fs::write(&path, updated)?;
    }
    Ok(())
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

/// Where a publish from a repository pushes the wiki branch and opens the
/// pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishTarget {
    /// `origin`'s URL; `None` when the repository has no `origin`.
    pub remote_url: Option<String>,
    /// `owner/name` when `origin` is on github.com.
    pub slug: Option<String>,
    /// The GitHub account the GitHub CLI is signed in to, when known.
    pub signed_in_as: Option<String>,
    /// Whether GitHub says that account may push to the repository: a
    /// teammate in the organization that owns it, or a collaborator.
    pub can_push: bool,
}

impl PublishTarget {
    /// What the user names to confirm this target: `owner/name` on GitHub,
    /// otherwise the remote URL.
    pub fn name(&self) -> Option<&str> {
        self.slug.as_deref().or(self.remote_url.as_deref())
    }

    /// The owner of the GitHub repository.
    pub fn owner(&self) -> Option<&str> {
        self.slug.as_deref().and_then(|slug| slug.split('/').next())
    }

    /// Whether the GitHub repository belongs to the signed-in account.
    pub fn owned(&self) -> bool {
        matches!(
            (self.owner(), self.signed_in_as.as_deref()),
            (Some(owner), Some(login)) if owner.eq_ignore_ascii_case(login)
        )
    }

    /// Whether pull requests may open here without the user naming the
    /// repository: it is theirs, or GitHub lets them push to it (a team's
    /// repository). Someone else's project they only cloned still needs naming.
    pub fn ours(&self) -> bool {
        self.owned() || self.can_push
    }

    /// Whether `confirm` names this target.
    pub fn confirmed_by(&self, confirm: &str) -> bool {
        let confirm = confirm.trim();
        match &self.slug {
            Some(slug) => {
                let confirm = github_slug(confirm).unwrap_or_else(|| {
                    confirm
                        .strip_prefix("github.com/")
                        .unwrap_or(confirm)
                        .trim_end_matches('/')
                        .to_owned()
                });
                confirm.eq_ignore_ascii_case(slug)
            }
            None => self.remote_url.as_deref().is_some_and(|url| url == confirm),
        }
    }
}

/// Where a publish from the repository holding `repo` would go, and which
/// GitHub account `gh` is signed in to.
pub fn publish_target(repo: &Path, gh: Option<&Path>) -> PublishTarget {
    let remote_url = origin_url(repo);
    let slug = remote_url.as_deref().and_then(github_slug);
    let signed_in_as = match (&slug, gh) {
        (Some(_), Some(gh)) => {
            let mut command = Command::new(gh);
            command
                .args(["api", "user", "--jq", ".login"])
                .stdin(std::process::Stdio::null());
            run(command, "gh api user")
                .ok()
                .filter(|login| !login.is_empty())
        }
        _ => None,
    };
    let can_push = match (&slug, &signed_in_as, gh) {
        (Some(slug), Some(_), Some(gh)) => {
            let mut command = Command::new(gh);
            command
                .args(["api", &format!("repos/{slug}"), "--jq", ".permissions.push"])
                .stdin(std::process::Stdio::null());
            run(command, "gh api repos").is_ok_and(|push| push == "true")
        }
        _ => false,
    };
    PublishTarget {
        remote_url,
        slug,
        signed_in_as,
        can_push,
    }
}

/// Whether two remote URLs name the same repository: the same `owner/name`
/// on GitHub over https or ssh, otherwise the same URL.
pub fn same_remote(a: &str, b: &str) -> bool {
    match (github_slug(a), github_slug(b)) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(&b),
        _ => a.trim() == b.trim(),
    }
}

/// The bound repository's `origin`, refused when it no longer names the
/// repository the user agreed to at bind: nothing is fetched from or pushed to
/// it until they bind again.
fn bound_origin(binding: &RepoWikiBinding) -> Result<String, WenlanError> {
    let repo = Path::new(&binding.repo_root);
    let remote = origin_url(repo).ok_or_else(|| {
        WenlanError::Validation(format!(
            "{} has no `origin` remote to open a pull request on",
            repo.display()
        ))
    })?;
    if let Some(bound) = binding.remote_url.as_deref() {
        if !same_remote(bound, &remote) {
            return Err(WenlanError::Validation(format!(
                "the repository's `origin` changed since it was bound (was {bound}, now \
                 {remote}); nothing was pushed. Run `wenlan repo bind` again to publish there"
            )));
        }
    }
    Ok(remote)
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
    /// The Space's own approved project rules, for the block in `AGENTS.md`.
    /// Never the general ones: those are the person's habits, not the
    /// repository's.
    /// `None` when they could not be read: the rules already in the
    /// repository's block stay as they are, never taken for "no rules".
    pub rules: Option<Vec<String>>,
    /// This Wenlan's publisher id ([`publisher_id_at`]). A repository another
    /// publisher keeps is left alone unless the binding says to take it over.
    pub publisher: String,
    /// Folder for this Space's worktree (under [`worktrees_root`]).
    pub worktree: PathBuf,
    /// `gh` to open the pull request with; `None` pushes the branch only.
    pub gh: Option<PathBuf>,
    /// Unix seconds, stamped on the result.
    pub now: i64,
    /// What [`prepare`] found, when the caller ran it first to take edits
    /// back into Wenlan; `None` runs it here.
    pub prepared: Option<Prepared>,
    /// Paths of the edits in `prepared` that Wenlan holds, because they were
    /// taken back now or already match the page: their files are written
    /// from the page again. Every other edit stays in the repository as the
    /// person wrote it.
    pub taken_back: BTreeSet<String>,
    /// How many of `taken_back` changed a page this time. An edit on the
    /// default branch is found again on every publish until the wiki pull
    /// request merges, and is counted only the first time.
    pub took_back: usize,
}

/// A wiki page file someone changed in the repository since Wenlan wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiFileEdit {
    pub page_id: String,
    /// Path inside the repository.
    pub path: String,
    /// Where it was read: the remote's default branch or the wiki branch.
    pub tree: String,
    /// The page version the file was written from.
    pub origin_version: i64,
    /// The file's body without its frontmatter and closing `## Code` list.
    pub body: String,
    /// The code lines its frontmatter names.
    pub spans: Vec<(String, u32, u32)>,
    /// The file is as a merged `wenlan wiki refresh` wrote it: the AI tool
    /// kept the page in step with the code, no person edited it.
    pub by_refresh: bool,
}

/// How [`RepoWikiBinding::kept_edits`] names an edit the repository keeps.
pub fn kept_edit_key(edit: &WikiFileEdit) -> String {
    format!("{}#{}", edit.path, sha256_hex(&edit.body))
}

/// What a publish learns from the repository before it writes anything.
#[derive(Debug, Clone)]
pub struct Prepared {
    base: String,
    /// Wiki page files people changed.
    pub edits: Vec<WikiFileEdit>,
    /// Other files people changed on the wiki branch after Wenlan's last
    /// commit there; the next commit keeps them as they are.
    carried: Vec<String>,
    /// The commit the remote wiki branch was at when this was prepared, or
    /// `None` when it was not there: the push replaces only that.
    branch_at: Option<String>,
}

/// A publish's result, and what became of the doc fixes.
#[derive(Debug)]
pub struct Published {
    pub result: RepoWikiPublishResult,
    /// Doc fixes in the pull request.
    pub applied: Vec<String>,
    /// Doc fixes whose old text is no longer on the default branch: merged,
    /// or the sentence changed since.
    pub gone: Vec<String>,
    /// The edits the repository keeps as people wrote them
    /// ([`RepoWikiBinding::kept_edits`]); `None` when the publish stopped
    /// before looking.
    pub kept_edits: Option<Vec<String>>,
}

pub(crate) fn remote_default_branch(repo: &Path) -> String {
    let mut command = git(repo);
    command.args(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]);
    if let Ok(name) = run(command, "git symbolic-ref") {
        if let Some(branch) = name.strip_prefix("origin/") {
            return branch.to_owned();
        }
    }
    let mut command = git(repo);
    command.args(["ls-remote", "--symref", "origin", "HEAD"]);
    if let Ok(out) = run_within(command, "git ls-remote", NETWORK_LIMIT) {
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
    tree_file(repo, &format!("origin/{base}"), path)
}

/// Whether the file at `path` in the working copy at `repo_root` reads as
/// it does on the remote's default branch.
pub fn working_copy_matches_default_branch(repo_root: &str, path: &str) -> bool {
    let repo = Path::new(repo_root);
    let base = remote_default_branch(repo);
    match (
        base_file(repo, &base, path),
        std::fs::read_to_string(repo.join(path)),
    ) {
        (Some(remote), Ok(local)) => remote == local,
        _ => false,
    }
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

/// The text of `path` in `tree` (`origin/<branch>` or a commit) of `repo`.
fn tree_file(repo: &Path, tree: &str, path: &str) -> Option<String> {
    let mut command = git(repo);
    command.args(["show", &format!("{tree}:{path}")]);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// Whether `commit` is already in `tree`'s history.
fn is_ancestor(repo: &Path, commit: &str, tree: &str) -> bool {
    let mut command = git(repo);
    command.args(["merge-base", "--is-ancestor", commit, tree]);
    command.status().is_ok_and(|status| status.success())
}

/// The object id of `path` in `tree`, or `None` when it is not there.
fn tree_blob(repo: &Path, tree: &str, path: &str) -> Option<String> {
    let mut command = git(repo);
    command.args([
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("{tree}:{path}"),
    ]);
    run(command, "git rev-parse").ok()
}

fn tree_exists(repo: &Path, tree: &str) -> bool {
    let mut command = git(repo);
    command.args(["rev-parse", "--verify", "--quiet", tree]);
    run(command, "git rev-parse").is_ok()
}

fn sha256_hex(text: &str) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(text.as_bytes()))
}

/// Page id, page version and body of a page file the export wrote: the body
/// without its frontmatter and the closing `## Code` list Wenlan adds.
pub fn parse_wiki_page_file(text: &str) -> Option<(String, i64, String)> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let (front, body) = (&rest[..end], &rest[end + 5..]);
    let field = |name: &str| {
        front
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(|value| value.trim().trim_matches('"'))
    };
    let page_id = field("origin_id:")?.to_owned();
    let version = field("origin_version:")?.parse().ok()?;
    let mut body = body.trim_start_matches('\n').trim_end().to_owned();
    const CODE: &str = "\n## Code\n";
    if let Some(at) = body.rfind(CODE) {
        let footer = &body[at + CODE.len()..];
        if footer
            .lines()
            .all(|line| line.trim().is_empty() || line.starts_with("- [`"))
        {
            body.truncate(at);
        }
    }
    Some((page_id, version, body.trim_end().to_owned()))
}

/// Page files in `folder` of `tree` whose bytes are no longer the ones the
/// export recorded writing there: someone edited them.
fn wiki_file_edits(repo: &Path, tree: &str, folder: &str) -> Vec<WikiFileEdit> {
    let Some(marker) = tree_file(repo, tree, &format!("{folder}/{BUNDLE_MARKER}")) else {
        return Vec::new();
    };
    let Ok(marker) = serde_json::from_str::<serde_json::Value>(&marker) else {
        return Vec::new();
    };
    let digests = marker.get("digests").and_then(serde_json::Value::as_object);
    let files = marker.get("files").and_then(serde_json::Value::as_array);
    let (Some(digests), Some(files)) = (digests, files) else {
        return Vec::new();
    };
    let manifest = tree_file(
        repo,
        tree,
        &format!("{folder}/{}", crate::repo_wiki_ci::CODE_FILE),
    );
    let mut edits = Vec::new();
    for entry in files.iter().filter_map(serde_json::Value::as_str) {
        if !entry.starts_with("pages/") || !entry.ends_with(".md") {
            continue;
        }
        let Some(recorded) = digests.get(entry).and_then(serde_json::Value::as_str) else {
            continue;
        };
        let path = format!("{folder}/{entry}");
        let Some(text) = tree_file(repo, tree, &path) else {
            continue;
        };
        if sha256_hex(&text) == recorded {
            continue;
        }
        if let Some((page_id, origin_version, body)) = parse_wiki_page_file(&text) {
            let by_refresh = manifest.as_deref().is_some_and(|manifest| {
                crate::repo_wiki_ci::written_by_refresh(manifest, entry, &text)
            });
            edits.push(WikiFileEdit {
                page_id,
                spans: crate::repo_wiki_ci::frontmatter_spans(&text),
                by_refresh,
                path,
                tree: tree.to_owned(),
                origin_version,
                body,
            });
        }
    }
    edits
}

/// Files the export writes in `folder`, which it rewrites every time.
fn written_by_export(path: &str, folder: &str) -> bool {
    let Some(rest) = path.strip_prefix(&format!("{folder}/")) else {
        return false;
    };
    rest == "index.md"
        || rest == BUNDLE_MARKER
        || rest == crate::repo_wiki_ci::CODE_FILE
        || ["pages/", "sources/", ".wenlan-archive/"]
            .iter()
            .any(|dir| rest.starts_with(dir))
}

/// Files people changed in their own commits on the wiki branch (not in
/// Wenlan's, and not in ones merged in from elsewhere) that the default
/// branch has not changed since the wiki branch started from it.
/// The publisher named by the newest Wenlan commit that `branch` holds and
/// the default branch does not: who keeps the open pull request.
/// A merged pull request usually deletes its branch on the remote, and a plain
/// fetch keeps our record of it. That stale record would be read as an open
/// pull request and would make the next push fail, so drop it — this one
/// branch only, never the person's other remote branches.
fn forget_deleted_wiki_branch(repo: &Path) {
    let mut command = git(repo);
    command.args([
        "ls-remote",
        "--heads",
        "origin",
        &format!("refs/heads/{REPO_WIKI_BRANCH}"),
    ]);
    let Ok(out) = run(command, "git ls-remote") else {
        return;
    };
    if out.trim().is_empty() {
        let mut command = git(repo);
        command.args([
            "update-ref",
            "-d",
            &format!("refs/remotes/origin/{REPO_WIKI_BRANCH}"),
        ]);
        let _ = run(command, "git update-ref");
    }
}

fn branch_publisher(repo: &Path, base_tree: &str, branch: &str) -> Option<String> {
    let mut command = git(repo);
    command.args(["log", "--format=%B%x00", &format!("{base_tree}..{branch}")]);
    let log = run(command, "git log").ok()?;
    log.split('\0').find_map(|message| {
        message
            .lines()
            .find_map(|line| line.strip_prefix(PUBLISHER_TRAILER))
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty())
    })
}

fn files_changed_by_others(repo: &Path, base_tree: &str, branch: &str, fork: &str) -> Vec<String> {
    let mut command = git(repo);
    command.args([
        "log",
        "--first-parent",
        "--no-merges",
        "--format=%H%x09%s",
        &format!("{base_tree}..{branch}"),
    ]);
    let Ok(log) = run(command, "git log") else {
        return Vec::new();
    };
    let mut paths = BTreeSet::new();
    for line in log.lines() {
        let Some((sha, subject)) = line.split_once('\t') else {
            continue;
        };
        if subject == COMMIT_SUBJECT {
            continue;
        }
        let mut command = git(repo);
        command.args([
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-z",
            sha,
        ]);
        if let Ok(out) = run(command, "git diff-tree") {
            paths.extend(
                out.split('\0')
                    .filter(|path| !path.is_empty())
                    .map(str::to_owned),
            );
        }
    }
    paths
        .into_iter()
        .filter(|path| tree_blob(repo, base_tree, path) == tree_blob(repo, fork, path))
        .collect()
}

/// Fetch, check out the remote's default branch in `worktree`, and find what
/// people changed in the repository since Wenlan last wrote there: wiki page
/// files (on the default branch, or on the open wiki branch), and other files
/// on the wiki branch. Blocking: runs git.
pub fn prepare(binding: &RepoWikiBinding, worktree: &Path) -> Result<Prepared, WenlanError> {
    let repo = PathBuf::from(&binding.repo_root);
    let folder = normalize_folder(&binding.folder)?;
    bound_origin(binding)?;
    let mut command = git(&repo);
    command.args(["fetch", "--quiet", "origin"]);
    run_within(command, "git fetch origin", NETWORK_LIMIT)?;
    forget_deleted_wiki_branch(&repo);
    let base = remote_default_branch(&repo);
    prepare_worktree(&repo, worktree, &base)?;

    let base_tree = format!("origin/{base}");
    let mut edits = wiki_file_edits(&repo, &base_tree, &folder);
    let mut carried = Vec::new();
    let branch = format!("origin/{REPO_WIKI_BRANCH}");
    if tree_exists(&repo, &branch) {
        let mut command = git(&repo);
        command.args(["merge-base", &base_tree, &branch]);
        if let Ok(fork) = run(command, "git merge-base") {
            // Only what the default branch has not changed since: once the
            // pull request is merged, the default branch is the newer copy.
            let unchanged =
                |path: &str| tree_blob(&repo, &base_tree, path) == tree_blob(&repo, &fork, path);
            for edit in wiki_file_edits(&repo, &branch, &folder) {
                if unchanged(&edit.path) {
                    edits.retain(|earlier| earlier.page_id != edit.page_id);
                    edits.push(edit);
                }
            }
            carried = files_changed_by_others(&repo, &base_tree, &branch, &fork)
                .into_iter()
                .filter(|path| !written_by_export(path, &folder))
                .collect();
        }
    }
    let mut command = git(&repo);
    command.args([
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("{branch}^{{commit}}"),
    ]);
    let branch_at = run(command, "git rev-parse")
        .ok()
        .filter(|sha| !sha.is_empty());
    Ok(Prepared {
        base,
        edits,
        carried,
        branch_at,
    })
}

/// Put `path` in `worktree` back as it is in `tree`, or remove it when it is
/// not there.
fn restore(worktree: &Path, tree: &str, path: &str) -> Result<(), WenlanError> {
    if tree_blob(worktree, tree, path).is_some() {
        let mut command = git(worktree);
        command.args(["checkout", tree, "--", path]);
        run(command, "git checkout")?;
    } else {
        match std::fs::remove_file(worktree.join(path)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// A quote on one line, for the pull request body.
fn one_line_quote(quote: &str) -> String {
    squeeze(quote).replace('"', "'")
}

struct PrBody<'a> {
    binding: &'a RepoWikiBinding,
    pages: usize,
    left_out: usize,
    waiting: usize,
    kept: usize,
    /// Project rules written into the block in `AGENTS.md`.
    rules: usize,
    /// Every project rule was removed in Wenlan, and this takes them out.
    rules_removed: bool,
    fixes: Vec<&'a RepoDocFix>,
    /// `owner/name` on GitHub, for links to the code.
    slug: Option<&'a str>,
    base: &'a str,
}

#[cfg(test)]
fn pr_body_for_test(binding: &RepoWikiBinding, fixes: Vec<&RepoDocFix>) -> String {
    pr_body(PrBody {
        binding,
        pages: 0,
        left_out: 0,
        waiting: 0,
        kept: 0,
        rules: 0,
        rules_removed: false,
        fixes,
        slug: None,
        base: "main",
    })
}

fn pr_body(body: PrBody<'_>) -> String {
    let PrBody {
        binding,
        pages,
        left_out,
        waiting,
        kept,
        rules,
        rules_removed,
        fixes,
        slug,
        base,
    } = body;
    let mut out = format!(
        "Written by [Wenlan](https://wenlan.app) from the `{}` Space.",
        binding.space
    );
    if pages > 0 {
        out.push_str(&format!(
            " {pages} wiki page{} in `{}/`, each ending with the code lines it relies on. \
             `AGENTS.md` points coding agents at it.",
            plural(pages),
            binding.folder
        ));
    }
    if rules > 0 {
        out.push_str(&format!(
            "{}`AGENTS.md` {} {rules} project rule{} from the person's own pages \
             in that Space, under \"Rules for this project\". Read them before merging: \
             they come from notes, not from the code.",
            if pages > 0 { "\n\n" } else { " " },
            if pages > 0 { "also carries" } else { "carries" },
            plural(rules)
        ));
    }
    if rules_removed {
        out.push_str(
            " Every project rule was removed in Wenlan, so this takes them out of Wenlan's part \
             of `AGENTS.md`; the rest of the file is unchanged.",
        );
    }
    if !fixes.is_empty() {
        out.push_str(&format!(
            "\n\n{} sentence{} in the repository's docs no longer match{} {}:\n",
            fixes.len(),
            plural(fixes.len()),
            if fixes.len() == 1 { "es" } else { "" },
            match (
                fixes.iter().any(|fix| fix.page.is_none()),
                fixes.iter().any(|fix| fix.page.is_some()),
            ) {
                (true, true) => "the code or the page the doc follows",
                (false, true) => "the page the doc follows",
                _ => "the code",
            }
        ));
        for fix in fixes {
            out.push_str(&format!("\n- `{}`: {}", fix.path, fix.reason));
            if let Some(page) = &fix.page {
                out.push_str(&format!(
                    " (the page it follows says: \"{}\")",
                    one_line_quote(&page.quote)
                ));
            }
            let links: Vec<String> = fix
                .code
                .iter()
                .map(|code| {
                    let lines = if code.line_start == code.line_end {
                        format!("L{}", code.line_start)
                    } else {
                        format!("L{}-L{}", code.line_start, code.line_end)
                    };
                    match slug {
                        Some(slug) => format!(
                            "[`{}` {lines}](https://github.com/{slug}/blob/{base}/{}#{lines})",
                            code.path, code.path
                        ),
                        None => format!("`{}` {lines}", code.path),
                    }
                })
                .collect();
            if !links.is_empty() {
                out.push_str(&format!(" ({})", links.join(", ")));
            }
        }
        out.push_str(
            "\n\nOnly those sentences change. Edit any of them here and Wenlan keeps your version.",
        );
    }
    if kept > 0 {
        out.push_str(&format!(
            "\n\n{kept} wiki page{} edited in this repository keep{} the edit; Wenlan changed {} \
             too, so its version waits in Wenlan.",
            plural(kept),
            if kept == 1 { "s" } else { "" },
            if kept == 1 { "it" } else { "them" }
        ));
    }
    if left_out > 0 {
        out.push_str(&format!(
            "\n\n{left_out} page{} of the Space rest on personal notes or chats rather than this \
             code and stay out of the repository.",
            plural(left_out)
        ));
    }
    if waiting > 0 {
        out.push_str(&format!(
            "\n\n{waiting} more page{} cite code that is not on the default branch yet and \
             join once it is pushed.",
            plural(waiting)
        ));
    }
    out
}

/// Record beside the pages which code lines each one relies on, with their
/// anchors in the default branch, for `wenlan wiki refresh` to check from a
/// plain checkout ([`crate::repo_wiki_ci`]).
fn write_code_manifest(
    repo: &Path,
    base: &str,
    target: &Path,
    published_bases: &HashMap<String, Vec<CodeBasisLocator>>,
    base_files: &HashMap<String, Option<String>>,
    keep_remote: &[String],
) -> Result<(), WenlanError> {
    let current = crate::repo_wiki_ci::read_manifest(target).ok();
    let mut command = git(repo);
    command.args(["rev-parse", &format!("origin/{base}")]);
    let commit = run(command, "git rev-parse").ok();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(target.join("pages"))? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(id) = frontmatter_origin_id(&text) else {
            continue;
        };
        let Some(spans) = published_bases.get(id) else {
            continue;
        };
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let mut spans: Vec<(String, u32, u32)> = spans
            .iter()
            .map(|locator| (locator.path.clone(), locator.line_start, locator.line_end))
            .collect();
        spans.sort();
        spans.dedup();
        files.push((format!("pages/{name}"), id.to_owned(), spans));
    }
    let mut manifest = crate::repo_wiki_ci::build_manifest(commit, files, |path| {
        base_files.get(path).and_then(|text| text.as_deref())
    });
    // A page kept as the repository has it keeps the code it names there.
    for file in keep_remote {
        if let Some(entry) = current.as_ref().and_then(|current| current.pages.get(file)) {
            manifest.pages.insert(file.clone(), entry.clone());
        }
    }
    // Same pages on the same lines: keep the commit it names, so a publish
    // after unrelated commits does not churn the pull request.
    if current.is_some_and(|current| current.pages == manifest.pages) {
        return Ok(());
    }
    crate::repo_wiki_ci::write_manifest(target, &manifest)
}

/// Whether the staged change in `worktree` touches only what a page's
/// version records (its version number, dates and the export marker): the
/// page reads as the default branch has it, as after a page came back from
/// the repository. Offering that would be a pull request with nothing in it.
fn only_bookkeeping_changed(worktree: &Path, folder: &str) -> bool {
    let mut command = git(worktree);
    command.args(["diff", "--cached", "--name-only"]);
    let Ok(names) = run(command, "git diff --cached") else {
        return false;
    };
    let marker = format!("{folder}/{}", crate::export::okf::MARKER_FILE);
    let pages = format!("{folder}/pages/");
    let show = |spec: String| {
        let mut command = git(worktree);
        command.args(["show", &spec]);
        run(command, "git show").ok()
    };
    let content = |text: String| -> Vec<String> {
        text.lines()
            .filter(|line| {
                !["origin_version: ", "modified: ", "generated: "]
                    .iter()
                    .any(|prefix| line.starts_with(prefix))
            })
            .map(|line| line.trim_end().to_owned())
            .collect()
    };
    names.lines().all(|name| {
        if name == marker {
            return true;
        }
        if !(name.starts_with(&pages) && name.ends_with(".md")) {
            return false;
        }
        match (show(format!("HEAD:{name}")), show(format!(":{name}"))) {
            (Some(before), Some(after)) => content(before) == content(after),
            _ => false,
        }
    })
}

/// The `origin_id:` of a page file's frontmatter.
pub fn frontmatter_origin_id(text: &str) -> Option<&str> {
    let frontmatter = text.strip_prefix("---\n")?;
    let end = frontmatter.find("\n---\n")?;
    frontmatter[..end]
        .lines()
        .find_map(|line| line.strip_prefix("origin_id: "))
        .map(str::trim)
}

/// Write the pages and the doc fixes into the repository and offer them as a
/// pull request. Blocking: runs git and gh. The user's working copy is only
/// read (its remote and objects); everything is written in `input.worktree`.
///
/// Nothing a person wrote in the repository is overwritten: a wiki page file
/// someone edited is written from the page again only when the edit was
/// taken back into Wenlan first (`input.taken_back`), and files people
/// changed on the open wiki branch are kept as they are.
pub fn publish(input: PublishInput<'_>) -> Result<Published, WenlanError> {
    let binding = input.binding;
    let waiting = std::cell::Cell::new(0);
    let kept = std::cell::Cell::new(0);
    let fix_count = std::cell::Cell::new(0);
    let taken_back = input.took_back;
    let result = |status, pages, branch: Option<&str>, pr_url, compare_url| RepoWikiPublishResult {
        status,
        pages,
        left_out: input.left_out,
        waiting: waiting.get(),
        branch: branch.map(str::to_owned),
        pr_url,
        compare_url,
        doc_fixes: fix_count.get(),
        taken_back,
        kept: kept.get(),
        at: input.now,
    };
    let nothing = |result: RepoWikiPublishResult| Published {
        result,
        applied: Vec::new(),
        gone: Vec::new(),
        kept_edits: None,
    };
    let open_fix = binding
        .doc_fixes
        .iter()
        .any(|fix| fix.state != RepoDocFixState::Dismissed);
    // Nothing new to put in. Only a repository this Wenlan published to can
    // still hold rules it put there that are now taken away.
    let rules_known = input.rules.is_some();
    let rules_given = input.rules.as_ref().is_some_and(|rules| !rules.is_empty());
    let nothing_new = input.pages.is_empty() && !open_fix && !rules_given;
    // Pushed before: a recorded "nothing to publish" put nothing there.
    let pushed_before = binding.last_publish.as_ref().is_some_and(|last| {
        last.branch.is_some()
            || matches!(
                last.status,
                RepoWikiPublishStatus::Pushed
                    | RepoWikiPublishStatus::PrOpened
                    | RepoWikiPublishStatus::PrUpdated
            )
    });
    if nothing_new && !pushed_before && !binding.take_over {
        return Ok(nothing(result(
            RepoWikiPublishStatus::NothingToPublish,
            0,
            None,
            None,
            None,
        )));
    }
    let repo = PathBuf::from(&binding.repo_root);
    let folder = normalize_folder(&binding.folder)?;
    let remote = bound_origin(binding);
    let prepared = match (remote.is_ok(), input.prepared) {
        (true, Some(prepared)) => Ok(prepared),
        (true, None) => prepare(binding, &input.worktree),
        (false, _) => Err(WenlanError::Generic(String::new())),
    };
    // With nothing new, a repository that cannot be looked at now stays as
    // it was, as before: there is nothing to tell the person.
    if nothing_new && prepared.is_err() {
        if let (Some(rules), Err(error)) = (input.rules.as_ref(), prepared.as_ref()) {
            if rules.is_empty() && remote.is_ok() {
                log::warn!(
                    "[repo_wiki] every rule of {:?} was removed, but the repository could not \
                     be looked at ({error}); its AGENTS.md keeps the old rules until a later \
                     publish",
                    binding.space
                );
            }
        }
        return Ok(nothing(result(
            RepoWikiPublishStatus::NothingToPublish,
            0,
            None,
            None,
            None,
        )));
    }
    let remote = remote?;
    let prepared = prepared?;
    let base = prepared.base.clone();
    let base_tree = format!("origin/{base}");

    // Rules this Wenlan put into the default branch, or into the open wiki
    // pull request, that are all taken away now: the pull request takes them
    // out again (UX-SKILL-7). Rules that could not be read are not "none".
    // Everything is read from the repository's copy, not the checkout, so
    // line endings the checkout converted do not hide the block.
    let published_before = binding.last_publish.is_some();
    let branch_tree = format!("origin/{REPO_WIKI_BRANCH}");
    let ours = |text: &str| {
        binding.take_over
            || match keeper(Some(text), &input.publisher) {
                Keeper::Ours => true,
                Keeper::Unnamed => pushed_before,
                Keeper::Another | Keeper::Nobody => false,
            }
    };
    let holds_our_rules = |tree: &str| {
        ["AGENTS.md", "CLAUDE.md"].iter().any(|name| {
            tree_file(&repo, tree, name).is_some_and(|text| block_holds_rules(&text) && ours(&text))
        })
    };
    let all_rules_removed = rules_known && !rules_given;
    let slug = github_slug(&remote);
    // The open wiki pull request, asked only when its rules matter. With
    // `gh` it is the open pull request GitHub names; without it, a wiki
    // branch the default branch has not taken in yet.
    let mut open_pr: Option<String> = None;
    let branch_open = (all_rules_removed || !rules_known)
        && tree_exists(&repo, &branch_tree)
        && match (input.gh.as_ref(), slug.as_ref()) {
            (Some(gh), Some(slug)) => match open_pull_request(gh, &input.worktree, slug) {
                Ok(url) if !url.is_empty() => {
                    open_pr = Some(url);
                    true
                }
                Ok(_) => false,
                Err(error) => {
                    log::warn!("[repo_wiki] {error}");
                    false
                }
            },
            _ => !is_ancestor(&repo, &branch_tree, &base_tree),
        };
    let rules_on_branch = all_rules_removed && branch_open && holds_our_rules(&branch_tree);
    let rules_to_take_out = all_rules_removed && (holds_our_rules(&base_tree) || rules_on_branch);
    // Rules unread: the block keeps the ones it holds now, in the open pull
    // request when that is ours, else on the default branch.
    let rules = match input.rules {
        Some(rules) => rules,
        None => {
            let on_branch = branch_open
                .then(|| tree_file(&repo, &branch_tree, "AGENTS.md"))
                .flatten()
                .filter(|text| text.contains(AGENTS_START) && ours(text));
            on_branch
                .or_else(|| tree_file(&repo, &base_tree, "AGENTS.md"))
                .map(|text| block_rule_lines(&text))
                .unwrap_or_default()
        }
    };
    if nothing_new && !rules_to_take_out {
        return Ok(nothing(result(
            RepoWikiPublishStatus::NothingToPublish,
            0,
            None,
            None,
            None,
        )));
    }

    // One Wenlan keeps a repository's pages and rules. Another one's are on
    // the default branch (merged) or on the open wiki branch: either way a
    // publish from here would replace them, so it stops unless told to take
    // the repository over. A block from before publishers were named counts
    // as ours only when this Wenlan has published here before.
    if !binding.take_over {
        let branch = format!("origin/{REPO_WIKI_BRANCH}");
        let on_base = keeper(
            tree_file(&repo, &base_tree, "AGENTS.md").as_deref(),
            &input.publisher,
        );
        let on_branch = if tree_exists(&repo, &branch) {
            match branch_publisher(&repo, &base_tree, &branch) {
                Some(id) if id == input.publisher => Keeper::Ours,
                Some(_) => Keeper::Another,
                None => keeper(
                    tree_file(&repo, &branch, "AGENTS.md").as_deref(),
                    &input.publisher,
                ),
            }
        } else {
            Keeper::Nobody
        };
        let foreign = |keeper| match keeper {
            Keeper::Another => true,
            Keeper::Unnamed => !published_before,
            Keeper::Ours | Keeper::Nobody => false,
        };
        if foreign(on_base) || foreign(on_branch) {
            return Err(kept_by_another(&folder));
        }
    }

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
    if ready.is_empty() && !open_fix && !rules_given && !rules_to_take_out {
        return Ok(nothing(result(
            RepoWikiPublishStatus::NothingToPublish,
            0,
            None,
            None,
            None,
        )));
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
    let export = !pages.is_empty();
    if export {
        let target = input.worktree.join(&folder);
        let target_existed = target.exists();
        let stats =
            export_okf_for_repository(&pages, &target, input.concept_paths, &published_bases)
                .map_err(|error| match error {
                    WenlanError::Conflict(message) if target_existed => {
                        WenlanError::Conflict(format!(
                            "{message}. `{folder}/` in the repository holds files Wenlan did \
                             not write; choose another folder"
                        ))
                    }
                    other => other,
                })?;
        if stats.failed > 0 {
            return Err(WenlanError::Generic(format!(
                "{} page file(s) could not be written into {folder}/",
                stats.failed
            )));
        }
    }
    // A page file someone edited: taken back into Wenlan, the export above
    // wrote it from the page again. Otherwise it goes back as they wrote it,
    // and is counted on every publish while it waits: it stays an edit until
    // someone changes the file or the page catches up.
    let mut kept_edits = Vec::new();
    for edit in &prepared.edits {
        let taken = input.taken_back.contains(&edit.path);
        if !(export && taken && input.worktree.join(&edit.path).is_file()) {
            restore(&input.worktree, &edit.tree, &edit.path)?;
        }
        // Taken back, even with nothing exported: the page says what the file says.
        if taken {
            continue;
        }
        kept.set(kept.get() + 1);
        kept_edits.push(kept_edit_key(edit));
    }
    if export {
        // The export moves an edited file aside before writing. Every edit
        // was settled above, so the moved copy stays out of the pull request.
        let mut command = git(&input.worktree);
        command.args(["clean", "-fdq", "--", &format!("{folder}/{BUNDLE_ARCHIVE}")]);
        run(command, "git clean")?;
        let kept_files: Vec<String> = prepared
            .edits
            .iter()
            .filter(|edit| !input.taken_back.contains(&edit.path))
            .filter_map(|edit| edit.path.strip_prefix(&format!("{folder}/")))
            .map(str::to_owned)
            .collect();
        write_code_manifest(
            &repo,
            &base,
            &input.worktree.join(&folder),
            &published_bases,
            &base_files,
            &kept_files,
        )?;
    }

    // Doc fixes, against the default branch. One whose old text is gone was
    // merged or overtaken; one in a file people changed on the wiki branch
    // stays as they left it.
    let carried: BTreeSet<&str> = prepared.carried.iter().map(String::as_str).collect();
    let mut applied = Vec::new();
    let mut gone = Vec::new();
    for fix in &binding.doc_fixes {
        let on_base = tree_file(&repo, &base_tree, &fix.path);
        if on_base
            .as_deref()
            .map(|text| occurrences(text, &fix.old_text))
            != Some(1)
        {
            gone.push(fix.id.clone());
            continue;
        }
        if fix.state == RepoDocFixState::Dismissed || carried.contains(fix.path.as_str()) {
            continue;
        }
        let file = input.worktree.join(&fix.path);
        let text = std::fs::read_to_string(&file)?;
        if occurrences(&text, &fix.old_text) != 1 {
            continue;
        }
        std::fs::write(&file, text.replacen(&fix.old_text, &fix.new_text, 1))?;
        applied.push(fix.id.clone());
    }
    fix_count.set(applied.len());
    for path in &prepared.carried {
        restore(&input.worktree, &branch_tree, path)?;
    }
    // The rules go into `AGENTS.md` with or without pages. The block points
    // at the wiki folder only when the repository holds one: written now, or
    // merged earlier and still on the default branch.
    let holds_wiki = export
        || input
            .worktree
            .join(&folder)
            .join(crate::repo_wiki_ci::CODE_FILE)
            .is_file();
    let writes_block = export || rules_given || rules_to_take_out;
    if writes_block {
        maintain_agent_files(
            &input.worktree,
            holds_wiki.then_some(folder.as_str()),
            &rules,
            &input.publisher,
        )?;
    }
    if export {
        maintain_gitattributes(&input.worktree, &folder)?;
    }
    crate::repo_wiki_github::maintain_workflow(
        &input.worktree,
        &folder,
        binding.daily.as_ref(),
        crate::repo_wiki_github::local_offset_seconds(),
    )?;

    let mut command = git(&input.worktree);
    command.args(["add", "--all"]);
    run(command, "git add")?;
    let page_count = pages.len();
    let settled = |result| Published {
        result,
        applied: applied.clone(),
        gone: gone.clone(),
        kept_edits: Some(kept_edits.clone()),
    };
    let mut command = git(&input.worktree);
    command.args(["diff", "--cached", "--quiet"]);
    let unchanged = command
        .status()
        .map_err(|error| WenlanError::Generic(format!("git diff: {error}")))?
        .success()
        || only_bookkeeping_changed(&input.worktree, &folder);
    if unchanged && !rules_on_branch {
        return Ok(settled(result(
            RepoWikiPublishStatus::NoChanges,
            page_count,
            None,
            None,
            None,
        )));
    }
    // The open pull request still carries rules that are all taken away, and
    // nothing else is left to change: it is closed, never left empty.
    if unchanged {
        if let (Some(gh), Some(slug), Some(url)) = (input.gh.as_ref(), slug.as_ref(), &open_pr) {
            let mut command = Command::new(gh);
            command.current_dir(&input.worktree).args([
                "pr",
                "close",
                url,
                "--repo",
                slug,
                "--comment",
                "Wenlan: every rule was removed, nothing left to change.",
                "--delete-branch",
            ]);
            match run(command, "gh pr close") {
                Ok(_) => {
                    return Ok(settled(result(
                        RepoWikiPublishStatus::NoChanges,
                        page_count,
                        None,
                        None,
                        None,
                    )))
                }
                // Not closed: the branch is still brought back below.
                Err(error) => log::warn!("[repo_wiki] {error}"),
            }
        }
    }
    // The commit stays on a detached HEAD: no local branch is added to the
    // user's repository, only the remote branch the push below names.
    let mut command = git(&input.worktree);
    command.args([
        "commit",
        "--quiet",
        "--allow-empty",
        "-m",
        COMMIT_SUBJECT,
        "-m",
        &format!("{PUBLISHER_TRAILER}{}", input.publisher),
    ]);
    run(command, "git commit")?;
    // The branch is rebuilt on the latest default branch every time. The
    // lease refuses to overwrite commits someone else pushed to it since the
    // fetch above.
    // The open pull request already holds exactly this tree: pushing a fresh
    // commit of it would only churn the pull request.
    let mut command = git(&input.worktree);
    command.args(["diff", "--quiet", &branch_tree, "HEAD"]);
    if tree_exists(&repo, &branch_tree)
        && command
            .status()
            .map_err(|error| WenlanError::Generic(format!("git diff: {error}")))?
            .success()
    {
        // Nothing new, but the pull request is still open: keep pointing at it.
        let open = match (input.gh.as_ref(), github_slug(&remote)) {
            (Some(gh), Some(slug)) => open_pull_request(gh, &input.worktree, &slug)
                .ok()
                .filter(|url| !url.is_empty()),
            _ => None,
        };
        return Ok(settled(result(
            RepoWikiPublishStatus::NoChanges,
            page_count,
            Some(REPO_WIKI_BRANCH),
            open,
            None,
        )));
    }
    // Only the branch as it was when this was prepared is replaced: a
    // commit someone pushed since, even one a later fetch brought in, stops
    // the push.
    let mut command = git(&input.worktree);
    command.args([
        "push",
        "--quiet",
        &format!(
            "--force-with-lease=refs/heads/{REPO_WIKI_BRANCH}:{}",
            prepared.branch_at.as_deref().unwrap_or_default()
        ),
        "origin",
        &format!("HEAD:refs/heads/{REPO_WIKI_BRANCH}"),
    ]);
    run(command, "git push")?;

    // A branch with nothing in it but the default branch: no pull request
    // is offered for it.
    let compare_url = slug.as_ref().filter(|_| !unchanged).map(|slug| {
        format!("https://github.com/{slug}/compare/{base}...{REPO_WIKI_BRANCH}?expand=1")
    });
    let body = pr_body(PrBody {
        binding,
        pages: page_count,
        left_out: input.left_out,
        waiting: waiting.get(),
        kept: kept.get(),
        rules: if writes_block {
            rules.len().min(AGENTS_BLOCK_MAX_RULES)
        } else {
            0
        },
        rules_removed: rules_to_take_out && !unchanged,
        fixes: binding
            .doc_fixes
            .iter()
            .filter(|fix| applied.contains(&fix.id))
            .collect(),
        slug: slug.as_deref(),
        base: &base,
    });
    let pushed = |compare_url| {
        Ok(settled(result(
            RepoWikiPublishStatus::Pushed,
            page_count,
            Some(REPO_WIKI_BRANCH),
            None,
            compare_url,
        )))
    };
    let (Some(gh), Some(slug)) = (input.gh.as_ref(), slug.as_ref()) else {
        return pushed(compare_url);
    };
    let open = match open_pull_request(gh, &input.worktree, slug) {
        Ok(url) => url,
        Err(error) => {
            log::warn!("[repo_wiki] {error}");
            return pushed(compare_url);
        }
    };
    if open.is_empty() && unchanged {
        return pushed(None);
    }
    if !open.is_empty() {
        // The description says what is in the pull request now.
        let mut command = Command::new(gh);
        command
            .current_dir(&input.worktree)
            .args(["pr", "edit", &open, "--repo", slug, "--body", &body]);
        if let Err(error) = run(command, "gh pr edit") {
            log::warn!("[repo_wiki] {error}");
        }
        return Ok(settled(result(
            RepoWikiPublishStatus::PrUpdated,
            page_count,
            Some(REPO_WIKI_BRANCH),
            Some(open),
            None,
        )));
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
        COMMIT_SUBJECT,
        "--body",
        &body,
    ]);
    match run(command, "gh pr create") {
        Ok(out) => {
            let url = out
                .lines()
                .rev()
                .find(|line| line.starts_with("https://"))
                .map(str::to_owned);
            Ok(settled(result(
                RepoWikiPublishStatus::PrOpened,
                page_count,
                Some(REPO_WIKI_BRANCH),
                url,
                None,
            )))
        }
        Err(error) => {
            log::warn!("[repo_wiki] {error}");
            pushed(compare_url)
        }
    }
}

/// The URL of the open wiki pull request in `slug`, or empty.
fn open_pull_request(gh: &Path, worktree: &Path, slug: &str) -> Result<String, WenlanError> {
    let mut command = Command::new(gh);
    command.current_dir(worktree).args([
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
    run(command, "gh pr list")
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
        if name == "cursor-agent" || name == "agent" {
            candidates.push(home.join(".cursor/bin"));
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
/// `agent` on `space`. The run is non-interactive and writes only through
/// Wenlan: Claude Code gets only the read-only tools above, Codex runs in its
/// read-only sandbox, Copilot may only read and call Wenlan, Cursor runs
/// without `--force` (it proposes file changes but makes none).
pub fn daily_agent_args(agent: RepoWikiAgent, space: &str, verb: &str) -> Vec<String> {
    let prompt = format!("Use the wenlan skill to run Wenlan: /wenlan {verb} space:{space}");
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
        RepoWikiAgent::Cursor => vec![
            "-p".into(),
            prompt,
            "--approve-mcps".into(),
            "--output-format".into(),
            "text".into(),
        ],
        RepoWikiAgent::Copilot => vec![
            "-p".into(),
            prompt,
            format!("--allow-tool={}", COPILOT_DAILY_TOOLS.join(", ")),
            "--no-ask-user".into(),
            "-s".into(),
        ],
    }
}

/// What a daily Copilot run may do without asking: read, use Wenlan, and
/// read git history.
const COPILOT_DAILY_TOOLS: &[&str] = &[
    "read",
    "wenlan",
    "shell(git rev-parse:*)",
    "shell(git ls-files:*)",
    "shell(git log:*)",
    "shell(git show:*)",
];

/// The executable for `agent`.
pub fn find_agent(agent: RepoWikiAgent) -> Option<PathBuf> {
    agent.programs().iter().find_map(|name| find_tool(name))
}

/// Where `agent` may read Wenlan's skill under `home`, for the tools that
/// read Agent Skills from a per-user folder the app's Set up fills. Claude
/// Code and Codex get theirs from a plugin instead, so they have none.
pub fn wenlan_skill_paths(agent: RepoWikiAgent, home: &Path) -> Vec<PathBuf> {
    let folders: &[&str] = match agent {
        RepoWikiAgent::Claude | RepoWikiAgent::Codex => &[],
        RepoWikiAgent::Cursor => &[".cursor/skills"],
        RepoWikiAgent::Copilot => &[".copilot/skills"],
    };
    folders
        .iter()
        .map(|folder| home.join(folder).join("wenlan/SKILL.md"))
        .collect()
}

/// Whether a daily run of `agent` on this computer can use Wenlan: without
/// the skill, the tool runs, finds no `/wenlan`, and changes nothing.
pub fn has_wenlan_skill(agent: RepoWikiAgent) -> bool {
    match dirs::home_dir() {
        Some(home) => {
            let paths = wenlan_skill_paths(agent, &home);
            paths.is_empty() || paths.iter().any(|path| path.is_file())
        }
        None => true,
    }
}

/// Whether a daily run of `binding` is due at local `now`: enabled, past its
/// hour, and not yet started today.
pub fn daily_due(binding: &RepoWikiBinding, now: chrono::DateTime<chrono::Local>) -> bool {
    use chrono::Timelike;
    let Some(daily) = &binding.daily else {
        return false;
    };
    daily.enabled
        && daily.runs_on == RepoWikiRunner::Computer
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
