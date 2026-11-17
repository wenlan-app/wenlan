// SPDX-License-Identifier: AGPL-3.0-only
//! Folders as a team member sees them on the web.
//!
//! A member never sees a path on the server. While the owner keeps each
//! Space's pages in a folder of its own ([`wenlan_core::space_folders`]),
//! a member sees each of their Spaces as a top folder named after the
//! Space, and the folders inside it:
//!
//! ```text
//! on the server   Acme/產品/Q3/路線.md
//! the member sees 產品/Q3/路線.md
//! ```
//!
//! Only calls the team gate forwarded for a member, pinned to one Space,
//! are read this way. With the layout off, a member has no folders.

use std::path::Path;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use wenlan_core::db::MemoryDB;
use wenlan_core::error::WenlanError;
use wenlan_core::space_folders;

use crate::state::SharedState;

/// A member's call: their Space and that Space's folder on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemberView {
    pub space: String,
    pub folder: String,
}

/// The member view for a call, when the gate forwarded it for a member
/// pinned to `pinned` and that Space has a folder. A Space whose name
/// cannot be one folder (empty, `/`, `\`, `.`, `..`) has none here.
pub(crate) async fn view(
    db: &MemoryDB,
    root: &Path,
    member: bool,
    pinned: Option<&str>,
) -> Option<MemberView> {
    if !member {
        return None;
    }
    let space = pinned?.trim();
    if space.is_empty()
        || matches!(space, "." | "..")
        || space.contains(['/', '\\'])
        || space.chars().any(char::is_control)
    {
        return None;
    }
    let folder = space_folders::space_folder(db, root, space).await?;
    Some(MemberView {
        space: space.to_string(),
        folder,
    })
}

impl MemberView {
    /// A server path (relative to the pages folder) as the member sees it;
    /// `None` when it is outside their Space's folder.
    pub fn shown(&self, path: &str) -> Option<String> {
        let inside = space_folders::inside_space(&self.folder, path)?;
        Some(if inside.is_empty() {
            self.space.clone()
        } else {
            format!("{}/{inside}", self.space)
        })
    }

    /// A folder the member named, as a server path. It must be their
    /// Space's top folder or inside it.
    pub fn folder_path(&self, shown: &str) -> Result<String, WenlanError> {
        let inside = if shown == self.space {
            ""
        } else {
            shown
                .strip_prefix(self.space.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
                .ok_or_else(outside)?
        };
        space_folders::in_space(&self.folder, inside)
    }

    /// A page file the member named, as a server path: its folder as
    /// [`MemberView::folder_path`], its name checked by the move itself.
    pub fn file_path(&self, shown: &str) -> Result<String, WenlanError> {
        let (folder, name) = shown.rsplit_once('/').ok_or_else(outside)?;
        if name.is_empty() || name.contains('\\') {
            return Err(outside());
        }
        Ok(format!("{}/{name}", self.folder_path(folder)?))
    }
}

fn outside() -> WenlanError {
    WenlanError::Validation("not a folder of this Space".into())
}

/// Adds `space_path` (the file as the member sees it) next to every
/// `storage_path` in a member call's answer. The gate takes
/// `storage_path` itself out; `space_path` is all the member gets.
pub(crate) async fn add_space_paths(
    State(state): State<SharedState>,
    req: Request,
    next: Next,
) -> Response {
    let member = crate::team_actor::from_headers(req.headers()).is_some();
    if !member {
        return next.run(req).await;
    }
    // Read as the Space header extractor does: UTF-8, so a Space named in
    // Chinese (not visible ASCII) still counts.
    let pinned = req
        .headers()
        .get(crate::space_header::HEADER_NAME)
        .or_else(|| req.headers().get(crate::space_header::HEADER_NAME_LEGACY))
        .and_then(|value| std::str::from_utf8(value.as_bytes()).ok())
        .map(str::to_string);
    let response = next.run(req).await;
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if !is_json {
        return response;
    }
    let (db, root) = {
        let s = state.read().await;
        (
            s.db.clone(),
            s.lint_config.page_root().map(Path::to_path_buf),
        )
    };
    let (Some(db), Some(root)) = (db, root) else {
        return response;
    };
    let Some(view) = view(&db, &root, true, pinned.as_deref()).await else {
        return response;
    };
    let (parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    mark(&mut value, &view);
    let mut parts = parts;
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(
        parts,
        Body::from(serde_json::to_vec(&value).unwrap_or_default()),
    )
}

fn mark(value: &mut Value, view: &MemberView) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(path)) = map.get("storage_path") {
                let shown = view.shown(path).map(Value::String).unwrap_or(Value::Null);
                map.insert("space_path".into(), shown);
            }
            map.values_mut().for_each(|v| mark(v, view));
        }
        Value::Array(list) => list.iter_mut().for_each(|v| mark(v, view)),
        _ => {}
    }
}

#[cfg(test)]
#[path = "member_folders_test.rs"]
mod tests;
