// SPDX-License-Identifier: AGPL-3.0-only
//! The Space header's GitHub button: bind a Space to a repository, put its
//! code pages into that repository as a pull request, and set the daily
//! update. The daemon does the work (`crates/wenlan-server/src/repo_wiki_routes.rs`).

use crate::api::percent_encode_path_segment;
use crate::state::AppState;
use std::sync::Arc;
use tokio::sync::RwLock;
use wenlan_types::repo_wiki::{
    BindRepoWikiRequest, RepoDocsReply, RepoDocsRequest, RepoWikiBinding, RepoWikiDaily,
    RepoWikiGithubRequest, RepoWikiGithubStatus, RepoWikiPublishResult, RepoWikiSuggestion,
};

type State = Arc<RwLock<AppState>>;

async fn client(state: &tauri::State<'_, State>) -> crate::api::WenlanClient {
    state.read().await.client.clone()
}

fn wiki_path(space: &str, tail: &str) -> String {
    format!(
        "/api/spaces/{}/wiki/{tail}",
        percent_encode_path_segment(space)
    )
}

/// The repository `space` is bound to, or `None`.
#[tauri::command]
pub async fn get_repo_wiki(
    state: tauri::State<'_, State>,
    space: String,
) -> Result<Option<RepoWikiBinding>, String> {
    client(&state)
        .await
        .get_json(&wiki_path(&space, "repo"))
        .await
}

/// Repositories the code pages of `space` could go into, most pages first.
#[tauri::command]
pub async fn suggest_repo_wiki(
    state: tauri::State<'_, State>,
    space: String,
) -> Result<RepoWikiSuggestion, String> {
    client(&state)
        .await
        .get_json(&wiki_path(&space, "suggest"))
        .await
}

/// Bind `space` to the repository holding `repo_root`, or change its folder
/// or daily update. `daily: None` keeps the daily setting as it is.
#[tauri::command]
pub async fn bind_repo_wiki(
    state: tauri::State<'_, State>,
    space: String,
    repo_root: String,
    folder: Option<String>,
    daily: Option<RepoWikiDaily>,
) -> Result<RepoWikiBinding, String> {
    let req = BindRepoWikiRequest {
        repo_root,
        folder,
        daily,
    };
    client(&state)
        .await
        .put_json(&wiki_path(&space, "repo"), &req)
        .await
}

/// Unbind `space`. Pages already merged into the repository stay there.
#[tauri::command]
pub async fn unbind_repo_wiki(state: tauri::State<'_, State>, space: String) -> Result<(), String> {
    let _removed: serde_json::Value = client(&state)
        .await
        .delete_path(&wiki_path(&space, "repo"))
        .await?;
    Ok(())
}

/// Put the current pages into the repository and open or update the pull
/// request. Does not ask any AI to rewrite them.
#[tauri::command]
pub async fn publish_repo_wiki(
    state: tauri::State<'_, State>,
    space: String,
) -> Result<RepoWikiPublishResult, String> {
    client(&state)
        .await
        .post_empty(&wiki_path(&space, "publish"))
        .await
}

/// Decline the doc fix `id` of `space`: it leaves the pull request on the
/// next update and is not suggested again.
#[tauri::command]
pub async fn dismiss_repo_doc_fix(
    state: tauri::State<'_, State>,
    space: String,
    id: String,
) -> Result<RepoDocsReply, String> {
    client(&state)
        .await
        .post_json(&wiki_path(&space, "docs"), &RepoDocsRequest::Dismiss { id })
        .await
}

/// What a daily update on GitHub still needs in the repository of `space`,
/// and its last run.
#[tauri::command]
pub async fn get_repo_wiki_github(
    state: tauri::State<'_, State>,
    space: String,
) -> Result<RepoWikiGithubStatus, String> {
    client(&state)
        .await
        .get_json(&wiki_path(&space, "github"))
        .await
}

/// One setup step for a daily update on GitHub: store the AI tool's secret
/// (signing in to Claude in the browser, or a pasted OpenAI key), or let
/// Actions open pull requests. Returns the status after it.
#[tauri::command]
pub async fn setup_repo_wiki_github(
    state: tauri::State<'_, State>,
    space: String,
    request: RepoWikiGithubRequest,
) -> Result<RepoWikiGithubStatus, String> {
    client(&state)
        .await
        .post_json(&wiki_path(&space, "github"), &request)
        .await
}
