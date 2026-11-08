// SPDX-License-Identifier: AGPL-3.0-only
//! Thin typed proxies for daemon-owned organizing records.
use crate::{api::percent_encode_path_segment, state::AppState};
use std::sync::Arc;
use tokio::sync::RwLock;
use wenlan_types::research::*;
type State = Arc<RwLock<AppState>>;
async fn client(state: &tauri::State<'_, State>) -> crate::api::WenlanClient {
    state.read().await.client.clone()
}
fn query(space: &str) -> String {
    format!("space={}", percent_encode_path_segment(space))
}
#[tauri::command]
pub async fn get_research_rules(
    state: tauri::State<'_, State>,
    space: String,
) -> Result<OrganizeRules, String> {
    client(&state)
        .await
        .get_json(&format!("/api/research/rules?{}", query(&space)))
        .await
}
#[tauri::command]
pub async fn save_research_rules(
    state: tauri::State<'_, State>,
    request: SaveOrganizeRulesRequest,
) -> Result<OrganizeRules, String> {
    client(&state)
        .await
        .post_json("/api/research/rules", &request)
        .await
}
#[tauri::command]
pub async fn list_research_runs(
    state: tauri::State<'_, State>,
    space: String,
) -> Result<ResearchRunsResponse, String> {
    client(&state)
        .await
        .get_json(&format!("/api/research/runs?{}", query(&space)))
        .await
}
#[tauri::command]
pub async fn create_research_run(
    state: tauri::State<'_, State>,
    request: CreateOrganizeRunRequest,
) -> Result<OrganizeRun, String> {
    client(&state)
        .await
        .post_json("/api/research/runs", &request)
        .await
}
#[tauri::command]
pub async fn get_research_run(
    state: tauri::State<'_, State>,
    space: String,
    id: String,
) -> Result<OrganizeRun, String> {
    client(&state)
        .await
        .get_json(&format!(
            "/api/research/runs/{}?{}",
            percent_encode_path_segment(&id),
            query(&space)
        ))
        .await
}
#[tauri::command]
pub async fn update_research_run(
    state: tauri::State<'_, State>,
    id: String,
    request: UpdateOrganizeRunRequest,
) -> Result<OrganizeRun, String> {
    client(&state)
        .await
        .post_json(
            &format!("/api/research/runs/{}", percent_encode_path_segment(&id)),
            &request,
        )
        .await
}
#[tauri::command]
pub async fn get_research_classification_decisions(
    state: tauri::State<'_, State>,
    space: String,
    source: String,
    source_id: String,
) -> Result<SourceClassificationDecisions, String> {
    let q = format!(
        "{}&source={}&source_id={}",
        query(&space),
        percent_encode_path_segment(&source),
        percent_encode_path_segment(&source_id)
    );
    client(&state)
        .await
        .get_json(&format!("/api/research/classifications?{q}"))
        .await
}
#[tauri::command]
pub async fn propose_research_classifications(
    state: tauri::State<'_, State>,
    request: ProposeSourceClassificationsRequest,
) -> Result<ClassificationProposalResult, String> {
    client(&state)
        .await
        .post_json("/api/research/classifications", &request)
        .await
}
#[tauri::command]
pub async fn accept_research_classification(
    state: tauri::State<'_, State>,
    id: String,
    space: Option<String>,
    changes: Vec<SourceClassificationChange>,
) -> Result<wenlan_types::responses::AcceptRefinementResponse, String> {
    let mut path = format!(
        "/api/refinery/queue/{}/accept",
        percent_encode_path_segment(&id)
    );
    if let Some(space) = space {
        path.push_str(&format!("?space={}", percent_encode_path_segment(&space)));
    }
    client(&state)
        .await
        .post_json_as_person(
            &path,
            &serde_json::json!({"action":"classification_edit","changes":changes}),
        )
        .await
}

#[tauri::command]
pub async fn merge_research_tags(
    state: tauri::State<'_, State>,
    request: MergeResearchTagsRequest,
) -> Result<MergeResearchTagsResponse, String> {
    client(&state)
        .await
        .post_json("/api/research/merge-tags", &request)
        .await
}
