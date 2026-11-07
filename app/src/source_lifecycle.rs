// SPDX-License-Identifier: AGPL-3.0-only
//! Typed proxies; the daemon owns scoped status, CAS and impact transactions.
use crate::{api::percent_encode_path_segment, state::AppState};
use std::sync::Arc;
use tokio::sync::RwLock;
use wenlan_types::source_lifecycle::{
    SetSourceLifecycleRequest, SourceLifecycleResponse, SourceReferenceIdentity,
};
type State = Arc<RwLock<AppState>>;

#[tauri::command]
pub async fn resolve_source_reference(
    state: tauri::State<'_, State>,
    space: String,
    locator: String,
) -> Result<SourceReferenceIdentity, String> {
    let client = state.read().await.client.clone();
    client
        .get_json(&format!(
            "/api/research/source-lifecycle/reference?space={}&locator={}",
            percent_encode_path_segment(&space),
            percent_encode_path_segment(&locator)
        ))
        .await
}

#[tauri::command]
pub async fn get_source_lifecycle(
    state: tauri::State<'_, State>,
    space: String,
    source: String,
    source_id: String,
) -> Result<SourceLifecycleResponse, String> {
    let client = state.read().await.client.clone();
    client
        .get_json(&format!(
            "/api/research/source-lifecycle?space={}&source={}&source_id={}",
            percent_encode_path_segment(&space),
            percent_encode_path_segment(&source),
            percent_encode_path_segment(&source_id)
        ))
        .await
}

#[tauri::command]
pub async fn set_source_lifecycle(
    state: tauri::State<'_, State>,
    request: SetSourceLifecycleRequest,
) -> Result<SourceLifecycleResponse, String> {
    let client = state.read().await.client.clone();
    client
        .post_json("/api/research/source-lifecycle", &request)
        .await
}
