// SPDX-License-Identifier: AGPL-3.0-only
//! Images and videos placed in pages. The daemon copies each file into the
//! pages folder's `_attachments/` (`crates/wenlan-core/src/attachments.rs`);
//! this module hands it files and lets the webview load that one folder.

use crate::state::AppState;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::ipc::{InvokeBody, Request};
use tokio::sync::RwLock;
use wenlan_types::requests::AttachFileRequest;
use wenlan_types::responses::{AttachFileResponse, KnowledgePathResponse};

type State = Arc<RwLock<AppState>>;

/// Longest file name accepted for pasted or dropped bytes.
const MAX_NAME_BYTES: usize = 200;

async fn client(state: &tauri::State<'_, State>) -> crate::api::WenlanClient {
    state.read().await.client.clone()
}

/// The folder that holds files shown in pages. The webview may load files
/// from this folder only (not the rest of the disk).
#[tauri::command]
pub async fn page_attachments_dir(
    app: tauri::AppHandle,
    state: tauri::State<'_, State>,
) -> Result<String, String> {
    let resp: KnowledgePathResponse = client(&state).await.get_json("/api/knowledge/path").await?;
    let dir = PathBuf::from(resp.path).join(wenlan_core::attachments::ATTACHMENTS_DIR);
    tauri::Manager::asset_protocol_scope(&app)
        .allow_directory(&dir, false)
        .map_err(|error| error.to_string())?;
    Ok(dir.to_string_lossy().into_owned())
}

/// Copy a file picked or dropped by its path.
#[tauri::command]
pub async fn attach_file_to_page(
    state: tauri::State<'_, State>,
    path: String,
    alt: Option<String>,
) -> Result<AttachFileResponse, String> {
    client(&state)
        .await
        .post_json(
            "/api/attachments",
            &AttachFileRequest {
                source_path: path,
                alt,
            },
        )
        .await
}

/// Copy pasted or dropped bytes. The body is the raw file; the `x-file-name`
/// header carries its percent-encoded name.
#[tauri::command]
pub async fn attach_bytes_to_page(
    state: tauri::State<'_, State>,
    request: Request<'_>,
) -> Result<AttachFileResponse, String> {
    let (directory, path) = {
        let InvokeBody::Raw(bytes) = request.body() else {
            return Err("attachment bytes missing".into());
        };
        let name = request
            .headers()
            .get("x-file-name")
            .and_then(|value| value.to_str().ok())
            .map(percent_decode)
            .and_then(|name| safe_file_name(&name))
            .ok_or_else(|| "attachment name missing".to_string())?;
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = directory.path().join(&name);
        std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
        (directory, path)
    };
    let result = client(&state)
        .await
        .post_json(
            "/api/attachments",
            &AttachFileRequest {
                source_path: path.to_string_lossy().into_owned(),
                alt: None,
            },
        )
        .await;
    drop(directory);
    result
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The last path component of `name`, or `None` when nothing usable is left.
fn safe_file_name(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next()?.trim();
    if base.is_empty() || base.starts_with('.') || base.len() > MAX_NAME_BYTES {
        return None;
    }
    Some(base.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_percent_encoded_names() {
        assert_eq!(percent_decode("%E9%A6%96%E9%A0%81.png"), "首頁.png");
        assert_eq!(percent_decode("a%20b.png"), "a b.png");
        assert_eq!(percent_decode("100%.png"), "100%.png");
    }

    #[test]
    fn keeps_only_the_final_name() {
        assert_eq!(
            safe_file_name("../../etc/shot.png").as_deref(),
            Some("shot.png")
        );
        assert_eq!(
            safe_file_name("C:\\Users\\a\\b.mov").as_deref(),
            Some("b.mov")
        );
        assert_eq!(safe_file_name(".hidden.png"), None);
        assert_eq!(safe_file_name(""), None);
    }
}
