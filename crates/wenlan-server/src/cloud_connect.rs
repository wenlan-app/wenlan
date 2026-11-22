// SPDX-License-Identifier: AGPL-3.0-only
//! Connecting this computer to the person's Wenlan cloud.
//!
//! ```text
//! app ── POST /api/cloud/connect ──▶ this daemon ── POST /auth/device ──▶ cloud
//!                                     │ the code, shown in the app; the
//!                                     │ person allows it in the browser
//!                                     └─ polls /auth/device/token ──▶ the key
//! ```
//!
//! With the key, the library is kept in the cloud too
//! ([`wenlan_core::shared_space::cloud`]): every library pass makes the
//! cloud and the library folder hold the same files. A computer whose
//! library is not synced anywhere yet gets one in its data folder.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::Json;
use tokio::sync::RwLock;
use wenlan_core::shared_space::{self, cloud, library, Scope};
use wenlan_types::shared_space::{CloudConnectStart, CloudConnection, CloudStatus};
use wenlan_types::team::{TeamDeviceGrant, TeamDeviceStart};

use crate::error::ServerError;
use crate::route_registry::{get, post, TrackedRouter};
use crate::state::{ServerState, SharedState};

/// The folder, in the data folder, that holds the library of a computer
/// that syncs it nowhere else.
pub const LOCAL_LIBRARY_DIR: &str = "cloud-library";

#[derive(Default)]
struct Connecting {
    /// Bumped by every connect and disconnect: a poll of an older one stops.
    round: u64,
    waiting: Option<CloudConnectStart>,
    failed: Option<String>,
}

static CONNECTING: Mutex<Connecting> = Mutex::new(Connecting {
    round: 0,
    waiting: None,
    failed: None,
});

fn connecting() -> std::sync::MutexGuard<'static, Connecting> {
    CONNECTING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn register(router: TrackedRouter<SharedState>) -> TrackedRouter<SharedState> {
    router
        .route(
            "/api/cloud",
            get(handle_cloud_status).delete(handle_cloud_disconnect),
        )
        .route("/api/cloud/connect", post(handle_cloud_connect))
}

fn client() -> Result<reqwest::Client, ServerError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| ServerError::Internal(error.to_string()))
}

fn status() -> CloudStatus {
    let settings = shared_space::settings_path();
    let state = cloud::read_state(&cloud::state_path(&settings));
    let connecting = connecting();
    match state.link {
        Some(link) => CloudStatus {
            state: CloudConnection::Connected,
            url: link.url,
            start: None,
            last_ok_at: state.last_ok_at,
            error: state.last_error,
        },
        None => CloudStatus {
            state: if connecting.waiting.is_some() {
                CloudConnection::Waiting
            } else {
                CloudConnection::Off
            },
            url: cloud::url(),
            start: connecting.waiting.clone(),
            last_ok_at: None,
            error: connecting.failed.clone(),
        },
    }
}

/// GET /api/cloud — whether this computer is connected, and how the last
/// sync with the cloud went. No key.
pub async fn handle_cloud_status() -> Json<CloudStatus> {
    Json(status())
}

/// POST /api/cloud/connect — asks the cloud for a short code; the person
/// allows this computer in the browser, and this daemon collects its key.
pub async fn handle_cloud_connect(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<CloudConnectStart>, ServerError> {
    if status().state == CloudConnection::Connected {
        return Err(ServerError::Conflict(
            "this computer is connected already".into(),
        ));
    }
    let url =
        shared_space::remote::check_url(&cloud::url()).map_err(ServerError::ValidationError)?;
    let response = client()?
        .post(format!("{url}/auth/device"))
        .json(&wenlan_types::team::TeamDeviceRequest {
            tool: library::computer_name(),
        })
        .send()
        .await
        .map_err(|_| ServerError::Upstream("Wenlan cloud cannot be reached".into()))?;
    if !response.status().is_success() {
        return Err(ServerError::Upstream(format!(
            "Wenlan cloud answered {}",
            response.status().as_u16()
        )));
    }
    let started: TeamDeviceStart = response
        .json()
        .await
        .map_err(|_| ServerError::Upstream("Wenlan cloud answered something else".into()))?;
    let shown = CloudConnectStart {
        user_code: started.user_code.clone(),
        verification_uri: started.verification_uri.clone(),
        expires_in: started.expires_in,
    };
    let round = {
        let mut connecting = connecting();
        connecting.round += 1;
        connecting.waiting = Some(shown.clone());
        connecting.failed = None;
        connecting.round
    };
    tokio::spawn(poll(state, url, started, round));
    Ok(Json(shown))
}

/// DELETE /api/cloud — disconnects this computer (or stops connecting).
/// Its pages stay here and the cloud keeps its copy.
pub async fn handle_cloud_disconnect() -> Result<Json<CloudStatus>, ServerError> {
    {
        let mut connecting = connecting();
        connecting.round += 1;
        connecting.waiting = None;
        connecting.failed = None;
    }
    let settings = shared_space::settings_path();
    tokio::task::spawn_blocking(move || cloud::disconnect(&settings))
        .await
        .map_err(|error| ServerError::Internal(error.to_string()))?
        .map_err(ServerError::Internal)?;
    Ok(Json(status()))
}

/// Ends connecting round `round` (unless a newer one started) with `failed`.
fn end(round: u64, failed: Option<String>) {
    let mut connecting = connecting();
    if connecting.round == round {
        connecting.waiting = None;
        connecting.failed = failed;
    }
}

fn current(round: u64) -> bool {
    connecting().round == round
}

async fn poll(state: Arc<RwLock<ServerState>>, url: String, started: TeamDeviceStart, round: u64) {
    let Ok(client) = client() else {
        return end(round, Some("Wenlan cloud cannot be reached".into()));
    };
    let deadline = Instant::now() + Duration::from_secs(started.expires_in);
    let mut every = Duration::from_secs(started.interval.max(1));
    while Instant::now() < deadline {
        tokio::time::sleep(every).await;
        if !current(round) {
            return;
        }
        let answer = client
            .post(format!("{url}/auth/device/token"))
            .json(&wenlan_types::team::TeamDevicePoll {
                device_code: started.device_code.clone(),
            })
            .send()
            .await;
        let Ok(answer) = answer else {
            continue;
        };
        if answer.status().is_success() {
            let Ok(grant) = answer.json::<TeamDeviceGrant>().await else {
                return end(round, Some("Wenlan cloud answered something else".into()));
            };
            let finished = finish(&state, &url, grant, round).await;
            return end(round, finished.err());
        }
        let code = answer
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|body| body["error"].as_str().map(str::to_string))
            .unwrap_or_default();
        match code.as_str() {
            "slow_down" => every += Duration::from_secs(5),
            "access_denied" => return end(round, Some("this computer was not allowed".into())),
            "expired_token" => break,
            _ => {}
        }
    }
    end(round, Some("the code expired; connect again".into()));
}

/// Keeps the key, and makes sure the library is synced somewhere this
/// computer's passes reach: a computer that syncs it nowhere yet gets a
/// folder of its own, filled from the cloud first so it joins the library
/// its other computers already keep there.
async fn finish(
    state: &Arc<RwLock<ServerState>>,
    url: &str,
    grant: TeamDeviceGrant,
    round: u64,
) -> Result<(), String> {
    if !current(round) {
        return Ok(());
    }
    let settings = shared_space::settings_path();
    let link = cloud::CloudLink {
        url: url.to_string(),
        key: grant.key,
        key_id: grant.key_id,
    };
    {
        let settings = settings.clone();
        tokio::task::spawn_blocking(move || cloud::connect(&settings, link))
            .await
            .map_err(|error| error.to_string())??;
    }
    let shared = {
        let settings = settings.clone();
        tokio::task::spawn_blocking(move || {
            shared_space::share_settings_at(&settings, &Scope::Library)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?
    };
    if shared.is_some() {
        // The next library pass brings the cloud and the folder together.
        return Ok(());
    }
    let chosen = wenlan_core::config::data_root().join(LOCAL_LIBRARY_DIR);
    let engine = library::engine_folder(&chosen);
    std::fs::create_dir_all(&engine).map_err(|error| error.to_string())?;
    if let Some(Err(reason)) = cloud::mirror_library(&settings, &engine).await {
        return Err(reason);
    }
    // The person's own other computer's member, when the cloud has one.
    let member_id = shared_space::read_space_file(&engine)
        .ok()
        .flatten()
        .and_then(|file| file.members.into_iter().map(|m| m.id).min());
    crate::spaces_routes::shared_sync(
        state,
        Scope::Library,
        wenlan_types::shared_space::SharedSyncRequest {
            folder: chosen.to_string_lossy().into_owned(),
            host: None,
            member_id,
            member_name: None,
        },
    )
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}
