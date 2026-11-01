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

/// What `GET /api/attachments/resolve` answers.
#[derive(serde::Deserialize)]
struct ResolvedEmbed {
    path: String,
}

/// Image and video extensions a page may show, the same set the webview
/// accepts in `src/lib/pageAttachments.ts`.
const MEDIA_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "heic", "mp4", "m4v", "mov", "webm",
];

/// Whether `path` is an image or video inside `root` and outside any hidden
/// folder such as `.obsidian/`.
fn is_showable_embed(path: &std::path::Path, root: &std::path::Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    if relative.components().any(|part| {
        !matches!(part, std::path::Component::Normal(_))
            || part.as_os_str().to_string_lossy().starts_with('.')
    }) {
        return false;
    }
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| MEDIA_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Where an Obsidian embed (`![[name]]`) points: the daemon finds the file in
/// the pages folder the way Obsidian does, and the webview may then load that
/// one file. With `page_id`, a file in that page's own folder wins. `None`
/// when nothing matches or the daemon cannot say.
#[tauri::command]
pub async fn resolve_page_embed(
    app: tauri::AppHandle,
    state: tauri::State<'_, State>,
    name: String,
    page_id: Option<String>,
) -> Result<Option<String>, String> {
    let client = client(&state).await;
    let root: KnowledgePathResponse = client.get_json("/api/knowledge/path").await?;
    let mut path = format!(
        "/api/attachments/resolve?name={}",
        crate::api::percent_encode_path_segment(&name)
    );
    if let Some(page_id) = page_id.filter(|id| !id.is_empty()) {
        path.push_str("&page_id=");
        path.push_str(&crate::api::percent_encode_path_segment(&page_id));
    }
    let Ok(resolved) = client.get_json::<ResolvedEmbed>(&path).await else {
        return Ok(None);
    };
    let (Ok(file), Ok(root)) = (
        std::fs::canonicalize(&resolved.path),
        std::fs::canonicalize(&root.path),
    ) else {
        return Ok(None);
    };
    if !is_showable_embed(&file, &root) {
        return Ok(None);
    }
    tauri::Manager::asset_protocol_scope(&app)
        .allow_file(&file)
        .map_err(|error| error.to_string())?;
    Ok(Some(file.to_string_lossy().into_owned()))
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
    // The request body is borrowed, so the file is written before any await.
    let temp = {
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
        write_temp(&name, bytes)?
    };
    attach_temp(&state, temp).await
}

/// `bytes` in a temporary folder, as a file named `name`.
fn write_temp(name: &str, bytes: &[u8]) -> Result<(tempfile::TempDir, PathBuf), String> {
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let path = directory.path().join(name);
    std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
    Ok((directory, path))
}

/// Have the daemon copy a temporary file in, then remove it.
async fn attach_temp(
    state: &tauri::State<'_, State>,
    (directory, path): (tempfile::TempDir, PathBuf),
) -> Result<AttachFileResponse, String> {
    let result = client(state)
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

/// Largest image fetched for a pasted image link, matching the daemon's image limit.
const MAX_REMOTE_IMAGE_BYTES: usize = 50 * 1024 * 1024;

/// Download an image the clipboard only names by its web address, as Chrome's
/// "Copy image" can leave it, and copy it into the pages folder. The webview
/// cannot fetch it itself: most image hosts refuse cross-origin reads.
#[tauri::command]
pub async fn attach_image_url_to_page(
    state: tauri::State<'_, State>,
    url: String,
) -> Result<AttachFileResponse, String> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|error| error.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("only web images can be fetched".into());
    }
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!("Wenlan/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    let mut response = http
        .get(parsed.clone())
        .header(reqwest::header::ACCEPT, "image/*")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("{} from the image link", response.status()));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let extension = image_extension(&content_type)
        .ok_or_else(|| format!("the link is not an image ({content_type})"))?;
    if response
        .content_length()
        .is_some_and(|length| length as usize > MAX_REMOTE_IMAGE_BYTES)
    {
        return Err("image is too large".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if bytes.len() + chunk.len() > MAX_REMOTE_IMAGE_BYTES {
            return Err("image is too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let name = remote_image_name(&parsed, extension);
    let temp = write_temp(&name, &bytes)?;
    attach_temp(&state, temp).await
}

/// The file extension for an image MIME type a page can show.
fn image_extension(content_type: &str) -> Option<&'static str> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    Some(match mime.as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" | "image/pjpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/svg+xml" => "svg",
        "image/heic" => "heic",
        _ => return None,
    })
}

/// A file name from the link's last path segment, with the extension the
/// server's content type calls for.
fn remote_image_name(url: &reqwest::Url, extension: &str) -> String {
    let segment = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .map(percent_decode)
        .unwrap_or_default();
    let stem = segment
        .rsplit_once('.')
        .map_or(segment.as_str(), |(stem, _)| stem);
    let stem: String = stem
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .take(60)
        .collect();
    let stem = stem.trim().trim_start_matches('.');
    // Google's thumbnails all live at ".../images?q=tbn:…".
    let stem = if stem.is_empty() || stem == "images" {
        "pasted-image"
    } else {
        stem
    };
    format!("{stem}.{extension}")
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
    fn shows_only_media_inside_the_pages_folder() {
        let root = std::path::Path::new("/vault");
        assert!(is_showable_embed(
            std::path::Path::new("/vault/img/a.PNG"),
            root
        ));
        assert!(!is_showable_embed(
            std::path::Path::new("/vault/.obsidian/a.png"),
            root
        ));
        assert!(!is_showable_embed(
            std::path::Path::new("/elsewhere/a.png"),
            root
        ));
        assert!(!is_showable_embed(
            std::path::Path::new("/vault/notes/a.md"),
            root
        ));
    }

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

    #[test]
    fn names_remote_images_by_their_content_type() {
        assert_eq!(image_extension("image/jpeg; charset=binary"), Some("jpg"));
        assert_eq!(image_extension("IMAGE/PNG"), Some("png"));
        assert_eq!(image_extension("text/html"), None);
        let url = |s: &str| reqwest::Url::parse(s).unwrap();
        assert_eq!(
            remote_image_name(
                &url("https://encrypted-tbn0.gstatic.com/images?q=tbn:abc"),
                "jpg"
            ),
            "pasted-image.jpg"
        );
        assert_eq!(
            remote_image_name(
                &url("https://upload.wikimedia.org/a/b/Cat%20photo.webp"),
                "jpg"
            ),
            "Cat photo.jpg"
        );
        assert_eq!(
            remote_image_name(&url("https://example.com/"), "png"),
            "pasted-image.png"
        );
    }
}
