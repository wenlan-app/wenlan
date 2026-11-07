// SPDX-License-Identifier: AGPL-3.0-only
//! Typed native proxies; every request uses the selected daemon client.
use crate::{api::percent_encode_path_segment, state::AppState};
use std::sync::Arc;
use tokio::sync::RwLock;
use wenlan_types::page_export::*;
type State = Arc<RwLock<AppState>>;
fn endpoint(page: &str, suffix: &str, space: Option<&str>) -> String {
    let mut path = format!(
        "/api/pages/{}/{}",
        percent_encode_path_segment(page),
        suffix
    );
    if let Some(space) = space {
        path.push_str(&format!("?space={}", percent_encode_path_segment(space)));
    }
    path
}
#[tauri::command]
pub async fn capture_page_export(
    state: tauri::State<'_, State>,
    page_id: String,
    space: Option<String>,
) -> Result<PageExportBundle, String> {
    let client = { state.read().await.client.clone() };
    client
        .post_json(
            &endpoint(&page_id, "export-capture", None),
            &CapturePageExportRequest { space },
        )
        .await
}
#[tauri::command]
pub async fn list_page_export_versions(
    state: tauri::State<'_, State>,
    page_id: String,
    space: Option<String>,
) -> Result<PageExportVersionsResponse, String> {
    let client = { state.read().await.client.clone() };
    client
        .get_json(&endpoint(&page_id, "export-versions", space.as_deref()))
        .await
}
#[tauri::command]
pub async fn get_page_export_bundle(
    state: tauri::State<'_, State>,
    page_id: String,
    bundle_id: String,
    space: Option<String>,
) -> Result<PageExportBundle, String> {
    let client = { state.read().await.client.clone() };
    client
        .get_json(&endpoint(
            &page_id,
            &format!("export-bundles/{}", percent_encode_path_segment(&bundle_id)),
            space.as_deref(),
        ))
        .await
}
