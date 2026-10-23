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
    /// Unix seconds.
    pub at: i64,
}
