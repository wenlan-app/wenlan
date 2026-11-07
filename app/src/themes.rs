// SPDX-License-Identifier: AGPL-3.0-only
//! Tauri commands for custom themes.
//!
//! The daemon owns theme files and every check (`wenlan_core::theme`). The
//! app proxies the typed requests, reads the two decoration images the
//! daemon already checked, and copies an image the person picked into the
//! theme's folder so the daemon can check it.

use crate::api::WenlanClient;
use crate::state::AppState;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use wenlan_types::theme::{
    ActiveThemeResponse, CheckThemeRequest, CreateThemeRequest, SetActiveThemeRequest, ThemeBase,
    ThemeCheckResponse, ThemeDetail, ThemeFile, ThemeListResponse, ThemeProblem,
};

type State = Arc<RwLock<AppState>>;

/// Same caps as the daemon's checks; checked here too so a huge file is never
/// copied just to be refused.
const MAX_BACKGROUND_BYTES: u64 = 5 * 1024 * 1024;
const MAX_STICKER_BYTES: u64 = 1024 * 1024;

/// A save or apply the daemon refused, with the problems it found, so the
/// settings screen can show them next to the fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ThemeWriteOutcome<T> {
    Ok { value: T },
    Refused { problems: Vec<ThemeProblem> },
}

#[derive(Deserialize)]
struct RefusalBody {
    #[serde(default)]
    problems: Vec<ThemeProblem>,
}

/// The daemon answers a failed check with 422 and a JSON body; `WenlanClient`
/// folds that into the error text after the status.
fn refusal_problems(error: &str) -> Option<Vec<ThemeProblem>> {
    if !error.contains(" 422 ") {
        return None;
    }
    let body = &error[error.find('{')?..];
    serde_json::from_str::<RefusalBody>(body)
        .ok()
        .map(|b| b.problems)
        .filter(|p| !p.is_empty())
}

fn outcome<T>(result: Result<T, String>) -> Result<ThemeWriteOutcome<T>, String> {
    match result {
        Ok(value) => Ok(ThemeWriteOutcome::Ok { value }),
        Err(error) => match refusal_problems(&error) {
            Some(problems) => Ok(ThemeWriteOutcome::Refused { problems }),
            None => Err(error),
        },
    }
}

impl WenlanClient {
    pub async fn list_themes(&self) -> Result<ThemeListResponse, String> {
        self.get_json("/api/themes").await
    }

    pub async fn active_theme(&self) -> Result<ActiveThemeResponse, String> {
        self.get_json("/api/themes/active").await
    }

    pub async fn theme_detail(&self, id: &str) -> Result<ThemeDetail, String> {
        self.get_json(&theme_path(id)?).await
    }
}

/// `/api/themes/<id>`, refusing anything that is not a theme id so a page
/// can never steer the request to another daemon route.
fn theme_path(id: &str) -> Result<String, String> {
    if wenlan_core_free_valid_id(id) {
        Ok(format!("/api/themes/{id}"))
    } else {
        Err("not a theme id".into())
    }
}

async fn client(state: &tauri::State<'_, State>) -> WenlanClient {
    state.read().await.client.clone()
}

#[tauri::command]
pub async fn list_themes(state: tauri::State<'_, State>) -> Result<ThemeListResponse, String> {
    client(&state).await.list_themes().await
}

#[tauri::command]
pub async fn get_active_theme(
    state: tauri::State<'_, State>,
) -> Result<ActiveThemeResponse, String> {
    client(&state).await.active_theme().await
}

#[tauri::command]
pub async fn get_theme(state: tauri::State<'_, State>, id: String) -> Result<ThemeDetail, String> {
    client(&state).await.theme_detail(&id).await
}

#[tauri::command]
pub async fn create_theme(
    state: tauri::State<'_, State>,
    name: String,
    base: ThemeBase,
) -> Result<ThemeWriteOutcome<ThemeDetail>, String> {
    outcome(
        client(&state)
            .await
            .post_json("/api/themes", &CreateThemeRequest { name, base })
            .await,
    )
}

#[tauri::command]
pub async fn save_theme(
    state: tauri::State<'_, State>,
    id: String,
    theme: ThemeFile,
) -> Result<ThemeWriteOutcome<ThemeDetail>, String> {
    outcome(
        client(&state)
            .await
            .put_json(&theme_path(&id)?, &theme)
            .await,
    )
}

/// Check an unsaved theme for the editor's live preview and status line.
#[tauri::command]
pub async fn check_theme_draft(
    state: tauri::State<'_, State>,
    id: String,
    theme: ThemeFile,
) -> Result<ThemeCheckResponse, String> {
    client(&state)
        .await
        .post_json(
            "/api/themes/check",
            &CheckThemeRequest {
                id,
                theme: Some(theme),
            },
        )
        .await
}

/// Choose a theme, or `None` for the built-in look.
#[tauri::command]
pub async fn set_active_theme(
    state: tauri::State<'_, State>,
    id: Option<String>,
) -> Result<ThemeWriteOutcome<ActiveThemeResponse>, String> {
    outcome(
        client(&state)
            .await
            .put_json("/api/themes/active", &SetActiveThemeRequest { id })
            .await,
    )
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThemeImageKind {
    Background,
    Sticker,
}

impl ThemeImageKind {
    fn max_bytes(self) -> u64 {
        match self {
            Self::Background => MAX_BACKGROUND_BYTES,
            Self::Sticker => MAX_STICKER_BYTES,
        }
    }

    fn stem(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Sticker => "sticker",
        }
    }
}

/// png, jpeg or webp from the first bytes, the same rule the daemon applies.
fn image_type(head: &[u8]) -> Option<(&'static str, &'static str)> {
    if head.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some(("png", "image/png"))
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("jpg", "image/jpeg"))
    } else if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some(("webp", "image/webp"))
    } else {
        None
    }
}

/// The active theme's background or sticker as a `data:` URL. The path comes
/// from the daemon's checked answer, never from the page, and must sit inside
/// the themes folder the daemon reports.
#[tauri::command]
pub async fn active_theme_image(
    state: tauri::State<'_, State>,
    kind: ThemeImageKind,
) -> Result<Option<String>, String> {
    let client = client(&state).await;
    let active = client.active_theme().await?;
    let path = match kind {
        ThemeImageKind::Background => active.background.map(|b| b.path),
        ThemeImageKind::Sticker => active.sticker.map(|s| s.path),
    };
    let Some(path) = path else {
        return Ok(None);
    };
    let themes_dir = client.list_themes().await?.themes_dir;
    tokio::task::spawn_blocking(move || read_image_data_url(&themes_dir, Path::new(&path), kind))
        .await
        .map_err(|e| format!("theme image task failed: {e}"))?
        .map(Some)
}

/// One image inside a theme's folder as a `data:` URL, for the editor's
/// thumbnails. `file` must be a plain name; the folder check below refuses
/// anything that resolves outside the themes folder.
#[tauri::command]
pub async fn theme_image(
    state: tauri::State<'_, State>,
    id: String,
    file: String,
    kind: ThemeImageKind,
) -> Result<String, String> {
    if !wenlan_core_free_valid_id(&id)
        || file.is_empty()
        || file.contains(['/', '\\'])
        || file.starts_with('.')
    {
        return Err("not a theme image".into());
    }
    let themes_dir = client(&state).await.list_themes().await?.themes_dir;
    tokio::task::spawn_blocking(move || {
        let path = Path::new(&themes_dir).join(&id).join(&file);
        read_image_data_url(&themes_dir, &path, kind)
    })
    .await
    .map_err(|e| format!("theme image task failed: {e}"))?
}

/// Open the themes folder in Finder / Explorer, creating it first so an empty
/// install still has somewhere to show.
#[tauri::command]
pub async fn open_themes_folder(state: tauri::State<'_, State>) -> Result<(), String> {
    let themes_dir = client(&state).await.list_themes().await?.themes_dir;
    std::fs::create_dir_all(&themes_dir).map_err(|e| format!("themes folder: {e}"))?;
    open::that_detached(&themes_dir).map_err(|e| format!("could not open the themes folder: {e}"))
}

fn read_image_data_url(
    themes_dir: &str,
    path: &Path,
    kind: ThemeImageKind,
) -> Result<String, String> {
    let root = std::fs::canonicalize(themes_dir).map_err(|e| format!("themes folder: {e}"))?;
    let file = std::fs::canonicalize(path).map_err(|e| format!("theme image: {e}"))?;
    if !file.starts_with(&root) {
        return Err("theme image is outside the themes folder".into());
    }
    let meta = std::fs::metadata(&file).map_err(|e| format!("theme image: {e}"))?;
    if !meta.is_file() || meta.len() > kind.max_bytes() {
        return Err("theme image is not a file of an allowed size".into());
    }
    let bytes = std::fs::read(&file).map_err(|e| format!("theme image: {e}"))?;
    let (_, mime) = image_type(&bytes).ok_or("theme image is not png, jpeg or webp")?;
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// Copy an image the person picked into `<themes>/<id>/` as
/// `background.<ext>` or `sticker.<ext>`, and return that file name for the
/// theme file. The daemon checks it again when the theme is saved.
#[tauri::command]
pub async fn import_theme_image(
    state: tauri::State<'_, State>,
    id: String,
    kind: ThemeImageKind,
    source: String,
) -> Result<String, String> {
    let themes_dir = client(&state).await.list_themes().await?.themes_dir;
    tokio::task::spawn_blocking(move || {
        copy_theme_image(Path::new(&themes_dir), &id, kind, Path::new(&source))
    })
    .await
    .map_err(|e| format!("theme image task failed: {e}"))?
}

fn copy_theme_image(
    themes_dir: &Path,
    id: &str,
    kind: ThemeImageKind,
    source: &Path,
) -> Result<String, String> {
    if !wenlan_core_free_valid_id(id) {
        return Err("not a theme id".into());
    }
    let meta = std::fs::metadata(source).map_err(|e| format!("could not read the image: {e}"))?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > kind.max_bytes() {
        return Err(match kind {
            ThemeImageKind::Background => "too_large_background".into(),
            ThemeImageKind::Sticker => "too_large_sticker".into(),
        });
    }
    let bytes = std::fs::read(source).map_err(|e| format!("could not read the image: {e}"))?;
    let (ext, _) = image_type(&bytes).ok_or_else(|| "not_an_image".to_string())?;
    let dir: PathBuf = themes_dir.join(id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create the theme folder: {e}"))?;
    let name = format!("{}.{ext}", kind.stem());
    // Remove an older copy with another extension so the folder holds one.
    for other in ["png", "jpg", "webp"] {
        if other != ext {
            let _ = std::fs::remove_file(dir.join(format!("{}.{other}", kind.stem())));
        }
    }
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, &bytes)
        .and_then(|()| std::fs::rename(&tmp, dir.join(&name)))
        .map_err(|e| format!("could not copy the image: {e}"))?;
    Ok(name)
}

/// Mirrors `wenlan_core::theme::valid_theme_id` (the app does not link
/// wenlan-core); the daemon re-checks the id on every request anyway.
fn wenlan_core_free_valid_id(id: &str) -> bool {
    !id.is_empty()
        && !matches!(id, "active" | "check")
        && id.len() <= 64
        && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    #[test]
    fn refusals_become_problems_and_other_errors_stay_errors() {
        let refused = r#"HTTP PUT /api/themes/x returned 422 Unprocessable Entity: {"error":"theme_check_failed","id":"x","problems":[{"field":"colors.text","message":"too faint"}]}"#;
        match outcome::<()>(Err(refused.into())).unwrap() {
            ThemeWriteOutcome::Refused { problems } => assert_eq!(problems[0].field, "colors.text"),
            other => panic!("{other:?}"),
        }
        assert!(outcome::<()>(Err("HTTP PUT /x: connection refused".into())).is_err());
    }

    #[test]
    fn picked_images_are_copied_under_a_fixed_name_and_checked() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("my stars.PNG");
        std::fs::write(&src, PNG).unwrap();
        let themes = tmp.path().join("themes");
        let name = copy_theme_image(&themes, "night", ThemeImageKind::Background, &src).unwrap();
        assert_eq!(name, "background.png");
        assert_eq!(
            std::fs::read(themes.join("night/background.png")).unwrap(),
            PNG
        );

        let svg = tmp.path().join("bunny.svg");
        std::fs::write(&svg, "<svg/>").unwrap();
        assert_eq!(
            copy_theme_image(&themes, "night", ThemeImageKind::Sticker, &svg).unwrap_err(),
            "not_an_image"
        );
        assert!(copy_theme_image(&themes, "../x", ThemeImageKind::Sticker, &src).is_err());
    }

    #[test]
    fn data_urls_only_come_from_inside_the_themes_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let themes = tmp.path().join("themes");
        std::fs::create_dir_all(themes.join("night")).unwrap();
        std::fs::write(themes.join("night/sticker.png"), PNG).unwrap();
        let outside = tmp.path().join("outside.png");
        std::fs::write(&outside, PNG).unwrap();
        let root = themes.display().to_string();
        let url = read_image_data_url(
            &root,
            &themes.join("night/sticker.png"),
            ThemeImageKind::Sticker,
        )
        .unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        assert!(read_image_data_url(&root, &outside, ThemeImageKind::Sticker).is_err());
    }
}
