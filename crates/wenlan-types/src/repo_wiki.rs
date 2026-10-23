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
}

/// When a bound Space's wiki is brought up to date by itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoWikiDaily {
    pub enabled: bool,
    /// Local hour (0-23) after which the day's run starts.
    pub hour: u8,
    pub agent: RepoWikiAgent,
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
    /// Wiki pages someone edited in the repository, taken back into Wenlan
    /// before the publish so the edit is kept.
    #[serde(default)]
    pub taken_back: usize,
    /// Wiki pages someone edited in the repository while Wenlan changed them
    /// too: the repository keeps the person's text and Wenlan keeps its own.
    #[serde(default)]
    pub kept: usize,
    /// Unix seconds.
    pub at: i64,
}
