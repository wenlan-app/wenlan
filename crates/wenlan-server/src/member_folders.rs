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
//! the member sees 產品/Q3/<page id>.md
//! ```
//!
//! Only calls the team gate forwarded for a member, pinned to one Space,
//! are read this way. With the layout off, a member has no folders.

use std::path::Path;

use wenlan_core::db::MemoryDB;
use wenlan_core::error::WenlanError;
use wenlan_core::space_folders;

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

    /// An older draft's stored path is physical. Apply the same boundary as
    /// on a newly requested destination before an eligible draft can publish.
    pub fn check_stored_folder(&self, path: &str) -> Result<(), WenlanError> {
        let shown = self.shown(path).ok_or_else(outside)?;
        let resolved = self.folder_path(&shown)?;
        if resolved != path {
            return Err(outside());
        }
        Ok(())
    }

    /// A member location contains the page's opaque ID and its shared folder.
    pub fn page_location(&self, id: &str, path: &str) -> Option<String> {
        let shown = self.shown(path)?;
        let (folder, _) = shown.rsplit_once('/')?;
        Some(format!("{folder}/{id}.md"))
    }

    /// Resolve the requested folder and the authorized page's live filename.
    /// Keep the requested folder so a retry or stale move still uses the
    /// original compare-and-swap precondition.
    pub fn file_path(&self, id: &str, shown: &str, live: &str) -> Result<String, WenlanError> {
        let (folder, name) = shown.rsplit_once('/').ok_or_else(outside)?;
        if name != format!("{id}.md") || self.shown(live).is_none() {
            return Err(outside());
        }
        let (_, basename) = live.rsplit_once('/').ok_or_else(outside)?;
        Ok(format!("{}/{basename}", self.folder_path(folder)?))
    }
}

pub(crate) async fn required_view(
    db: &MemoryDB,
    root: &Path,
    pinned: Option<&str>,
) -> Result<MemberView, crate::error::ServerError> {
    view(db, root, true, pinned).await.ok_or_else(unavailable)
}

pub(crate) fn unavailable() -> crate::error::ServerError {
    crate::error::ServerError::ValidationError(
        "Folders need each Space kept in a folder of its own".into(),
    )
}

fn outside() -> WenlanError {
    WenlanError::Validation("not a folder of this Space".into())
}

#[cfg(test)]
#[path = "member_folders_test.rs"]
mod tests;
