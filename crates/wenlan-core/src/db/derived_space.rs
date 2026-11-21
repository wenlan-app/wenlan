// SPDX-License-Identifier: Apache-2.0
//! Which Space something Wenlan writes by itself goes to (docs/ux/spec.md,
//! UX-CHAT-5). Five paths write memories nobody asked for: a night's points,
//! its preferences, tool problems and lessons, a tidy that merges or updates,
//! the old-memory pass, and the filing of old points. They all decide here.
//!
//! The rules:
//! - A rewrite keeps exactly the Space of what it takes the place of
//!   ([`rewrite_space`]); never "whatever the default is".
//! - A new write from a conversation goes to the Space the conversation's
//!   label named, else the repository's or the thing's other conversations',
//!   else the default Space. But a Space shared with other people (a team's,
//!   or one shared by folder) is written only when a fact the person set up
//!   backs that Space for the conversation ([`SpaceFacts::backs`]); a label
//!   alone is a model's guess and is not one. Otherwise it is written
//!   personal (`Uncategorized`).
//! - A preference is about the person: never a shared Space.
//! - Nothing here ever asks for `Inherit`, which would land in whatever the
//!   default Space is.

use super::MemoryDB;
use crate::shared_space;
use crate::WenlanError;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use wenlan_types::WriteSpaceTarget;

/// What the person has set up that says a Space belongs to a folder, and
/// which Spaces are shared with other people. Read once per night.
#[derive(Debug, Clone, Default)]
pub struct SpaceFacts {
    /// The Spaces shared with other people, by name, with the folder each is
    /// shared through. `None` when the sharing settings could not be read:
    /// then nothing is known to be safe.
    shared: Option<BTreeMap<String, PathBuf>>,
    /// Repositories tied to a Space by `wenlan init` / `wenlan repo bind`:
    /// the repository's folder and the Space.
    bindings: Vec<(PathBuf, String)>,
    /// `~/.wenlan/spaces.toml`, the person's folder-to-Space mapping.
    spaces_file: Option<PathBuf>,
    home: Option<PathBuf>,
}

impl SpaceFacts {
    /// Read the facts from the given files. A repository store that cannot
    /// be read gives no bindings (a binding only ever allows a write).
    pub fn read(
        settings: &Path,
        repo_store: &Path,
        spaces_file: Option<&Path>,
        home: Option<&Path>,
    ) -> Self {
        let shared = match shared_space::spaces_shared_with_others(settings) {
            Ok(shared) => Some(shared),
            Err(error) => {
                log::warn!(
                    "[things] the shared Spaces could not be read, so what a night writes stays personal: {error}"
                );
                None
            }
        };
        let bindings = crate::repo_wiki::load_bindings_at(repo_store)
            .unwrap_or_default()
            .into_iter()
            .map(|binding| (PathBuf::from(binding.repo_root), binding.space))
            .collect();
        Self {
            shared,
            bindings,
            spaces_file: spaces_file.map(Path::to_path_buf),
            home: home.map(Path::to_path_buf),
        }
    }

    /// The facts on this computer.
    pub fn here() -> Self {
        let home = dirs::home_dir();
        let spaces_file = home.as_ref().map(|home| home.join(".wenlan/spaces.toml"));
        Self::read(
            &shared_space::settings_path(),
            &crate::repo_wiki::store_path(),
            spaces_file.as_deref(),
            home.as_deref(),
        )
    }

    /// A person who shares nothing and has set up no folder.
    pub fn nothing_shared() -> Self {
        Self {
            shared: Some(BTreeMap::new()),
            ..Self::default()
        }
    }

    /// Whether `space` is shared with other people; `None` when that cannot
    /// be told (the settings were unreadable).
    pub fn is_shared(&self, space: &str) -> Option<bool> {
        self.shared
            .as_ref()
            .map(|shared| shared.contains_key(space))
    }

    /// Whether the person's own setup puts a conversation that ran in `cwd`
    /// in `space`: its repository is tied to the Space, `spaces.toml` maps
    /// its folder to the Space, its repository's folder is named like the
    /// Space, or it ran inside the folder the Space is shared through.
    pub fn backs(&self, space: &str, cwd: Option<&str>) -> bool {
        let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) else {
            return false;
        };
        let cwd_path = Path::new(cwd);
        if let Some(root) = crate::conversation_intake::repo_root_of(cwd) {
            if self
                .bindings
                .iter()
                .any(|(bound, name)| name == space && same_path(bound, Path::new(&root)))
            {
                return true;
            }
        }
        let hint = wenlan_types::folder_space::folder_space_hint(
            cwd_path,
            self.spaces_file.as_deref(),
            self.home.as_deref(),
        );
        // The person's own mapping wins: a folder they mapped to another
        // Space is not this one because its repository is named like it.
        let named = match hint.mapped.as_deref() {
            Some(mapped) => Some(mapped),
            None => hint.repository.as_deref(),
        };
        if named == Some(space) {
            return true;
        }
        self.shared
            .as_ref()
            .and_then(|shared| shared.get(space))
            .is_some_and(|folder| !folder.as_os_str().is_empty() && cwd_path.starts_with(folder))
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)),
                    (Ok(a), Ok(b)) if a == b)
}

/// Where a write that takes the place of a memory goes: exactly where that
/// memory is. A memory with no Space stays with none.
pub fn rewrite_space(old: Option<&str>) -> WriteSpaceTarget {
    match old {
        Some(space) => WriteSpaceTarget::Named(space.to_string()),
        None => WriteSpaceTarget::Uncategorized,
    }
}

/// The conversation a write comes from.
#[derive(Debug, Clone, Copy)]
pub struct Place<'a> {
    pub thing_id: &'a str,
    pub session_id: &'a str,
    /// The Space its label named, or empty.
    pub label: &'a str,
    /// The folder it ran in.
    pub cwd: Option<&'a str>,
}

impl MemoryDB {
    /// The Space a conversation points at, before the sharing check: its
    /// label, else its repository's or the thing's other conversations'.
    async fn conversation_candidate(&self, place: &Place<'_>) -> Option<String> {
        if !place.label.is_empty() {
            return Some(place.label.to_string());
        }
        let in_repo = place
            .cwd
            .and_then(crate::conversation_intake::repo_root_of)
            .is_some();
        self.dream_point_space(place.thing_id, place.session_id, in_repo)
            .await
            .unwrap_or_else(|error| {
                log::warn!("[things] a Space for {}: {error}", place.thing_id);
                None
            })
    }

    /// Whether a write may go to `space` for a conversation that ran in
    /// `cwd`: any Space not shared with other people, and a shared one only
    /// when the person's setup backs it. With the sharing settings unread,
    /// none.
    async fn space_allowed(&self, facts: &SpaceFacts, space: &str, cwd: Option<&str>) -> bool {
        if facts.shared.is_none() {
            return false;
        }
        let canonical = match self.get_space_without_private_counts(space).await {
            Ok(found) => found.map(|found| found.name),
            Err(_) => None,
        };
        let names = [Some(space), canonical.as_deref()];
        let shared = names
            .iter()
            .flatten()
            .any(|name| facts.is_shared(name) == Some(true));
        if !shared {
            return true;
        }
        // Reads folders and runs `git`: not on the async thread.
        let facts = facts.clone();
        let names: Vec<String> = names.iter().flatten().map(|n| n.to_string()).collect();
        let cwd = cwd.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            names.iter().any(|name| facts.backs(name, cwd.as_deref()))
        })
        .await
        .unwrap_or(false)
    }

    /// The Space of a new memory a night keeps from a conversation (a point,
    /// a tool problem, a lesson), and the Space it names, if any. Never
    /// `Inherit`.
    pub async fn derived_conversation_space(
        &self,
        facts: &SpaceFacts,
        place: &Place<'_>,
    ) -> (WriteSpaceTarget, Option<String>) {
        let space = match self.conversation_candidate(place).await {
            Some(space) => Some(space),
            None => match self.get_default_space().await {
                Ok(default) => default.map(|space| space.name),
                Err(error) => {
                    log::warn!("[things] the default Space could not be read: {error}");
                    None
                }
            },
        };
        let Some(space) = space else {
            return (WriteSpaceTarget::Uncategorized, None);
        };
        if self.space_allowed(facts, &space, place.cwd).await {
            (WriteSpaceTarget::Named(space.clone()), Some(space))
        } else {
            log::info!(
                "[things] nothing the person set up ties this conversation to the shared Space {space}, so what it settled is kept personal"
            );
            (WriteSpaceTarget::Uncategorized, None)
        }
    }

    /// The Space of a preference: none. What the person said once about how
    /// they like to work applies in all their own projects, so it is kept
    /// outside every Space (never in a shared one, never guessed), and is
    /// given to the person's own AI tools in every Space (UX-CHAT-5).
    pub fn derived_preference_space(&self) -> WriteSpaceTarget {
        WriteSpaceTarget::Uncategorized
    }

    /// Where an old night point with no Space is filed: the Space its thing's
    /// other conversations are all in, unless that is shared with other
    /// people and nothing the person set up backs it for this conversation.
    pub async fn derived_filing_space(
        &self,
        facts: &SpaceFacts,
        thing_id: &str,
        session_id: &str,
    ) -> Result<Option<String>, WenlanError> {
        let Some(space) = self.dream_point_space(thing_id, session_id, false).await? else {
            return Ok(None);
        };
        let cwd = self.session_cwd(session_id).await?;
        Ok(self
            .space_allowed(facts, &space, cwd.as_deref())
            .await
            .then_some(space))
    }

    /// The folder a conversation ran in, as it was recorded.
    async fn session_cwd(&self, session_id: &str) -> Result<Option<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT cwd FROM agent_sessions WHERE id = ?1",
                libsql::params![session_id],
            )
            .await
            .map_err(|e| WenlanError::VectorDb(format!("session cwd: {e}")))?;
        Ok(
            match rows
                .next()
                .await
                .map_err(|e| WenlanError::VectorDb(format!("session cwd row: {e}")))?
            {
                Some(row) => row.get::<Option<String>>(0).unwrap_or(None),
                None => None,
            },
        )
    }
}
