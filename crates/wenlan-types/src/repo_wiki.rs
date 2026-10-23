// SPDX-License-Identifier: Apache-2.0
//! Wire types for a Space bound to a git repository: its wiki pages are
//! written into a folder of that repository and offered as a pull request.
//!
//! Only pages that rest on the repository's code alone go into the
//! repository; a page that cites the user's memories stays in Wenlan, since a
//! repository is often public.

use serde::{Deserialize, Serialize};

/// Folder inside the repository the wiki is written to when none is named.
pub const DEFAULT_REPO_WIKI_FOLDER: &str = "wiki";

/// Branch the wiki pull request is offered from. One branch per repository:
/// while its pull request is open, every publish updates that pull request.
pub const REPO_WIKI_BRANCH: &str = "wenlan/wiki";

/// The AI tool a daily run starts in the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoWikiAgent {
    Claude,
    Codex,
    /// Cursor's command line agent (`agent`).
    Cursor,
    /// GitHub Copilot CLI (`copilot`).
    Copilot,
}

impl RepoWikiAgent {
    /// Every tool, in the order a picker lists them.
    pub const ALL: [RepoWikiAgent; 4] = [
        RepoWikiAgent::Claude,
        RepoWikiAgent::Codex,
        RepoWikiAgent::Cursor,
        RepoWikiAgent::Copilot,
    ];

    /// The name on the wire and in `wenlan wiki ... --agent`.
    pub fn slug(self) -> &'static str {
        match self {
            RepoWikiAgent::Claude => "claude",
            RepoWikiAgent::Codex => "codex",
            RepoWikiAgent::Cursor => "cursor",
            RepoWikiAgent::Copilot => "copilot",
        }
    }

    /// The name people know the tool by.
    pub fn display_name(self) -> &'static str {
        match self {
            RepoWikiAgent::Claude => "Claude Code",
            RepoWikiAgent::Codex => "Codex",
            RepoWikiAgent::Cursor => "Cursor",
            RepoWikiAgent::Copilot => "GitHub Copilot",
        }
    }

    /// The programs the tool installs as, in the order to look for them.
    pub fn programs(self) -> &'static [&'static str] {
        match self {
            RepoWikiAgent::Claude => &["claude"],
            RepoWikiAgent::Codex => &["codex"],
            // `agent` is the newer name, but too common a word to look for first.
            RepoWikiAgent::Cursor => &["cursor-agent", "agent"],
            RepoWikiAgent::Copilot => &["copilot"],
        }
    }
}

/// Where a daily update runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoWikiRunner {
    /// On this computer, by the daemon, while it is on.
    #[default]
    Computer,
    /// On GitHub Actions, by a workflow the wiki pull request adds to the
    /// repository: runs with the computer off.
    Github,
}

/// When a bound Space's wiki is brought up to date by itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoWikiDaily {
    pub enabled: bool,
    /// Local hour (0-23) after which the day's run starts.
    pub hour: u8,
    pub agent: RepoWikiAgent,
    #[serde(default)]
    pub runs_on: RepoWikiRunner,
}

/// Where the daily workflow file stands in the repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoWikiWorkflowState {
    /// On the default branch: GitHub runs it every day.
    InRepo,
    /// In the open wiki pull request: runs once that is merged.
    InPr,
    /// Not written yet: the next update puts it in the pull request.
    Missing,
}

/// The last run of the daily workflow on GitHub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoWikiGithubRun {
    /// When it started, RFC 3339.
    pub at: String,
    pub url: String,
    /// `queued`, `in_progress` or `completed`.
    pub status: String,
    /// For a completed run: `success`, `failure` and so on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    /// The pull request the refresh last opened or updated, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
}

/// What a daily update on GitHub still needs, and how it last went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoWikiGithubStatus {
    /// `owner/name` on GitHub; `None` when `origin` is not on GitHub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// The GitHub CLI is installed and signed in.
    pub gh_ready: bool,
    pub workflow: RepoWikiWorkflowState,
    /// The repository has the secret the chosen AI tool needs; `None` when
    /// it could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<bool>,
    /// GitHub Actions may open pull requests in the repository; `None`
    /// when it could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_open_prs: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<RepoWikiGithubRun>,
    /// The repository's Actions page for the workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions_url: Option<String>,
}

/// A setup step for a daily update on GitHub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RepoWikiGithubRequest {
    /// Sign in to Claude in the browser (`claude setup-token`) and store the
    /// token as the repository secret `CLAUDE_CODE_OAUTH_TOKEN`. The token
    /// is never kept or returned.
    ClaudeToken,
    /// Store `key` as the repository secret the chosen tool reads: an
    /// OpenAI or Cursor API key, or a GitHub token for Copilot. The
    /// key is never kept or returned.
    #[serde(alias = "openai_key")]
    ApiKey { key: String },
    /// Let GitHub Actions open pull requests in the repository.
    AllowPullRequests,
}

/// A Space bound to a git repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoWikiBinding {
    pub space: String,
    /// Absolute path of the repository's working copy on this computer. Code
    /// bases cite files under it.
    pub repo_root: String,
    /// Folder inside the repository the pages are written to, `/` separated.
    pub folder: String,
    /// `origin`'s URL when the binding was made, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily: Option<RepoWikiDaily>,
    /// The last publish, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_publish: Option<RepoWikiPublishResult>,
    /// Local date (`YYYY-MM-DD`) of the last daily run that started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_daily_run: Option<String>,
    /// Fixes to the repository's own docs (README, docs, AGENTS.md and the
    /// like) an AI tool found, waiting for or in the pull request, and the
    /// ones the user declined.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub doc_fixes: Vec<RepoDocFix>,
    /// The last time an AI tool read the repository's docs against the code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_check: Option<RepoDocCheck>,
    /// Wiki page files people edited that the repository keeps as they
    /// wrote them, by path and digest, so each is reported once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept_edits: Vec<String>,
}

/// Where a doc fix stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoDocFixState {
    /// Found; goes into the pull request on the next publish.
    Pending,
    /// In the open pull request.
    InPr,
    /// The user declined it; the same fix is not suggested again.
    Dismissed,
}

/// Code lines a doc fix rests on, relative to the repository root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoDocCode {
    pub path: String,
    pub line_start: u32,
    pub line_end: u32,
}

/// One sentence of a repository doc the code no longer agrees with, and what
/// it should say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoDocFix {
    /// Stable for the same file and old text.
    pub id: String,
    /// File relative to the repository root, `/` separated.
    pub path: String,
    /// Text in the file now; occurs there exactly once.
    pub old_text: String,
    pub new_text: String,
    /// One sentence on what the code says instead.
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub code: Vec<RepoDocCode>,
    pub state: RepoDocFixState,
    /// Unix seconds.
    pub found_at: i64,
}

/// When the docs were last read against the code, and how many.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoDocCheck {
    /// Unix seconds.
    pub at: i64,
    pub docs: u32,
}

/// Body of `POST /api/spaces/{space}/wiki/docs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepoDocsRequest {
    /// Record a fix to a repository doc.
    Suggest {
        path: String,
        old_text: String,
        new_text: String,
        reason: String,
        #[serde(default)]
        code: Vec<RepoDocCode>,
    },
    /// The user declines a fix; it leaves the pull request on the next
    /// publish and is not suggested again.
    Dismiss { id: String },
    /// An AI tool finished reading `docs` docs against the code.
    Checked { docs: u32 },
}

/// Reply of `POST /api/spaces/{space}/wiki/docs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoDocsReply {
    /// The fix suggested or dismissed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<RepoDocFix>,
    /// Fixes waiting for or in the pull request.
    pub open: usize,
}

/// Body of `PUT /api/spaces/{space}/wiki/repo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindRepoWikiRequest {
    /// Any path inside the repository; the daemon stores its top level.
    pub repo_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Leave the daily setting as it is when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily: Option<RepoWikiDaily>,
}

/// A repository the pages of a Space could go into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoWikiCandidate {
    /// Absolute path of the working copy the pages cite code from.
    pub repo_root: String,
    /// Pages that rest on this repository's code alone.
    pub pages: usize,
    /// `origin`'s URL, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
}

/// Result of `GET /api/spaces/{space}/wiki/suggest`: most pages first, empty
/// when no page of the Space rests on a repository's code alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoWikiSuggestion {
    pub candidates: Vec<RepoWikiCandidate>,
}

/// What a publish did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoWikiPublishStatus {
    /// The repository's default branch, or the open wiki pull request,
    /// already holds these pages.
    NoChanges,
    /// A new pull request was opened.
    PrOpened,
    /// The open wiki pull request was updated.
    PrUpdated,
    /// The branch was pushed, but no pull request could be opened from here
    /// (no `gh`, or not signed in); `compare_url` opens one by hand.
    Pushed,
    /// No page of this Space rests on this repository's code alone.
    NothingToPublish,
}

/// Result of `POST /api/spaces/{space}/wiki/publish`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoWikiPublishResult {
    pub status: RepoWikiPublishStatus,
    /// Pages written into the repository.
    pub pages: usize,
    /// Pages of the Space left out because they cite memories, not code.
    pub left_out: usize,
    /// Pages left out for now because a file they cite differs between the
    /// working copy and the default branch on the remote (their line numbers
    /// may not match it). They go in once that code is pushed.
    #[serde(default)]
    pub waiting: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    /// GitHub page that opens a pull request for the branch by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare_url: Option<String>,
    /// Doc fixes in this publish's pull request.
    #[serde(default)]
    pub doc_fixes: usize,
    /// Wiki pages changed in the repository (by a person, or by a merged
    /// `wenlan wiki refresh`), taken back into Wenlan before the publish so
    /// the change is kept.
    #[serde(default)]
    pub taken_back: usize,
    /// Wiki pages changed in the repository while Wenlan changed them too:
    /// the repository keeps its text, and its version waits in Wenlan as a
    /// revision card.
    #[serde(default)]
    pub kept: usize,
    /// Unix seconds.
    pub at: i64,
}
