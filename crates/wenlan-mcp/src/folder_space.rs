// SPDX-License-Identifier: Apache-2.0
//! The local stdio server follows the folder it was started in to a Space,
//! the way the terminal and the plugin do (`plugin/skills/README.md`,
//! "Choosing the active space", layers 4 and 5): a `~/.wenlan/spaces.toml`
//! mapping first, then a Space named like the git repository's folder.
//!
//! The folder is read once, at startup. Whether its names are registered
//! Spaces is asked again at most every [`RECHECK`], so a Space made while
//! the AI tool is open (`wenlan init`) is picked up without a restart. The
//! result is the last link of the chain: a strict pin, a `space` argument
//! and `WENLAN_DEFAULT_SPACE` all win over it.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use wenlan_types::folder_space::{folder_space_hint, FolderSpaceHint};

use crate::client::WenlanClient;
use crate::lock_state;

/// How long one answer from the registry is trusted.
const RECHECK: Duration = Duration::from_secs(30);

/// Read what `cwd` says about the Space. Runs `git`; call it off the async
/// runtime.
pub fn hint_for(cwd: &Path, home: Option<&Path>) -> FolderSpaceHint {
    let spaces_file = home.map(|home| home.join(".wenlan/spaces.toml"));
    folder_space_hint(cwd, spaces_file.as_deref(), home)
}

pub struct FolderSpaceWatch {
    hint: FolderSpaceHint,
    checked: Mutex<Option<Instant>>,
}

impl FolderSpaceWatch {
    pub fn new(hint: FolderSpaceHint) -> Self {
        Self {
            hint,
            checked: Mutex::new(None),
        }
    }

    /// Bring the folder's Space up to date before a tool call. Asks the
    /// daemon at most once per [`RECHECK`]; an unanswered question changes
    /// nothing.
    pub async fn refresh(&self, client: &WenlanClient) {
        if self.hint.mapped.is_none() && self.hint.repository.is_none() {
            return;
        }
        // Both win over the folder, so there is nothing to look up.
        if lock_state::is_locked() || lock_state::default_space().is_some() {
            return;
        }
        {
            let mut checked = self.checked.lock().expect("folder space check lock");
            if checked.is_some_and(|at| at.elapsed() < RECHECK) {
                return;
            }
            *checked = Some(Instant::now());
        }
        let Some(names) = client.space_names().await else {
            return;
        };
        let space = self
            .hint
            .registered(|name| names.contains(name))
            .map(str::to_string);
        if space != lock_state::folder_space() {
            match &space {
                Some(space) => {
                    tracing::info!(%space, "the working folder picks the Space for calls that name none")
                }
                None => tracing::info!("the working folder no longer picks a Space"),
            }
            lock_state::set_folder_space(space);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock_state::ENV_LOCK;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn space(name: &str) -> serde_json::Value {
        serde_json::json!({
            "id": name, "name": name, "description": null, "suggested": false,
            "starred": false, "is_default": false, "sort_order": 0,
            "memory_count": 0, "entity_count": 0, "created_at": 0.0, "updated_at": 0.0,
        })
    }

    async fn daemon_with(spaces: &[&str]) -> MockServer {
        let daemon = MockServer::start().await;
        let body: Vec<_> = spaces.iter().map(|name| space(name)).collect();
        Mock::given(method("GET"))
            .and(path("/api/spaces"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&daemon)
            .await;
        daemon
    }

    fn hint(mapped: Option<&str>, repository: Option<&str>) -> FolderSpaceHint {
        FolderSpaceHint {
            mapped: mapped.map(str::to_string),
            repository: repository.map(str::to_string),
        }
    }

    fn clean_environment() {
        std::env::remove_var("WENLAN_SPACE");
        std::env::remove_var("WENLAN_DEFAULT_SPACE");
        lock_state::init_from_env();
    }

    /// UX-CLI-5: an AI tool started in a repository writes to the Space
    /// named like it without being told.
    #[tokio::test]
    async fn ux_cli_5_a_tool_call_that_names_no_space_uses_the_repository_s_space() {
        let _guard = ENV_LOCK.lock().await;
        clean_environment();
        let daemon = daemon_with(&["wenlan", "other"]).await;
        let watch = FolderSpaceWatch::new(hint(None, Some("wenlan")));

        watch.refresh(&WenlanClient::new(daemon.uri())).await;

        assert_eq!(
            crate::tools::effective_space(&None).as_deref(),
            Some("wenlan")
        );
        // A named Space still wins.
        assert_eq!(
            crate::tools::effective_space(&Some("other".into())).as_deref(),
            Some("other")
        );
        clean_environment();
    }

    #[tokio::test]
    async fn a_repository_with_no_space_of_its_name_changes_nothing() {
        let _guard = ENV_LOCK.lock().await;
        clean_environment();
        let daemon = daemon_with(&["other"]).await;
        let watch = FolderSpaceWatch::new(hint(None, Some("wenlan")));

        watch.refresh(&WenlanClient::new(daemon.uri())).await;

        assert_eq!(crate::tools::effective_space(&None), None);
        clean_environment();
    }

    #[tokio::test]
    async fn a_folder_mapping_wins_over_the_repository_name() {
        let _guard = ENV_LOCK.lock().await;
        clean_environment();
        let daemon = daemon_with(&["mapped", "wenlan"]).await;
        let watch = FolderSpaceWatch::new(hint(Some("mapped"), Some("wenlan")));

        watch.refresh(&WenlanClient::new(daemon.uri())).await;

        assert_eq!(
            crate::tools::effective_space(&None).as_deref(),
            Some("mapped")
        );
        clean_environment();
    }

    #[tokio::test]
    async fn the_environment_default_wins_and_the_daemon_is_not_asked() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("WENLAN_SPACE");
        std::env::set_var("WENLAN_DEFAULT_SPACE", "fallback");
        lock_state::init_from_env();
        let daemon = daemon_with(&["wenlan", "fallback"]).await;
        let watch = FolderSpaceWatch::new(hint(None, Some("wenlan")));

        watch.refresh(&WenlanClient::new(daemon.uri())).await;

        assert_eq!(
            crate::tools::effective_space(&None).as_deref(),
            Some("fallback")
        );
        assert!(daemon.received_requests().await.unwrap().is_empty());
        clean_environment();
    }

    #[tokio::test]
    async fn the_registry_is_asked_once_within_the_recheck_window() {
        let _guard = ENV_LOCK.lock().await;
        clean_environment();
        let daemon = daemon_with(&["wenlan"]).await;
        let watch = FolderSpaceWatch::new(hint(None, Some("wenlan")));
        let client = WenlanClient::new(daemon.uri());

        watch.refresh(&client).await;
        watch.refresh(&client).await;

        assert_eq!(daemon.received_requests().await.unwrap().len(), 1);
        clean_environment();
    }

    #[tokio::test]
    async fn a_daemon_that_does_not_answer_keeps_what_was_known() {
        let _guard = ENV_LOCK.lock().await;
        clean_environment();
        lock_state::set_folder_space(Some("wenlan".into()));
        let watch = FolderSpaceWatch::new(hint(None, Some("wenlan")));

        // Nothing listens on this port.
        watch
            .refresh(&WenlanClient::new("http://127.0.0.1:9".into()))
            .await;

        assert_eq!(lock_state::folder_space().as_deref(), Some("wenlan"));
        clean_environment();
    }
}
