// SPDX-License-Identifier: Apache-2.0
//! Custom theme endpoints. The checks live in `wenlan_core::theme`, so the
//! app and MCP get the same verdict. Theme files are local
//! appearance settings, not memory: no database or user content is read.

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::SharedState;
use axum::{extract::Path, http::StatusCode, response::Json};
use wenlan_core::theme;
use wenlan_types::theme::{
    ActiveThemeResponse, CheckThemeRequest, CreateThemeRequest, SetActiveThemeRequest,
    ThemeCheckResponse, ThemeDetail, ThemeFile, ThemeListResponse, ThemeProblem,
};

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route(
            "/api/themes",
            get(handle_list_themes).post(handle_create_theme),
        )
        .route(
            "/api/themes/active",
            get(handle_get_active_theme).put(handle_set_active_theme),
        )
        .route("/api/themes/check", post(handle_check_theme))
        .route(
            "/api/themes/{id}",
            get(handle_get_theme).put(handle_save_theme),
        )
}

fn check_failed(id: Option<String>, problems: Vec<ThemeProblem>) -> ServerError {
    ServerError::Structured {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        body: serde_json::json!({
            "error": "theme_check_failed",
            "id": id,
            "problems": problems,
        }),
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ServerError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| ServerError::Internal("theme task failed".into()))
}

/// GET /api/themes — every theme folder with its check result, the folder an
/// AI tool should write to, and the chosen theme.
pub async fn handle_list_themes() -> Result<Json<ThemeListResponse>, ServerError> {
    Ok(Json(
        blocking(|| theme::list_themes(&theme::themes_dir())).await?,
    ))
}

/// GET /api/themes/active — the tokens and images the app applies. The app
/// polls `revision` to pick up edits made outside it.
pub async fn handle_get_active_theme() -> Result<Json<ActiveThemeResponse>, ServerError> {
    Ok(Json(
        blocking(|| theme::active_theme(&theme::themes_dir())).await?,
    ))
}

/// PUT /api/themes/active `{ "id": "night-bunny" }` or `{ "id": null }` to go
/// back to the built-in themes. A theme that fails its checks is refused with
/// 422 and the list of problems.
pub async fn handle_set_active_theme(
    Json(request): Json<SetActiveThemeRequest>,
) -> Result<Json<ActiveThemeResponse>, ServerError> {
    let root = theme::themes_dir();
    let id = request.id.clone();
    let result: Result<ActiveThemeResponse, Vec<ThemeProblem>> = blocking(move || {
        theme::set_active_id(&root, id.as_deref())?;
        Ok(theme::active_theme(&root))
    })
    .await?;
    result
        .map(Json)
        .map_err(|problems| check_failed(request.id, problems))
}

/// POST /api/themes/check `{ "id": "night-bunny" }` — run every check without
/// applying, so an AI tool can fix its file first. With `"theme": {...}` the
/// unsaved theme is checked instead and, when it passes, the tokens it would
/// set come back for the app's live preview.
pub async fn handle_check_theme(
    Json(request): Json<CheckThemeRequest>,
) -> Result<Json<ThemeCheckResponse>, ServerError> {
    let CheckThemeRequest { id, theme: unsaved } = request;
    let (problems, tokens) = {
        let id = id.clone();
        blocking(move || {
            let root = theme::themes_dir();
            match unsaved {
                Some(unsaved) => theme::check_unsaved(&root, &id, &unsaved),
                None => (theme::check_theme_id(&root, &id), Default::default()),
            }
        })
        .await?
    };
    Ok(Json(ThemeCheckResponse {
        ok: problems.is_empty(),
        id,
        problems,
        tokens,
    }))
}

/// POST /api/themes `{ "name": "Night Bunny", "base": "dark" }` — start a new
/// theme folder from the built-in colors.
pub async fn handle_create_theme(
    Json(request): Json<CreateThemeRequest>,
) -> Result<Json<ThemeDetail>, ServerError> {
    blocking(move || theme::create_theme(&theme::themes_dir(), &request.name, request.base))
        .await?
        .map(Json)
        .map_err(|problems| check_failed(None, problems))
}

/// GET /api/themes/{id} — one theme file for the app's editor.
pub async fn handle_get_theme(Path(id): Path<String>) -> Result<Json<ThemeDetail>, ServerError> {
    let lookup = id.clone();
    blocking(move || theme::read_theme(&theme::themes_dir(), &lookup))
        .await?
        .map(Json)
        .map_err(|problems| ServerError::Structured {
            status: StatusCode::NOT_FOUND,
            body: serde_json::json!({
                "error": "theme_not_found",
                "id": id,
                "problems": problems,
            }),
        })
}

/// PUT /api/themes/{id} with a theme file — save it when it passes every
/// check; a failing theme is refused with 422 and the file is left as it was.
pub async fn handle_save_theme(
    Path(id): Path<String>,
    Json(file): Json<ThemeFile>,
) -> Result<Json<ThemeDetail>, ServerError> {
    let target = id.clone();
    blocking(move || theme::save_theme(&theme::themes_dir(), &target, &file))
        .await?
        .map(Json)
        .map_err(|problems| check_failed(Some(id), problems))
}
