// SPDX-License-Identifier: Apache-2.0
//! The library kept in the person's Wenlan cloud as well.
//!
//! The cloud keeps a copy of the library's engine folder (`space.json` and
//! `members/<member>/<kind>/<id>/<sha>.wlv`, memories included). Around
//! every library pass this computer makes both the same:
//!
//! ```text
//! cloud ──▶ folder   every version file the folder does not have yet
//! folder ──▶ cloud   every version file the cloud does not have yet
//! space.json         whichever side changed since the last time; both
//!                    changed: their computers and members together
//! ```
//!
//! Version files never change and are named by their sha256, so each is
//! fetched or sent once. The cloud only stores: it never joins the library
//! as a computer, and a file removed from one side is not removed from the
//! other.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use serde::{Deserialize, Serialize};

use super::{version, SpaceFile, SPACE_FILE};

/// The personal cloud people connect to, unless `WENLAN_CLOUD_URL` says
/// otherwise.
pub const DEFAULT_URL: &str = "https://wenlan.app";
/// Overrides [`DEFAULT_URL`] (a trial server, a test).
pub const URL_ENV: &str = "WENLAN_CLOUD_URL";
/// This computer's link to the cloud and its record of the last
/// `space.json` both sides had, beside the share settings (never in the
/// library folder, which a cloud drive carries to other computers).
pub const STATE_FILE: &str = "cloud-library.json";

const TIMEOUT: Duration = Duration::from_secs(30);
const PARALLEL: usize = 8;

/// The cloud the library is kept in, and this computer's key for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudLink {
    /// `https://wenlan.app`.
    pub url: String,
    /// This computer's key (`wlc_…`).
    pub key: String,
    /// The key's id, as the person's list of computers shows it.
    pub key_id: String,
}

impl std::fmt::Display for CloudLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.url)
    }
}

/// The address to connect to.
pub fn url() -> String {
    std::env::var(URL_ENV)
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_URL.to_string())
}

/// Where `path` (as the cloud lists it) lives under `folder`, when it is a
/// file a library holds: `space.json`, or
/// `members/<member>/<pages|proposals|decisions|memories>/<id>/<sha>.wlv`.
/// None for anything else, so no path names a file outside the folder.
pub fn local_path(folder: &Path, path: &str) -> Option<PathBuf> {
    if path == SPACE_FILE {
        return Some(folder.join(SPACE_FILE));
    }
    match path.split('/').collect::<Vec<_>>().as_slice() {
        [members, member, kind, id, name]
            if *members == version::MEMBERS_DIR
                && (version::KIND_DIRS.contains(kind) || *kind == version::MEMORIES_DIR)
                && version::is_safe_id(member)
                && version::is_safe_id(id)
                && version::version_sha(name).is_some() =>
        {
            Some(
                folder
                    .join(members)
                    .join(member)
                    .join(kind)
                    .join(id)
                    .join(name),
            )
        }
        _ => None,
    }
}

/// Every library file in `folder`, as [`local_path`] takes them, sorted.
pub fn list_files(folder: &Path) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    if folder.join(SPACE_FILE).is_file() {
        out.push(SPACE_FILE.to_string());
    }
    let members = match std::fs::read_dir(folder.join(version::MEMBERS_DIR)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(error) => return Err(error),
    };
    let names = |dir: &Path| -> Vec<(String, PathBuf)> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
            .collect()
    };
    for member in members.flatten() {
        let Ok(member_name) = member.file_name().into_string() else {
            continue;
        };
        for (kind, kind_dir) in names(&member.path()) {
            for (id, id_dir) in names(&kind_dir) {
                for (name, _) in names(&id_dir) {
                    let path = format!("{}/{member_name}/{kind}/{id}/{name}", version::MEMBERS_DIR);
                    if local_path(folder, &path).is_some() {
                        out.push(path);
                    }
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

/// What one round did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirrorCounts {
    /// Files taken from the cloud.
    pub got: u32,
    /// Files sent to it.
    pub sent: u32,
}

/// This computer's [`STATE_FILE`]: the cloud it is connected to, and how
/// the last round went.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<CloudLink>,
    /// The sha256 of the `space.json` both sides last had.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub space_sha: String,
    /// When a round last finished (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok_at: Option<String>,
    /// Why the last round did not finish; cleared by one that does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Where this computer's [`CloudState`] is, beside the share settings.
pub fn state_path(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(STATE_FILE)
}

/// This computer's [`CloudState`]; empty when it never connected.
pub fn read_state(path: &Path) -> CloudState {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Writes it readable by this user only: it holds the computer's key.
fn write_state(path: &Path, state: &CloudState) -> Result<(), String> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|error| error.to_string())?;
    serde_json::to_writer(&mut tmp, state).map_err(|error| error.to_string())?;
    tmp.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    tmp.persist(path).map_err(|error| error.error.to_string())?;
    Ok(())
}

/// Changes this computer's [`CloudState`] under a lock, so a round and a
/// connect or disconnect never write over each other.
pub fn update_state<T>(
    path: &Path,
    change: impl FnOnce(&mut CloudState) -> T,
) -> Result<T, String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut state = read_state(path);
    let before = state.clone();
    let out = change(&mut state);
    if state != before {
        write_state(path, &state)?;
    }
    Ok(out)
}

/// Connects this computer: from now on every library pass keeps the cloud
/// the same as the library folder.
pub fn connect(settings_path: &Path, link: CloudLink) -> Result<(), String> {
    update_state(&state_path(settings_path), |state| {
        *state = CloudState {
            link: Some(link),
            ..CloudState::default()
        };
    })
}

/// Disconnects this computer. The pages stay here and the cloud keeps its
/// copy.
pub fn disconnect(settings_path: &Path) -> Result<(), String> {
    update_state(&state_path(settings_path), |state| {
        *state = CloudState::default();
    })
}

/// One round for the library at `folder` (its engine folder), when this
/// computer is connected; the outcome is kept for the person to see.
pub async fn mirror_library(
    settings_path: &Path,
    folder: &Path,
) -> Option<Result<MirrorCounts, String>> {
    let path = state_path(settings_path);
    let state = read_state(&path);
    let link = state.link.clone()?;
    // A library folder that is not there (a cloud drive not mounted) is
    // never made here.
    let outcome = if folder.parent().is_some_and(Path::is_dir) {
        mirror(&link, folder, &state.space_sha).await
    } else {
        Err(format!("{} is not there", folder.display()))
    };
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let kept = update_state(&path, |state| {
        // Disconnected or connected again meanwhile: not this round's.
        if state.link.as_ref() != Some(&link) {
            return;
        }
        match &outcome {
            Ok((_, sha)) => {
                if let Some(sha) = sha {
                    state.space_sha = sha.clone();
                }
                state.last_ok_at = Some(now);
                state.last_error = None;
            }
            Err(reason) => state.last_error = Some(reason.clone()),
        }
    });
    if let Err(error) = kept {
        log::warn!("[shared_space] Wenlan cloud state not kept: {error}");
    }
    Some(outcome.map(|(counts, _)| counts))
}

/// What becomes of `space.json`.
#[derive(Debug, PartialEq, Eq)]
enum SpaceStep {
    Same,
    Take,
    Send,
    /// Both changed: these bytes go to both sides.
    Both(Vec<u8>),
}

/// Decides `space.json` from this computer's copy, the cloud's, and the
/// sha both had last.
fn space_step(local: Option<&[u8]>, cloud: Option<&[u8]>, last: &str) -> SpaceStep {
    let sha = |bytes: &[u8]| version::sha256_hex(bytes);
    match (local, cloud) {
        (None, None) => SpaceStep::Same,
        (None, Some(_)) => SpaceStep::Take,
        (Some(_), None) => SpaceStep::Send,
        (Some(local), Some(cloud)) if local == cloud => SpaceStep::Same,
        (Some(local), Some(_)) if sha(local) == last => SpaceStep::Take,
        (Some(_), Some(cloud)) if sha(cloud) == last => SpaceStep::Send,
        (Some(local), Some(cloud)) => match merged_space(local, cloud) {
            Some(bytes) => SpaceStep::Both(bytes),
            // One side is not a library's space.json: keep the cloud's.
            None => SpaceStep::Take,
        },
    }
}

/// Both copies' members and computers, the cloud's first; the cloud's main
/// computer.
fn merged_space(local: &[u8], cloud: &[u8]) -> Option<Vec<u8>> {
    let local: SpaceFile = serde_json::from_slice(local).ok()?;
    let mut out: SpaceFile = serde_json::from_slice(cloud).ok()?;
    for member in local.members {
        if !out.members.iter().any(|m| m.id == member.id) {
            out.members.push(member);
        }
    }
    for device in local.devices {
        if !out.devices.iter().any(|d| d.id == device.id) {
            out.devices.push(device);
        }
    }
    if out.main_device.is_empty() {
        out.main_device = local.main_device;
    }
    Some((serde_json::to_string_pretty(&out).ok()? + "\n").into_bytes())
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("cannot reach Wenlan cloud: {error}"))
}

fn status_error(status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        401 => "Wenlan cloud no longer knows this computer; connect it again".into(),
        503 => "Wenlan cloud is busy; trying again soon".into(),
        code => format!("Wenlan cloud answered {code}"),
    }
}

#[derive(Deserialize)]
struct FileList {
    files: Vec<String>,
}

async fn list_cloud(client: &reqwest::Client, link: &CloudLink) -> Result<Vec<String>, String> {
    let response = client
        .get(format!("{}/team/v1/library/files", link.url))
        .bearer_auth(&link.key)
        .send()
        .await
        .map_err(|error| format!("cannot reach Wenlan cloud: {error}"))?;
    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }
    Ok(response
        .json::<FileList>()
        .await
        .map_err(|_| "Wenlan cloud answered something else".to_string())?
        .files)
}

async fn get_file(
    client: &reqwest::Client,
    link: &CloudLink,
    path: &str,
) -> Result<Vec<u8>, String> {
    let response = client
        .get(format!("{}/team/v1/library/file", link.url))
        .query(&[("path", path)])
        .bearer_auth(&link.key)
        .send()
        .await
        .map_err(|error| format!("cannot reach Wenlan cloud: {error}"))?;
    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("cannot reach Wenlan cloud: {error}"))?;
    if bytes.len() > super::remote::MAX_VERSION_BYTES {
        return Err(format!("{path} is too large"));
    }
    Ok(bytes.to_vec())
}

async fn put_file(
    client: &reqwest::Client,
    link: &CloudLink,
    path: &str,
    bytes: Vec<u8>,
) -> Result<(), String> {
    let response = client
        .put(format!("{}/team/v1/library/file", link.url))
        .query(&[("path", path)])
        .bearer_auth(&link.key)
        .body(bytes)
        .send()
        .await
        .map_err(|error| format!("cannot reach Wenlan cloud: {error}"))?;
    if response.status().is_success() {
        Ok(())
    } else if response.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
        // A file the cloud cannot keep as it is: sending it again would
        // not change that.
        log::warn!("[shared_space] Wenlan cloud did not keep {path}");
        Ok(())
    } else {
        Err(status_error(response.status()))
    }
}

/// Writes a file taken from the cloud. A version whose bytes do not match
/// its name is not kept; one already here is never replaced.
fn store(path: &str, local: &Path, bytes: &[u8]) -> Result<bool, String> {
    use std::io::Write;
    let io = |error: std::io::Error| format!("{}: {error}", local.display());
    if path != SPACE_FILE {
        let name = local
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if version::version_sha(name) != Some(version::sha256_hex(bytes).as_str()) {
            log::warn!("[shared_space] {path} from Wenlan cloud does not match its name");
            return Ok(false);
        }
    }
    let dir = local.parent().ok_or_else(|| format!("{path}: no folder"))?;
    std::fs::create_dir_all(dir).map_err(io)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".writing-")
        .tempfile_in(dir)
        .map_err(io)?;
    tmp.write_all(bytes).map_err(io)?;
    tmp.flush().map_err(io)?;
    if path == SPACE_FILE {
        tmp.persist(local).map_err(|error| io(error.error))?;
    } else {
        match tmp.persist_noclobber(local) {
            Ok(_) => {}
            Err(_) if local.exists() => return Ok(false),
            Err(error) => return Err(io(error.error)),
        }
    }
    Ok(true)
}

fn first_error(results: Vec<Result<bool, String>>) -> Result<u32, String> {
    let mut count = 0;
    for result in results {
        if result? {
            count += 1;
        }
    }
    Ok(count)
}

/// Makes the library's engine `folder` and the cloud hold the same files.
/// `last` is the sha of the `space.json` both had last; the one they have
/// now is returned with the counts.
pub async fn mirror(
    link: &CloudLink,
    folder: &Path,
    last: &str,
) -> Result<(MirrorCounts, Option<String>), String> {
    let client = client()?;
    let in_cloud = list_cloud(&client, link).await?;
    let here = {
        let owned = folder.to_path_buf();
        tokio::task::spawn_blocking(move || list_files(&owned))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("{}: {error}", folder.display()))?
    };
    let cloud_set: HashSet<&str> = in_cloud
        .iter()
        .map(|path| version::version_key(path))
        .collect();
    let here_set: HashSet<&str> = here.iter().map(|path| version::version_key(path)).collect();

    let wanted: Vec<(String, PathBuf)> = in_cloud
        .iter()
        .filter(|path| *path != SPACE_FILE && !here_set.contains(version::version_key(path)))
        .filter_map(|path| Some((path.clone(), local_path(folder, path)?)))
        .collect();
    let got = first_error(
        futures::stream::iter(wanted)
            .map(|(path, local)| {
                let client = client.clone();
                async move {
                    let bytes = get_file(&client, link, &path).await?;
                    tokio::task::spawn_blocking(move || store(&path, &local, &bytes))
                        .await
                        .map_err(|error| error.to_string())?
                }
            })
            .buffer_unordered(PARALLEL)
            .collect()
            .await,
    )?;

    let missing: Vec<(String, PathBuf)> = here
        .iter()
        .filter(|path| *path != SPACE_FILE && !cloud_set.contains(version::version_key(path)))
        .filter_map(|path| Some((path.clone(), local_path(folder, path)?)))
        .collect();
    let mut sent = first_error(
        futures::stream::iter(missing)
            .map(|(path, local)| {
                let client = client.clone();
                async move {
                    let bytes = std::fs::read(&local).map_err(|error| error.to_string())?;
                    if bytes.len() > super::remote::MAX_VERSION_BYTES {
                        log::warn!("[shared_space] {path} is too large for Wenlan cloud");
                        return Ok(false);
                    }
                    put_file(&client, link, &path, bytes).await.map(|()| true)
                }
            })
            .buffer_unordered(PARALLEL)
            .collect()
            .await,
    )?;

    let mut got = got;
    let space_here = folder.join(SPACE_FILE);
    let local_space = std::fs::read(&space_here).ok();
    let cloud_space = if in_cloud.iter().any(|path| path == SPACE_FILE) {
        Some(get_file(&client, link, SPACE_FILE).await?)
    } else {
        None
    };
    let settled = match space_step(local_space.as_deref(), cloud_space.as_deref(), last) {
        SpaceStep::Same => local_space.or(cloud_space),
        SpaceStep::Take => {
            let bytes = cloud_space.unwrap_or_default();
            store(SPACE_FILE, &space_here, &bytes)?;
            got += 1;
            Some(bytes)
        }
        SpaceStep::Send => {
            let bytes = local_space.unwrap_or_default();
            put_file(&client, link, SPACE_FILE, bytes.clone()).await?;
            sent += 1;
            Some(bytes)
        }
        SpaceStep::Both(bytes) => {
            store(SPACE_FILE, &space_here, &bytes)?;
            put_file(&client, link, SPACE_FILE, bytes.clone()).await?;
            got += 1;
            sent += 1;
            Some(bytes)
        }
    };
    Ok((
        MirrorCounts { got, sent },
        settled.map(|bytes| version::sha256_hex(&bytes)),
    ))
}

#[cfg(test)]
#[path = "cloud_test.rs"]
mod tests;
