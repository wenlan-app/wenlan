// SPDX-License-Identifier: AGPL-3.0-only
//! Human Review decisions remain daemon-owned, typed and transactional.
use crate::{api::percent_encode_path_segment, state::AppState};
use std::sync::Arc;
use tokio::sync::RwLock;
use wenlan_types::research_disputes::*;
type State = Arc<RwLock<AppState>>;
#[tauri::command]
pub async fn get_research_dispute(
    state: tauri::State<'_, State>,
    space: String,
    id: String,
) -> Result<ResearchDisputeView, String> {
    let client = state.read().await.client.clone();
    client
        .get_json(&format!(
            "/api/research/disputes/{}?space={}",
            percent_encode_path_segment(&id),
            percent_encode_path_segment(&space)
        ))
        .await
}
#[tauri::command]
pub async fn decide_research_dispute(
    state: tauri::State<'_, State>,
    space: String,
    id: String,
    expected_payload_digest: String,
    decision: ResearchDisputeDecision,
    reason: String,
) -> Result<ResearchDisputeView, String> {
    let client = state.read().await.client.clone();
    client
        .post_json(
            &format!(
                "/api/research/disputes/{}/decision",
                percent_encode_path_segment(&id)
            ),
            &DecideResearchDisputeRequest {
                space,
                expected_payload_digest,
                decision,
                reason,
            },
        )
        .await
}
